use crate::{AppError, Result};

pub fn ensure_not_blank(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(AppError::validation(format!("{field} cannot be blank")));
    }

    Ok(())
}

pub fn ensure_port(port: u16) -> Result<()> {
    if port == 0 {
        return Err(AppError::validation("port must be between 1 and 65535"));
    }

    Ok(())
}

pub fn ensure_file_path(field: &str, value: &str) -> Result<()> {
    ensure_not_blank(field, value)
}

pub fn is_safe_identifier_part(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
}

pub fn ensure_positive_limit(field: &str, value: u32) -> Result<()> {
    if value == 0 {
        return Err(AppError::validation(format!(
            "{field} must be greater than 0"
        )));
    }

    Ok(())
}
