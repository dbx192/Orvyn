use eframe::egui::{
    pos2, vec2, Align, Color32, CursorIcon, Id, Label, Layout, Rect, Response, RichText, Sense,
    Stroke, TextEdit, Ui, Vec2,
};

pub const CELL_PADDING_X: f32 = 8.0;
const TEXT_UNIT_WIDTH: f32 = 7.4;

pub struct GridCellResponse {
    pub response: Response,
    pub rect: Rect,
}

pub struct GridHeaderResponse {
    pub response: Response,
    pub resizing: bool,
}

pub fn estimate_text_width(text: &str) -> f32 {
    (display_width_units(text) * TEXT_UNIT_WIDTH + CELL_PADDING_X * 2.0 + 2.0).ceil()
}

pub fn grid_stroke(ui: &Ui) -> Stroke {
    if ui.visuals().dark_mode {
        Stroke::new(1.0, Color32::from_rgb(48, 55, 64))
    } else {
        Stroke::new(1.0, Color32::from_rgb(214, 220, 228))
    }
}

pub fn header_fill(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(38, 43, 50)
    } else {
        Color32::from_rgb(244, 247, 250)
    }
}

pub fn row_fill(ui: &Ui, row: usize, selected: bool) -> Color32 {
    if selected {
        if ui.visuals().dark_mode {
            Color32::from_rgb(34, 70, 110)
        } else {
            Color32::from_rgb(218, 235, 255)
        }
    } else if ui.visuals().dark_mode {
        if row % 2 == 0 {
            Color32::from_rgb(17, 20, 25)
        } else {
            Color32::from_rgb(21, 25, 31)
        }
    } else if row % 2 == 0 {
        Color32::from_rgb(255, 255, 255)
    } else {
        Color32::from_rgb(249, 251, 253)
    }
}

pub fn label_cell(
    ui: &mut Ui,
    width: f32,
    height: f32,
    text: RichText,
    sense: Sense,
    fill: Color32,
    stroke: Stroke,
) -> GridCellResponse {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    paint_cell_frame(ui, rect, fill, stroke);
    paint_label(ui, rect, height, text);
    GridCellResponse { response, rect }
}

pub fn text_edit_cell(
    ui: &mut Ui,
    width: f32,
    height: f32,
    value: &mut String,
    fill: Color32,
    stroke: Stroke,
) -> GridCellResponse {
    let (rect, background_response) =
        ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    paint_cell_frame(ui, rect, fill, stroke);

    let inner_rect = rect.shrink2(vec2(CELL_PADDING_X, 2.0));
    let mut child_ui = ui.child_ui(inner_rect, Layout::left_to_right(Align::Center));
    child_ui.set_clip_rect(inner_rect);
    child_ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

    let edit_response = child_ui.add_sized(
        [inner_rect.width(), (height - 4.0).max(18.0)],
        TextEdit::singleline(value)
            .desired_width(inner_rect.width())
            .frame(false),
    );
    if edit_response.has_focus() {
        paint_cell_outline(
            ui,
            rect,
            Stroke::new(1.5, ui.visuals().selection.stroke.color),
        );
    }

    GridCellResponse {
        response: edit_response.union(background_response),
        rect,
    }
}

pub fn resizable_header_cell(
    ui: &mut Ui,
    width: &mut f32,
    height: f32,
    text: RichText,
    hover_text: String,
    id: Id,
    min_width: f32,
    max_width: f32,
) -> GridHeaderResponse {
    let current_width = *width;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(current_width, height), Sense::click());
    paint_cell_frame(ui, rect, header_fill(ui), grid_stroke(ui));
    paint_label(ui, rect, height, text);

    let handle_rect = Rect::from_min_max(pos2(rect.max.x - 5.0, rect.min.y), rect.max);
    let handle_response = ui.interact(handle_rect, id, Sense::drag());
    if handle_response.hovered() || handle_response.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        ui.painter().line_segment(
            [
                pos2(rect.max.x - 1.0, rect.min.y),
                pos2(rect.max.x - 1.0, rect.max.y),
            ],
            Stroke::new(2.0, ui.visuals().selection.stroke.color),
        );
    }
    if handle_response.dragged() {
        let delta = ui.input(|input| input.pointer.delta().x);
        *width = (*width + delta).clamp(min_width, max_width);
    }

    let response = response.on_hover_text(hover_text);

    GridHeaderResponse {
        response,
        resizing: handle_response.hovered() || handle_response.dragged(),
    }
}

pub fn paint_cell_outline(ui: &Ui, rect: Rect, stroke: Stroke) {
    ui.painter().rect_stroke(rect.shrink(0.5), 0.0, stroke);
}

pub fn paint_cell_frame(ui: &Ui, rect: Rect, fill: Color32, stroke: Stroke) {
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter().rect_stroke(rect.shrink(0.5), 0.0, stroke);
}

fn paint_label(ui: &mut Ui, rect: Rect, height: f32, text: RichText) {
    let inner_rect = rect.shrink2(vec2(CELL_PADDING_X, 0.0));
    let mut child_ui = ui.child_ui(inner_rect, Layout::left_to_right(Align::Center));
    child_ui.set_clip_rect(inner_rect);
    child_ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    child_ui.add_sized(
        [inner_rect.width(), height],
        Label::new(text).truncate(true),
    );
}

fn display_width_units(text: &str) -> f32 {
    text.chars().map(char_width_units).sum()
}

fn char_width_units(ch: char) -> f32 {
    if ch == '\t' {
        return 4.0;
    }
    if ch.is_control() {
        return 1.0;
    }
    if ch.is_ascii() {
        return 1.0;
    }
    if is_wide_char(ch) {
        return 2.0;
    }
    1.4
}

fn is_wide_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x115f
            | 0x2329..=0x232a
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f300..=0x1faff
            | 0x20000..=0x3fffd
    )
}
