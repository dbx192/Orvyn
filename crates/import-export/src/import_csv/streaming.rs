//! Strict, bounded decoding into a temporary UTF-8 snapshot.
//! The snapshot prevents a file edited during import from changing halfway through.
use super::CsvImportOptions;
use db_common::{AppError, Result};
use encoding_rs::{DecoderResult, Encoding, GB18030, UTF_8};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub struct CsvRowReader {
    reader: csv::Reader<File>,
    _snapshot: tempfile::NamedTempFile,
    pub headers: Vec<String>,
    pub encoding: &'static str,
}
impl CsvRowReader {
    pub fn open(path: &Path, options: &CsvImportOptions) -> Result<Self> {
        Self::open_cancellable(path, options, None)
    }
    pub fn open_cancellable(
        path: &Path,
        options: &CsvImportOptions,
        cancellation: Option<&db_common::QueryCancellation>,
    ) -> Result<Self> {
        let mut input = File::open(path)?;
        let mut prefix = [0u8; 3];
        let count = input.read(&mut prefix)?;
        let bom = Encoding::for_bom(&prefix[..count]);
        let (encoding, skip, label) = match bom {
            Some((encoding, skip)) => (
                encoding,
                skip,
                if encoding == UTF_8 {
                    "UTF-8 BOM"
                } else if encoding == encoding_rs::UTF_16LE {
                    "UTF-16 LE"
                } else {
                    "UTF-16 BE"
                },
            ),
            None => (UTF_8, 0, "UTF-8"),
        };
        let mut snapshot = tempfile::NamedTempFile::new()?;
        let decoded = transcode(
            &mut input,
            snapshot.as_file_mut(),
            encoding,
            skip as u64,
            cancellation,
        );
        let label = match decoded {
            Ok(()) => label,
            Err(error) if bom.is_none() && error.kind == db_common::ErrorKind::TypeConversion => {
                snapshot.as_file_mut().set_len(0)?;
                snapshot.as_file_mut().seek(SeekFrom::Start(0))?;
                transcode(&mut input, snapshot.as_file_mut(), GB18030, 0, cancellation)?;
                "GB18030/GBK"
            }
            Err(error) => return Err(error),
        };
        snapshot.as_file_mut().flush()?;
        let mut file = snapshot.reopen()?;
        file.seek(SeekFrom::Start(0))?;
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(options.delimiter)
            .has_headers(options.has_header)
            .flexible(true)
            .from_reader(file);
        let record = reader.headers().map_err(csv_error)?;
        if record.is_empty() {
            return Err(AppError::validation("CSV file is empty"));
        }
        let headers = if options.has_header {
            record.iter().map(ToOwned::to_owned).collect()
        } else {
            (1..=record.len()).map(|i| format!("Column_{i}")).collect()
        };
        Ok(Self {
            reader,
            _snapshot: snapshot,
            headers,
            encoding: label,
        })
    }
    pub fn next_row(&mut self) -> Result<Option<Vec<String>>> {
        self.reader
            .records()
            .next()
            .transpose()
            .map_err(csv_error)
            .map(|record| record.map(|r| r.iter().map(ToOwned::to_owned).collect()))
    }
    pub fn read_batch(&mut self, limit: usize) -> Result<Vec<Vec<String>>> {
        let mut rows = Vec::new();
        while rows.len() < limit.max(1) {
            match self.next_row()? {
                Some(row) => rows.push(row),
                None => break,
            }
        }
        Ok(rows)
    }
}
fn csv_error(error: csv::Error) -> AppError {
    AppError::validation("CSV parse error").with_detail(error.to_string())
}
fn transcode(
    input: &mut File,
    output: &mut File,
    encoding: &'static Encoding,
    skip: u64,
    cancellation: Option<&db_common::QueryCancellation>,
) -> Result<()> {
    input.seek(SeekFrom::Start(skip))?;
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let mut bytes = [0u8; 8192];
    let mut utf8 = [0u8; 32768];
    loop {
        if cancellation.is_some_and(|token| token.is_cancelled()) {
            return Err(AppError::validation("CSV import cancelled"));
        }
        let count = input.read(&mut bytes)?;
        let last = count == 0;
        let mut used = 0;
        loop {
            let (result, read, written) =
                decoder.decode_to_utf8_without_replacement(&bytes[used..count], &mut utf8, last);
            output.write_all(&utf8[..written])?;
            used += read;
            match result {
                DecoderResult::InputEmpty => break,
                DecoderResult::OutputFull => continue,
                DecoderResult::Malformed(_, _) => {
                    return Err(AppError::new(
                        db_common::ErrorKind::TypeConversion,
                        "CSV contains an invalid character sequence",
                    ))
                }
            }
        }
        if last {
            return Ok(());
        }
    }
}
