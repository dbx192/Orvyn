use crate::ui::state::AppTheme;
use eframe::egui::{vec2, Color32, Context, Margin, Rounding, Stroke, Visuals};

pub(super) fn apply_app_style(ctx: &Context, theme: AppTheme) {
    let mut visuals = match theme {
        AppTheme::Light => Visuals::light(),
        AppTheme::Dark => Visuals::dark(),
    };

    match theme {
        AppTheme::Light => {
            visuals.panel_fill = Color32::from_rgb(246, 247, 249);
            visuals.extreme_bg_color = Color32::from_rgb(238, 240, 244);
            visuals.faint_bg_color = Color32::from_rgb(232, 236, 241);
            visuals.widgets.noninteractive.bg_stroke =
                Stroke::new(1.0, Color32::from_rgb(210, 216, 224));
            // Keep selected tabs readable without overpowering the workspace.
            visuals.selection.bg_fill = Color32::from_rgb(75, 137, 194);
            visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(48, 99, 151));
        }
        AppTheme::Dark => {
            visuals.panel_fill = Color32::from_rgb(20, 23, 28);
            visuals.extreme_bg_color = Color32::from_rgb(13, 15, 19);
            visuals.faint_bg_color = Color32::from_rgb(31, 36, 43);
            visuals.widgets.noninteractive.bg_stroke =
                Stroke::new(1.0, Color32::from_rgb(50, 58, 68));
            visuals.selection.bg_fill = Color32::from_rgb(58, 125, 184);
            visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(105, 165, 220));
        }
    }

    visuals.window_rounding = Rounding::same(6.0);
    visuals.menu_rounding = Rounding::same(6.0);
    visuals.widgets.noninteractive.rounding = Rounding::same(4.0);
    visuals.widgets.inactive.rounding = Rounding::same(4.0);
    visuals.widgets.hovered.rounding = Rounding::same(4.0);
    visuals.widgets.active.rounding = Rounding::same(4.0);
    visuals.widgets.open.rounding = Rounding::same(4.0);

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(8.0, 3.0);
    style.spacing.interact_size = vec2(44.0, 24.0);
    style.spacing.window_margin = Margin::symmetric(10.0, 8.0);
    ctx.set_style(style);
}
