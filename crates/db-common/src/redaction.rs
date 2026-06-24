use serde::{Deserialize, Serialize};
use std::fmt;

const MASK: &str = "********";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(MASK)
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(MASK)
    }
}

pub fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        MASK.to_string()
    }
}

pub fn redact_connection_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };

    let authority_start = scheme_end + 3;
    let Some(at_offset) = url[authority_start..].find('@') else {
        return url.to_string();
    };

    let at_index = authority_start + at_offset;
    let authority = &url[authority_start..at_index];
    let Some(colon_offset) = authority.rfind(':') else {
        return url.to_string();
    };

    let password_start = authority_start + colon_offset + 1;
    format!("{}{}{}", &url[..password_start], MASK, &url[at_index..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_password_in_url() {
        let url = "postgres://user:secret@localhost:5432/app";
        assert_eq!(
            redact_connection_url(url),
            "postgres://user:********@localhost:5432/app"
        );
    }
}
