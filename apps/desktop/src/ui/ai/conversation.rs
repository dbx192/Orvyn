//! AI chat conversation persistence (`ai-conversations.json`).

use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::fs;

pub const CONVERSATIONS_FILE: &str = "ai-conversations.json";
pub const MAX_CONVERSATIONS: usize = 50;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiChatMessage {
    /// "user" | "assistant"
    pub role: String,
    pub content: String,
    /// Milliseconds since the Unix epoch.
    pub created_at: u64,
    /// Serde value of the action that produced this message, if any.
    pub action: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiConversation {
    /// UUID v4.
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub messages: Vec<AiChatMessage>,
}

impl AiConversation {
    pub fn new(first_user_text: &str) -> Self {
        let trimmed = first_user_text.trim();
        let title = if trimmed.is_empty() {
            "New Chat".to_string()
        } else {
            trimmed.chars().take(100).collect()
        };
        let now = now_ms();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            created_at: now,
            updated_at: now,
            messages: Vec::new(),
        }
    }
}

/// Current UTC time in milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConversationFile {
    #[serde(default)]
    conversations: Vec<AiConversation>,
}

/// Load conversations from `{data_dir}/ai-conversations.json`, most recently
/// updated first; missing or invalid files yield an empty list.
pub fn load_conversations(data_dir: &std::path::Path) -> Vec<AiConversation> {
    let path = data_dir.join(CONVERSATIONS_FILE);
    let Ok(bytes) = fs::read(&path) else {
        return Vec::new();
    };
    if bytes.is_empty() {
        return Vec::new();
    }
    let file: ConversationFile = match serde_json::from_slice(&bytes) {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(path = %path.display(), "failed to parse AI conversations: {error}");
            return Vec::new();
        }
    };
    let mut conversations = file.conversations;
    conversations.sort_by_key(|conversation| Reverse(conversation.updated_at));
    conversations
}

/// Atomically write conversations to `{data_dir}/ai-conversations.json`,
/// keeping at most `MAX_CONVERSATIONS` entries (most recently updated first).
pub fn save_conversations(
    data_dir: &std::path::Path,
    convs: &[AiConversation],
) -> Result<(), String> {
    let mut conversations = convs.to_vec();
    conversations.sort_by_key(|conversation| Reverse(conversation.updated_at));
    conversations.truncate(MAX_CONVERSATIONS);
    fs::create_dir_all(data_dir)
        .map_err(|error| format!("failed to create data directory: {error}"))?;
    let file = ConversationFile { conversations };
    let bytes = serde_json::to_string_pretty(&file)
        .map_err(|error| format!("failed to encode AI conversations: {error}"))?;
    db_common::atomic_file::replace(&data_dir.join(CONVERSATIONS_FILE), bytes.as_bytes())
        .map_err(|error| format!("failed to save AI conversations: {error}"))
}
