use db_common::{AppError, CellValue, QueryResult, Result};
use rust_xlsxwriter::Workbook;
use std::path::{Path, PathBuf};

/// A native XLSX sheet written in row order with bounded memory.
pub(crate) struct XlsxWriter {
    workbook: Workbook,
    path: PathBuf,
    row: u32,
    columns: usize,
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
                .write_string(0, index as u16, column)
                .map_err(error)?;
        }
        Ok(Self {
            workbook,
            path: path.to_path_buf(),
            row: 1,
            columns: columns.len(),
        })
    }
    pub fn write_page(&mut self, result: &QueryResult) -> Result<()> {
        if u64::from(self.row) + result.rows.len() as u64 > 1_048_576 {
            return Err(AppError::validation(
                "XLSX supports at most 1048575 data rows plus the header",
            ));
        }
        let worksheet = self.workbook.worksheet_from_index(0).map_err(error)?;
        for row in &result.rows {
            for (column, value) in row.values.iter().take(self.columns).enumerate() {
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
                        worksheet
                            .write_string(self.row, column, super::export_cell_value(value))
                            .map_err(error)?;
                    }
                }
            }
            self.row += 1;
        }
        Ok(())
    }
    pub fn finish(&mut self) -> Result<()> {
        self.workbook.save(&self.path).map_err(error)
    }
}
