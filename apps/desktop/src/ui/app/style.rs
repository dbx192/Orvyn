use crate::ui::state::AppTheme;
use eframe::egui::{
    vec2, Color32, Context, FontFamily, FontId, Margin, Rounding, Stroke, TextStyle, Visuals,
};

/// dbx-inspired semantic palette. Kept here so the shell, tree and grids can
/// share one source of truth instead of hard-coding their own RGB values.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Main window / content background.
    pub bg: Color32,
    /// Deepest background (grid canvas, code gutters).
    pub bg_deep: Color32,
    /// Raised surfaces (cards, toolbars, sidebar header).
    pub surface: Color32,
    /// Primary text.
    pub fg: Color32,
    /// Secondary / muted text.
    pub fg_muted: Color32,
    /// Hairline border color.
    pub border: Color32,
    /// Row hover / subtle selected surface.
    pub hover: Color32,
    /// Selected row / focused item accent.
    pub accent: Color32,
    pub accent_fg: Color32,
    /// Accent framing for focused controls / resizers.
    pub accent_border: Color32,
    /// Semantic colors (kept small, used sparingly).
    pub danger: Color32,
    pub info: Color32,
}

impl Palette {
    pub fn for_theme(theme: AppTheme) -> Self {
        match theme {
            AppTheme::Dark => Self {
                bg: Color32::from_rgb(17, 18, 20),
                bg_deep: Color32::from_rgb(13, 14, 16),
                surface: Color32::from_rgb(24, 25, 28),
                fg: Color32::from_rgb(214, 215, 219),
                fg_muted: Color32::from_rgb(146, 147, 152),
                border: Color32::from_rgb(50, 52, 57),
                hover: Color32::from_rgb(46, 47, 51),
                accent: Color32::from_rgb(47, 51, 58),
                accent_fg: Color32::from_rgb(221, 222, 226),
                accent_border: Color32::from_rgb(131, 138, 151),
                danger: Color32::from_rgb(243, 98, 95),
                info: Color32::from_rgb(96, 165, 250),
            },
            AppTheme::Light => Self {
                bg: Color32::from_rgb(250, 250, 250),
                bg_deep: Color32::from_rgb(244, 245, 247),
                surface: Color32::from_rgb(255, 255, 255),
                fg: Color32::from_rgb(18, 18, 18),
                fg_muted: Color32::from_rgb(112, 113, 115),
                border: Color32::from_rgb(229, 229, 229),
                hover: Color32::from_rgb(245, 245, 245),
                accent: Color32::from_rgb(232, 233, 236),
                accent_fg: Color32::from_rgb(24, 24, 24),
                accent_border: Color32::from_rgb(125, 133, 146),
                danger: Color32::from_rgb(220, 38, 38),
                info: Color32::from_rgb(37, 99, 235),
            },
        }
    }
}

pub(super) fn apply_app_style(ctx: &Context, theme: AppTheme) {
    let palette = Palette::for_theme(theme);
    let mut visuals = match theme {
        AppTheme::Light => Visuals::light(),
        AppTheme::Dark => Visuals::dark(),
    };

    // ---- Surfaces ----
    visuals.panel_fill = palette.bg;
    visuals.window_fill = palette.surface;
    visuals.extreme_bg_color = palette.bg_deep;
    visuals.faint_bg_color = palette.surface;
    visuals.code_bg_color = palette.bg_deep;

    // ---- Text ----
    visuals.override_text_color = Some(palette.fg);
    visuals.hyperlink_color = palette.info;

    // ---- Widgets: subtle borders + quiet hover, matching dbx ----
    visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
    visuals.widgets.noninteractive.weak_bg_fill = palette.surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.fg_muted);

    visuals.widgets.inactive.bg_fill = palette.surface;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.fg);

    visuals.widgets.hovered.bg_fill = palette.hover;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, palette.accent_fg);

    visuals.widgets.active.bg_fill = palette.accent;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette.accent_border);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, palette.accent_fg);

    visuals.widgets.open.bg_fill = palette.accent;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, palette.accent_border);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, palette.accent_fg);

    // ---- Selection: quiet, refusable, professional ----
    visuals.selection.bg_fill = palette.accent;
    visuals.selection.stroke = Stroke::new(1.0, palette.accent_border);

    // ---- Geometry ----
    let radius = Rounding::same(6.0);
    visuals.window_rounding = radius;
    visuals.menu_rounding = radius;
    visuals.widgets.noninteractive.rounding = Rounding::same(4.0);
    visuals.widgets.inactive.rounding = Rounding::same(4.0);
    visuals.widgets.hovered.rounding = Rounding::same(4.0);
    visuals.widgets.active.rounding = Rounding::same(4.0);
    visuals.widgets.open.rounding = Rounding::same(4.0);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    for text_style in [
        TextStyle::Small,
        TextStyle::Body,
        TextStyle::Button,
        TextStyle::Heading,
    ] {
        if let Some(font_id) = style.text_styles.get_mut(&text_style) {
            font_id.family = FontFamily::Name("orvyn-inter".into());
        }
    }
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(14.0, FontFamily::Name("orvyn-mono".into())),
    );
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(10.0, 5.0);
    style.spacing.interact_size = vec2(44.0, 28.0);
    style.spacing.window_margin = Margin::symmetric(10.0, 8.0);
    ctx.set_style(style);
}

/// Convenience accessor for modules that paint custom chrome (toolbars,
/// sidebars, status bars) and want the active palette without re-deriving it.
pub fn current_palette(ctx: &Context) -> Palette {
    Palette::for_theme(if ctx.style().visuals.dark_mode {
        AppTheme::Dark
    } else {
        AppTheme::Light
    })
}
