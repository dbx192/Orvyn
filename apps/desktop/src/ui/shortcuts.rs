use eframe::egui::{InputState, Modifiers};

/// Represents a keyboard shortcut with optional modifiers and a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    pub key: egui::Key,
}

impl Shortcut {
    pub const fn new(key: egui::Key) -> Self {
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
        if input.modifiers == self.modifiers || input.modifiers.matches(self.modifiers) {
            if let Some(pressed_key) = input.key_pressed {
                return pressed_key == self.key;
            }
        }
        false
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
    /// Uses Ctrl on Windows/Linux, Cmd on macOS.
    pub fn shortcut(&self) -> Shortcut {
        match self {
            AppShortcut::NewQuery => Shortcut::new(egui::Key::N),
            AppShortcut::ExecuteCurrent => Shortcut::new(egui::Key::Enter),
            AppShortcut::ExecuteSelection => Shortcut::new(egui::Key::Enter),
            AppShortcut::Save => Shortcut::new(egui::Key::S),
            AppShortcut::Refresh => Shortcut::new(egui::Key::R),
            AppShortcut::Search => Shortcut::new(egui::Key::F),
            AppShortcut::FocusObjectSearch => Shortcut::new(egui::Key::L),
            AppShortcut::ClosePopup => Shortcut::new(egui::Key::Escape),
            AppShortcut::ToggleTheme => Shortcut::new(egui::Key::T),
            AppShortcut::FormatSql => Shortcut::new(egui::Key::F),
        }
    }

    /// Returns the shortcut with platform-appropriate modifier (Ctrl/Cmd).
    pub fn with_platform_modifier(self) -> Shortcut {
        #[cfg(target_os = "macos")]
        {
            use egui::ModifierNames;
            let cmd_mod = Modifiers {
                command: true,
                ..Modifiers::NONE
            };
            let mut s = self.shortcut();
            s.modifiers = cmd_mod;
            s
        }
        #[cfg(not(target_os = "macos"))]
        {
            use egui::ModifierNames;
            let ctrl_mod = Modifiers {
                ctrl: true,
                ..Modifiers::NONE
            };
            let mut s = self.shortcut();
            s.modifiers = ctrl_mod;
            s
        }
    }

    /// Returns the display string for this shortcut (e.g., "Ctrl+N", "Cmd+Enter").
    pub fn display_string(&self) -> String {
        #[cfg(target_os = "macos")]
        let modifier = "Cmd";
        #[cfg(not(target_os = "macos"))]
        let modifier = "Ctrl";

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
