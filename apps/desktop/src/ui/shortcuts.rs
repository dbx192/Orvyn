use eframe::egui::{InputState, Key, Modifiers};

/// Represents a keyboard shortcut with optional modifiers and a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Shortcut {
    pub const fn new(key: Key) -> Self {
        Self {
            modifiers: Modifiers::NONE,
            key,
        }
    }

    pub const fn with_ctrl(self) -> Self {
        Self {
            modifiers: Modifiers::CTRL,
            ..self
        }
    }

    pub const fn with_shift(self) -> Self {
        Self {
            modifiers: Modifiers::SHIFT,
            ..self
        }
    }

    pub const fn with_alt(self) -> Self {
        Self {
            modifiers: Modifiers::ALT,
            ..self
        }
    }

    pub const fn with_cmd(self) -> Self {
        Self {
            modifiers: Modifiers::COMMAND,
            ..self
        }
    }

    pub fn matches(&self, input: &InputState) -> bool {
        if input.modifiers == self.modifiers || input.modifiers.matches_exact(self.modifiers) {
            if input.key_pressed(self.key) {
                return true;
            }
        }
        false
    }
}

/// Returns platform-appropriate modifier (Ctrl on Windows/Linux, Cmd on macOS).
fn platform_modifier() -> Modifiers {
    #[cfg(target_os = "macos")]
    {
        Modifiers::COMMAND
    }
    #[cfg(not(target_os = "macos"))]
    {
        Modifiers::CTRL
    }
}

/// Returns "Ctrl" on Windows/Linux, "Cmd" on macOS for display.
fn platform_modifier_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Cmd"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Ctrl"
    }
}

/// Application-wide keyboard shortcuts.
#[derive(Debug, Clone)]
pub enum AppShortcut {
    NewQuery,
    ExecuteCurrent,
    ExecuteSelection,
    Save,
    Refresh,
    Search,
    FocusObjectSearch,
    ClosePopup,
    ToggleTheme,
    FormatSql,
}

impl AppShortcut {
    /// Returns the shortcut combination for this action.
    pub fn shortcut(&self) -> Shortcut {
        match self {
            AppShortcut::NewQuery => Shortcut::new(Key::N),
            AppShortcut::ExecuteCurrent => Shortcut::new(Key::Enter),
            AppShortcut::ExecuteSelection => Shortcut::new(Key::Enter),
            AppShortcut::Save => Shortcut::new(Key::S),
            AppShortcut::Refresh => Shortcut::new(Key::R),
            AppShortcut::Search => Shortcut::new(Key::F),
            AppShortcut::FocusObjectSearch => Shortcut::new(Key::L),
            AppShortcut::ClosePopup => Shortcut::new(Key::Escape),
            AppShortcut::ToggleTheme => Shortcut::new(Key::T),
            AppShortcut::FormatSql => Shortcut::new(Key::F),
        }
    }

    /// Returns the shortcut with platform-appropriate modifier (Ctrl/Cmd).
    pub fn with_platform_modifier(&self) -> Shortcut {
        let mut s = self.shortcut();
        s.modifiers = platform_modifier();
        s
    }

    /// Returns the display string for this shortcut (e.g., "Ctrl+N", "Cmd+Enter").
    pub fn display_string(&self) -> String {
        let modifier = platform_modifier_name();
        match self {
            AppShortcut::NewQuery => format!("{}+N", modifier),
            AppShortcut::ExecuteCurrent => format!("{}+Enter", modifier),
            AppShortcut::ExecuteSelection => format!("{}+Shift+Enter", modifier),
            AppShortcut::Save => format!("{}+S", modifier),
            AppShortcut::Refresh => format!("{}+R", modifier),
            AppShortcut::Search => format!("{}+F", modifier),
            AppShortcut::FocusObjectSearch => format!("{}+L", modifier),
            AppShortcut::ClosePopup => "Esc".to_string(),
            AppShortcut::ToggleTheme => format!("{}+T", modifier),
            AppShortcut::FormatSql => format!("{}+Shift+F", modifier),
        }
    }
}

/// Checks if the input state matches the given shortcut.
pub fn matches_shortcut(input: &InputState, shortcut: &Shortcut) -> bool {
    shortcut.matches(input)
}
