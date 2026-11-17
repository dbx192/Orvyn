use db_common::{AppError, CellValue, QueryResult, Result};
use rust_xlsxwriter::Workbook;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

// rust_xlsxwriter encodes control characters using Excel's _xHHHH_ escapes.
// Only U+FFFE/U+FFFF need filtering; removing C0 controls would lose data.
fn sanitize_xml(text: &str) -> Cow<'_, str> {
    if text.contains(['\u{fffe}', '\u{ffff}']) {
        Cow::Owned(
            text.chars()
                .filter(|c| !matches!(c, '\u{fffe}' | '\u{ffff}'))
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

const SHEET_ROWS: u32 = 1_048_576;

/// A native XLSX sheet written in row order with bounded memory.
pub(crate) struct XlsxWriter {
    workbook: Workbook,
    path: PathBuf,
    row: u32,
    columns: Vec<String>,
    sheet: usize,
}
fn error(error: rust_xlsxwriter::XlsxError) -> AppError {
    AppError::internal(error.to_string())
}
impl XlsxWriter {
    pub fn create(path: &Path, columns: &[String]) -> Result<Self> {
        if columns.len() > 16_384 {
            return Err(AppError::validation("XLSX supports at most 16384 columns"));
        }
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet_with_constant_memory();
        for (index, column) in columns.iter().enumerate() {
            worksheet
                .write_string(0, index as u16, &*sanitize_xml(column))
                .map_err(error)?;
        }
        Ok(Self {
            workbook,
            path: path.to_path_buf(),
            row: 1,
            columns: columns.to_vec(),
            sheet: 0,
        })
    }
    pub fn write_page(&mut self, result: &QueryResult) -> Result<()> {
        for row in &result.rows {
            if self.row == SHEET_ROWS {
                self.sheet += 1;
                let worksheet = self.workbook.add_worksheet_with_constant_memory();
                for (index, column) in self.columns.iter().enumerate() {
                    worksheet
                        .write_string(0, index as u16, &*sanitize_xml(column))
                        .map_err(error)?;
                }
                self.row = 1;
            }
            let worksheet = self
                .workbook
                .worksheet_from_index(self.sheet)
                .map_err(error)?;
            for (column, value) in row.values.iter().take(self.columns.len()).enumerate() {
                let column = column as u16;
                match value {
                    CellValue::Null => {}
                    CellValue::Bool(value) => {
                        worksheet
                            .write_boolean(self.row, column, *value)
                            .map_err(error)?;
                    }
                    CellValue::Float(value) if value.is_finite() => {
                        worksheet
                            .write_number(self.row, column, *value)
                            .map_err(error)?;
                    }
                    CellValue::Int(value) if value.unsigned_abs() <= 999_999_999_999_999 => {
                        worksheet
                            .write_number(self.row, column, *value as f64)
                            .map_err(error)?;
                    }
                    CellValue::UInt(value) if *value <= 999_999_999_999_999 => {
                        worksheet
                            .write_number(self.row, column, *value as f64)
                            .map_err(error)?;
                    }
                    // Excel only preserves 15 significant digits. Keep large integers
                    // and DECIMAL values as literal text to avoid silent rounding.
                    // write_string also prevents user text from becoming a formula.
                    value => {
                        let cell_text = super::export_cell_value(value);
                        let text = sanitize_xml(&cell_text);
                        worksheet
                            .write_string(self.row, column, &*text)
                            .map_err(|source| AppError::validation(format!(
                                "XLSX sheet {}, row {}, column {} ({}): {source}. Use CSV or JSON for text exceeding Excel's 32767-character cell limit.",
                                self.sheet + 1, self.row + 1, column + 1, self.columns[column as usize]
                            )))?;
                    }
                }
            }
            self.row += 1;
        }
        Ok(())
    }
    pub fn finish(&mut self) -> Result<()> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let temporary = tempfile::NamedTempFile::new_in(parent)?;
        self.workbook.save(temporary.path()).map_err(error)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|e| e.error)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_common::Row;
    use std::io::Read;

    #[test]
    fn workbook_preserves_large_integers_and_never_promotes_text_to_formulas() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.xlsx");
        let mut writer = XlsxWriter::create(&path, &["large".into(), "literal".into()]).unwrap();
        let page = QueryResult {
            statement: String::new(),
            columns: vec![],
            rows: vec![Row::new(vec![
                CellValue::UInt(u64::MAX),
                CellValue::Text("=1+1".into()),
            ])],
            affected_rows: 0,
            execution_time_ms: 0,
            notices: vec![],
            metadata: Default::default(),
        };
        writer.write_page(&page).unwrap();
        writer.write_page(&page).unwrap();
        writer.finish().unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut sheet = String::new();
        archive
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet)
            .unwrap();
        assert!(sheet.contains("18446744073709551615"));
        assert!(sheet.contains("=1+1"));
        assert!(!sheet.contains("<f>"));
        assert!(sheet.contains("r=\"A3\""));
    }

    #[test]
    fn escapes_control_characters_without_losing_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dirty.xlsx");
        let mut writer = XlsxWriter::create(&path, &["id\u{1}".into(), "note".into()]).unwrap();
        let page = QueryResult {
            statement: String::new(),
            columns: vec![],
            rows: vec![Row::new(vec![
                CellValue::Int(7),
                CellValue::Text("a\u{0}b\u{2}c".into()),
            ])],
            affected_rows: 0,
            execution_time_ms: 0,
            notices: vec![],
            metadata: Default::default(),
        };
        writer.write_page(&page).unwrap();
        writer.finish().unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut sheet = String::new();
        archive
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet)
            .unwrap();
        // The XML is valid while Excel can reconstruct the original controls.
        assert!(!sheet.contains('\u{1}'));
        assert!(!sheet.contains('\u{2}'));
        assert!(!sheet.contains('\u{0}'));
        assert!(sheet.contains("id_x0001_"));
        assert!(sheet.contains(">a_x0000_b_x0002_c<"));
    }
    fn single_text_page(text: &str) -> QueryResult {
        let mut page = QueryResult::empty("");
        page.rows.push(Row::new(vec![CellValue::Text(text.into())]));
        page
    }

    #[test]
    fn rolls_over_at_excel_boundary_with_headers_and_no_extra_empty_sheet() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("split.xlsx");
        let mut writer = XlsxWriter::create(&path, &["name".into()]).unwrap();
        writer.row = SHEET_ROWS - 1;
        writer.write_page(&single_text_page("last row")).unwrap();
        writer.write_page(&QueryResult::empty("")).unwrap();
        assert_eq!(writer.sheet, 0);
        writer.write_page(&single_text_page("next sheet")).unwrap();
        writer.finish().unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut first = String::new();
        archive
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut first)
            .unwrap();
        assert!(first.contains("r=\"A1048576\""));
        assert!(first.contains("last row"));
        let mut second = String::new();
        archive
            .by_name("xl/worksheets/sheet2.xml")
            .unwrap()
            .read_to_string(&mut second)
            .unwrap();
        assert!(second.contains("r=\"A1\""));
        assert!(second.contains(">name<"));
        assert!(second.contains("r=\"A2\""));
        assert!(second.contains("next sheet"));
        assert!(archive.by_name("xl/worksheets/sheet3.xml").is_err());
    }

    #[test]
    fn reports_oversized_cell_location_without_truncating_or_replacing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("existing.xlsx");
        std::fs::write(&path, b"original").unwrap();
        let mut writer = XlsxWriter::create(&path, &["description".into()]).unwrap();
        let error = writer
            .write_page(&single_text_page(&"a".repeat(32768)))
            .unwrap_err();
        assert!(error.message.contains("row 2, column 1 (description)"));
        assert!(error.message.contains("32767"));
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }

    #[test]
    fn failed_save_keeps_the_existing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("occupied.xlsx");
        std::fs::create_dir(&path).unwrap();
        let mut writer = XlsxWriter::create(&path, &["name".into()]).unwrap();
        writer.write_page(&single_text_page("value")).unwrap();
        assert!(writer.finish().is_err());
        assert!(path.is_dir());
    }

    #[test]
    #[ignore = "manual export throughput benchmark"]
    fn benchmark_large_workbook() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.xlsx");
        let started = std::time::Instant::now();
        let mut writer =
            XlsxWriter::create(&path, &["id".into(), "title".into(), "price".into()]).unwrap();
        let mut page = QueryResult::empty("");
        const ROWS: usize = 3_864_170;
        for offset in (0..ROWS).step_by(5_000) {
            page.rows = (offset..(offset + 5_000).min(ROWS))
                .map(|i| {
                    Row::new(vec![
                        CellValue::Int(i as i64),
                        CellValue::Text(format!("Book {i}")),
                        CellValue::Float(29.99),
                    ])
                })
                .collect();
            writer.write_page(&page).unwrap();
        }
        let write_time = started.elapsed();
        writer.finish().unwrap();
        assert_eq!(writer.sheet + 1, 4);
        eprintln!(
            "rows={ROWS}, writing={write_time:?}, total={:?}, bytes={}",
            started.elapsed(),
            std::fs::metadata(path).unwrap().len()
        );
    }
}
