//! AI provider settings persistence (`ai-settings.json`).

use db_common::CredentialRef;
use serde::{Deserialize, Serialize};
use std::fs;

pub const SETTINGS_FILE: &str = "ai-settings.json";
pub const API_KEY_LABEL: &str = "ai-api-key";

fn default_version() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

fn default_max_retries() -> u32 {
    2
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    #[serde(default = "default_version")]
    pub version: u32,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub models: Vec<String>,
    pub api_style: String,
    pub auth_method: String,
    #[serde(default = "default_true")]
    pub enable_thinking: bool,
    pub context_window: Option<u32>,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    pub proxy_enabled: bool,
    pub proxy_url: String,
    pub custom_instructions: String,
    pub credential_ref: Option<CredentialRef>,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            version: 1,
            provider: "claude".to_string(),
            endpoint: String::new(),
            model: String::new(),
            models: Vec::new(),
            api_style: String::new(),
            auth_method: String::new(),
            enable_thinking: true,
            context_window: None,
            max_retries: 2,
            proxy_enabled: false,
            proxy_url: String::new(),
            custom_instructions: String::new(),
            credential_ref: None,
        }
    }
}

impl AiSettings {
    /// Settings seeded with the endpoint/model/api_style/auth_method defaults
    /// for the given provider; unknown providers keep empty values.
    pub fn default_for_provider(provider: &str) -> Self {
        let mut settings = Self {
            provider: provider.to_string(),
            ..Self::default()
        };
        if let Some(preset) = super::client::provider_presets()
            .iter()
            .find(|preset| preset.id == provider)
        {
            settings.endpoint = preset.default_endpoint.to_owned();
            settings.model = preset.default_model.to_owned();
            settings.api_style = serde_json::to_value(preset.default_api_style)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            settings.auth_method = serde_json::to_value(preset.default_auth_method)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
        }
        settings
    }

    /// Load settings from `{data_dir}/ai-settings.json`; missing or invalid
    /// files fall back to defaults.
    pub fn load(data_dir: &std::path::Path) -> Self {
        let path = data_dir.join(SETTINGS_FILE);
        let Ok(bytes) = fs::read(&path) else {
            return Self::default();
        };
        if bytes.is_empty() {
            return Self::default();
        }
        match serde_json::from_slice(&bytes) {
            Ok(settings) => settings,
            Err(error) => {
                tracing::warn!(path = %path.display(), "failed to parse AI settings: {error}");
                Self::default()
            }
        }
    }

    /// Atomically write settings to `{data_dir}/ai-settings.json`.
    pub fn save(&self, data_dir: &std::path::Path) -> Result<(), String> {
        fs::create_dir_all(data_dir)
            .map_err(|error| format!("failed to create data directory: {error}"))?;
        let bytes = serde_json::to_string_pretty(self)
            .map_err(|error| format!("failed to encode AI settings: {error}"))?;
        db_common::atomic_file::replace(&data_dir.join(SETTINGS_FILE), bytes.as_bytes())
            .map_err(|error| format!("failed to save AI settings: {error}"))
    }
}
