use crate::{mysql_connect_options, mysql_connection_error, mysql_query_error};
use db_common::{AppError, ConnectionProfile, DataSourceKind, QueryCancellation, Result};
use encoding_rs::{Decoder, DecoderResult, Encoding, UTF_8};
use sqlx::{mysql::MySqlConnection, Connection};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    time::Duration,
};

const CHUNK_SIZE: usize = 64 * 1024;
const MAX_DELIMITER_BYTES: usize = 32;

#[derive(Clone, Copy, Default)]
pub enum ScriptEncoding {
    #[default]
    Auto,
    Explicit(&'static Encoding),
}

impl ScriptEncoding {
    pub fn from_label(label: &str) -> Result<Self> {
        if label.trim().is_empty() {
            return Ok(Self::Auto);
        }
        Encoding::for_label_no_replacement(label.trim().as_bytes())
            .map(Self::Explicit)
            .ok_or_else(|| AppError::validation("Unknown SQL file encoding"))
    }
}

#[derive(Clone, Copy, Default)]
pub struct ScriptProgress {
    pub statements: u64,
    pub bytes_read: u64,
    pub total_bytes: u64,
}

pub async fn run_sql_file(
    profile: ConnectionProfile,
    password: Option<String>,
    path: PathBuf,
    encoding: ScriptEncoding,
    cancellation: QueryCancellation,
    mut report: impl FnMut(ScriptProgress) + Send + 'static,
) -> Result<()> {
    if profile.security.read_only {
        return Err(AppError::validation(
            "SQL file execution requires a writable connection",
        ));
    }
    if !matches!(
        profile.kind,
        DataSourceKind::MySql | DataSourceKind::MariaDb
    ) {
        return Err(AppError::unsupported(
            "SQL file execution requires MySQL or MariaDB",
        ));
    }
    let options = mysql_connect_options(&profile, password.as_deref())?;
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(AppError::validation("Select a regular SQL file"));
        }
        let mut progress = ScriptProgress {
            total_bytes: metadata.len(),
            ..ScriptProgress::default()
        };
        report(progress);
        let input = ScriptInput::new(file, encoding, cancellation.clone())?;
        let mut script = Script::new(input);
        let mut connection = runtime.block_on(async {
            tokio::select! {
                _ = cancellation.cancelled() => Err(cancellation.error()),
                result = tokio::time::timeout(Duration::from_secs(30), MySqlConnection::connect_with(&options)) => {
                    result.map_err(|_| AppError::new(db_common::ErrorKind::Timeout, "MySQL connection timed out"))?
                        .map_err(mysql_connection_error)
                }
            }
        })?;
        let (packet_bytes, sql_mode): (u64, String) = runtime.block_on(
            sqlx::query_as("SELECT CAST(@@SESSION.max_allowed_packet AS UNSIGNED), @@SESSION.sql_mode")
                .fetch_one(&mut connection),
        ).map_err(mysql_query_error)?;
        let statement_limit = packet_bytes.saturating_sub(1) as usize;
        let mut mode = SqlMode::from_value(&sql_mode);
        loop {
            let statement = script.next_statement(statement_limit, mode)
                .map_err(|error| at_statement(error, progress.statements + 1))?;
            let Some(statement) = statement else { break };
            if cancellation.is_cancelled() {
                return Err(cancellation.error());
            }
            runtime.block_on(sqlx::raw_sql(&statement).execute(&mut connection))
                .map_err(mysql_query_error)
                .map_err(|error| at_statement(error, progress.statements + 1))?;
            progress.statements += 1;
            progress.bytes_read = script.input.bytes_read;
            report(progress);
            if may_change_session(&statement) {
                let (value, client_charset, connection_charset): (String, String, String) = runtime.block_on(
                    sqlx::query_as("SELECT @@SESSION.sql_mode, @@SESSION.character_set_client, @@SESSION.character_set_connection")
                        .fetch_one(&mut connection),
                ).map_err(mysql_query_error)?;
                if [&client_charset, &connection_charset].into_iter().any(|charset|
                    !matches!(charset.as_str(), "utf8mb4" | "utf8mb3" | "utf8")
                ) {
                    return Err(at_statement(AppError::validation(
                        "The script switched the MySQL session to a non-UTF-8 charset. Convert the file and its session charset directives to UTF-8 before running it."
                    ), progress.statements));
                }
                mode = SqlMode::from_value(&value);
            }
        }
        progress.bytes_read = script.input.bytes_read;
        report(progress);
        Ok(())
    })
    .await
    .map_err(|error| AppError::internal("SQL file worker failed").with_detail(error.to_string()))?
}

fn at_statement(mut error: AppError, number: u64) -> AppError {
    error.message = format!("SQL statement {number}: {}", error.message);
    error
}

fn may_change_session(statement: &str) -> bool {
    let first = statement
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .next()
        .unwrap_or_default();
    ["SET", "CALL", "EXECUTE"]
        .iter()
        .any(|keyword| first.eq_ignore_ascii_case(keyword))
        || statement.starts_with("/*!")
        || statement.starts_with("/*M!")
}

struct ScriptInput<R> {
    reader: BufReader<R>,
    decoder: Decoder,
    decoded: Vec<u8>,
    offset: usize,
    bytes_read: u64,
    done: bool,
    cancellation: QueryCancellation,
}

impl<R: Read> ScriptInput<R> {
    fn new(reader: R, encoding: ScriptEncoding, cancellation: QueryCancellation) -> Result<Self> {
        let mut reader = BufReader::with_capacity(CHUNK_SIZE, reader);
        let mut prefix = [0; 3];
        let mut size = 0;
        while size < prefix.len() {
            let count = reader.read(&mut prefix[size..])?;
            if count == 0 {
                break;
            }
            size += count;
        }
        let bom = Encoding::for_bom(&prefix[..size]);
        let (encoding, skip) = match encoding {
            ScriptEncoding::Auto => bom.unwrap_or((UTF_8, 0)),
            ScriptEncoding::Explicit(encoding) => match bom {
                Some((actual, size)) if actual == encoding => (encoding, size),
                Some(_) => {
                    return Err(AppError::validation(
                        "SQL file BOM conflicts with selected encoding",
                    ))
                }
                None => (encoding, 0),
            },
        };
        let mut input = Self {
            reader,
            decoder: encoding.new_decoder_without_bom_handling(),
            decoded: Vec::with_capacity(CHUNK_SIZE * 3),
            offset: 0,
            bytes_read: size as u64,
            done: false,
            cancellation,
        };
        let capacity = input.decoded.capacity();
        input.decoded.resize(capacity, 0);
        let (status, _, written) = input.decoder.decode_to_utf8_without_replacement(
            &prefix[skip..size],
            &mut input.decoded,
            false,
        );
        input.decoded.truncate(written);
        if matches!(status, DecoderResult::Malformed(..)) {
            return Err(invalid_encoding());
        }
        Ok(input)
    }

    fn window(&mut self, width: usize) -> Result<&[u8]> {
        if self.decoded.len() - self.offset < width && !self.done {
            self.decoded.drain(..self.offset);
            self.offset = 0;
            while self.decoded.len() < width && !self.done {
                if self.cancellation.is_cancelled() {
                    return Err(self.cancellation.error());
                }
                let source = self.reader.fill_buf()?;
                let start = self.decoded.len();
                self.decoded.resize(start + CHUNK_SIZE * 3 + 4, 0);
                let (status, consumed, written) = self.decoder.decode_to_utf8_without_replacement(
                    source,
                    &mut self.decoded[start..],
                    source.is_empty(),
                );
                self.done = source.is_empty();
                self.decoded.truncate(start + written);
                self.reader.consume(consumed);
                self.bytes_read += consumed as u64;
                if matches!(status, DecoderResult::Malformed(..)) {
                    return Err(invalid_encoding());
                }
            }
        }
        Ok(&self.decoded[self.offset..])
    }

    fn consume(&mut self, count: usize) {
        self.offset += count;
    }
}

fn invalid_encoding() -> AppError {
    AppError::validation("Invalid bytes for the selected SQL file encoding")
}

#[derive(Clone, Copy, Default)]
struct SqlMode {
    no_backslash_escapes: bool,
    ansi_quotes: bool,
}

impl SqlMode {
    fn from_value(value: &str) -> Self {
        Self {
            no_backslash_escapes: value.split(',').any(|mode| mode == "NO_BACKSLASH_ESCAPES"),
            ansi_quotes: value.split(',').any(|mode| mode == "ANSI_QUOTES"),
        }
    }
}

enum LexState {
    Sql,
    Quote(u8),
    LineComment,
    BlockComment(bool),
}

struct Script<R> {
    input: ScriptInput<R>,
    delimiter: Vec<u8>,
}

impl<R: Read> Script<R> {
    fn new(input: ScriptInput<R>) -> Self {
        Self {
            input,
            delimiter: vec![b';'],
        }
    }

    fn next_statement(&mut self, limit: usize, mode: SqlMode) -> Result<Option<String>> {
        let mut statement = Vec::new();
        let mut state = LexState::Sql;
        loop {
            let window = self.input.window(10.max(self.delimiter.len()))?;
            let Some(&byte) = window.first() else {
                if !matches!(state, LexState::Sql | LexState::LineComment) {
                    return Err(AppError::validation(
                        "Unterminated SQL quote or block comment",
                    ));
                }
                break;
            };
            let mut keep = true;
            let mut consume = 1;
            match state {
                LexState::Sql => {
                    if window.starts_with(&self.delimiter) {
                        self.input.consume(self.delimiter.len());
                        if statement.is_empty() {
                            continue;
                        }
                        break;
                    }
                    if statement.is_empty()
                        && window
                            .get(..9)
                            .is_some_and(|word| word.eq_ignore_ascii_case(b"delimiter"))
                        && window.get(9).is_some_and(u8::is_ascii_whitespace)
                    {
                        self.input.consume(9);
                        self.read_delimiter()?;
                        continue;
                    }
                    match byte {
                        b'\'' | b'"' | b'`' => state = LexState::Quote(byte),
                        b'#' => {
                            state = LexState::LineComment;
                            keep = false;
                        }
                        b'-' if window.get(1) == Some(&b'-')
                            && window.get(2).is_none_or(|byte| {
                                byte.is_ascii_whitespace() || byte.is_ascii_control()
                            }) =>
                        {
                            state = LexState::LineComment;
                            keep = false;
                            consume = 2;
                        }
                        b'/' if window.starts_with(b"/*") => {
                            keep = window.starts_with(b"/*!")
                                || window.starts_with(b"/*+")
                                || window.starts_with(b"/*M!");
                            state = LexState::BlockComment(keep);
                            consume = 2;
                            if !keep && !statement.is_empty() {
                                statement.push(b' ');
                            }
                        }
                        _ if statement.is_empty() && byte.is_ascii_whitespace() => keep = false,
                        _ => {}
                    }
                }
                LexState::Quote(quote) => {
                    let escapes = !mode.no_backslash_escapes
                        && quote != b'`'
                        && !(quote == b'"' && mode.ansi_quotes);
                    if byte == b'\\' && escapes && window.len() >= 2 {
                        consume = 2;
                    } else if byte == quote {
                        if window.get(1) == Some(&quote) {
                            consume = 2;
                        } else {
                            state = LexState::Sql;
                        }
                    } else if byte != b'\\' || !escapes {
                        consume = if escapes {
                            memchr::memchr2(quote, b'\\', window)
                        } else {
                            memchr::memchr(quote, window)
                        }
                        .unwrap_or(window.len());
                    }
                }
                LexState::LineComment => {
                    keep = false;
                    if matches!(byte, b'\r' | b'\n') {
                        state = LexState::Sql;
                        keep = !statement.is_empty();
                    } else {
                        consume = memchr::memchr2(b'\r', b'\n', window).unwrap_or(window.len());
                    }
                }
                LexState::BlockComment(preserve) => {
                    keep = preserve;
                    if window.starts_with(b"*/") {
                        state = LexState::Sql;
                        consume = 2;
                    }
                }
            }
            if keep {
                if statement.len().saturating_add(consume) > limit {
                    return Err(AppError::validation(
                        "SQL statement exceeds the server's max_allowed_packet",
                    ));
                }
                statement.extend_from_slice(&window[..consume]);
            }
            if statement.len() > limit {
                return Err(AppError::validation(
                    "SQL statement exceeds the server's max_allowed_packet",
                ));
            }
            self.input.consume(consume);
        }
        if statement.is_empty() {
            Ok(None)
        } else {
            String::from_utf8(statement)
                .map(Some)
                .map_err(|_| invalid_encoding())
        }
    }

    fn read_delimiter(&mut self) -> Result<()> {
        let mut delimiter = Vec::new();
        let mut trailing_space = false;
        while let Some(&byte) = self.input.window(1)?.first() {
            self.input.consume(1);
            if matches!(byte, b'\r' | b'\n') {
                break;
            }
            if byte.is_ascii_whitespace() {
                trailing_space = !delimiter.is_empty();
            } else if trailing_space
                || !byte.is_ascii_graphic()
                || byte == b'\\'
                || delimiter.len() == MAX_DELIMITER_BYTES
            {
                return Err(AppError::validation("Invalid MySQL DELIMITER command"));
            } else {
                delimiter.push(byte);
            }
        }
        if delimiter.is_empty() {
            return Err(AppError::validation("Missing MySQL statement delimiter"));
        }
        self.delimiter = delimiter;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{repeat, Cursor};

    fn input<R: Read>(reader: R, encoding: ScriptEncoding) -> Script<R> {
        Script::new(ScriptInput::new(reader, encoding, QueryCancellation::default()).unwrap())
    }

    #[test]
    fn handles_mysql_dump_quoting_and_client_delimiters_across_reads() {
        let source = br"-- dump header
DELIMITER //
CREATE PROCEDURE `test`() BEGIN SELECT 'it\'s; //'; SELECT 'a''b'; END//
DELIMITER ;
/*!40101 SET SQL_MODE='NO_BACKSLASH_ESCAPES' */;
SELECT 'x\';
SELECT 1/* comment */+2; # done
";
        let source = source.iter().fold(Vec::new(), |mut result, byte| {
            result.push(*byte);
            result
        });
        let mut script = input(ShortReads(Cursor::new(source)), ScriptEncoding::Auto);
        let mut next = |mode| script.next_statement(1024, mode).unwrap();
        let default = SqlMode::default();
        assert_eq!(
            next(default).unwrap(),
            "CREATE PROCEDURE `test`() BEGIN SELECT 'it\\'s; //'; SELECT 'a''b'; END"
        );
        assert!(may_change_session(&next(default).unwrap()));
        let mode = SqlMode::from_value("NO_BACKSLASH_ESCAPES");
        assert_eq!(next(mode).unwrap(), "SELECT 'x\\'");
        assert_eq!(next(mode).unwrap(), "SELECT 1 +2");
        assert!(next(mode).is_none());
    }

    struct ShortReads<R>(R);

    impl<R: Read> Read for ShortReads<R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let limit = buffer.len().min(1);
            self.0.read(&mut buffer[..limit])
        }
    }

    #[test]
    fn decodes_bom_and_explicit_encodings_without_replacing_errors() {
        let text = "SELECT '\u{5317}\u{4eac}'";
        let (gbk, _, errors) = encoding_rs::GBK.encode(text);
        assert!(!errors);
        let mut script = input(
            ShortReads(&*gbk),
            ScriptEncoding::from_label("GB18030").unwrap(),
        );
        assert_eq!(
            script
                .next_statement(128, SqlMode::default())
                .unwrap()
                .as_deref(),
            Some(text)
        );
        let utf16: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let mut script = input(ShortReads(&*utf16), ScriptEncoding::Auto);
        assert_eq!(
            script
                .next_statement(128, SqlMode::default())
                .unwrap()
                .as_deref(),
            Some(text)
        );
        let mut script = input(&b"SELECT '\xff'"[..], ScriptEncoding::Auto);
        assert!(script.next_statement(128, SqlMode::default()).is_err());
    }

    #[test]
    fn streams_past_two_gib_and_bounds_each_statement() {
        let size = 2 * 1024 * 1024 * 1024_u64;
        let reader = (&b"SELECT 1; #"[..])
            .chain(repeat(b'x').take(size))
            .chain(&b"\nSELECT 2;"[..]);
        let mut script = input(reader, ScriptEncoding::Auto);
        let mode = SqlMode::default();
        assert_eq!(
            script.next_statement(64, mode).unwrap().as_deref(),
            Some("SELECT 1")
        );
        assert_eq!(
            script.next_statement(64, mode).unwrap().as_deref(),
            Some("SELECT 2")
        );
        assert!(script.next_statement(64, mode).unwrap().is_none());
        assert!(script.input.bytes_read > size);
        assert!(script.input.decoded.capacity() < CHUNK_SIZE * 8);

        let reader = (&b"SELECT '"[..]).chain(repeat(b'x').take(size));
        let mut script = input(reader, ScriptEncoding::Auto);
        assert!(script
            .next_statement(64, mode)
            .unwrap_err()
            .message
            .contains("max_allowed_packet"));
        assert!(script.input.bytes_read < CHUNK_SIZE as u64 * 2);
    }
}
