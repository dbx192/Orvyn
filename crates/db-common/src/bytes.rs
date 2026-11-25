use base64::{engine::general_purpose::STANDARD, Engine};

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut size = bytes as f64;
    let mut unit = 0;

    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

pub fn base64_preview(bytes: &[u8], max_len: usize) -> String {
    let end = bytes.len().min(max_len);
    let encoded = STANDARD.encode(&bytes[..end]);

    if bytes.len() > max_len {
        format!("{encoded}...")
    } else {
        encoded
    }
}

/// Interpret database bytes as text using the result grid's preview rules.
pub fn byte_value_text(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    (!text.trim().is_empty() && !text.chars().any(char::is_control)).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
    }

    #[test]
    fn decodes_utf8_bytes_without_reencoding() {
        assert_eq!(byte_value_text(b"pure_cms"), Some("pure_cms"));
        assert_eq!(byte_value_text(" 数据库 ".as_bytes()), Some(" 数据库 "));
        for bytes in [&[0xff, 0xfe][..], b"abc\0", b" ", b""] {
            assert_eq!(byte_value_text(bytes), None);
        }
    }
}
