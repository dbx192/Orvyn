use eframe::egui::{
    epaint::TextShape, pos2, vec2, Align, Color32, CursorIcon, Id, Layout, Rect, Response,
    RichText, Sense, Stroke, TextEdit, TextStyle, Ui, Vec2, WidgetText,
};

// Keep enough room for the 14px body font and a visible breathing space
// between adjacent rows. These dimensions are shared by the result grid
// and the table editor, so editing controls remain aligned with the cells.
pub const CELL_PADDING_X: f32 = 6.0;
pub const TABLE_ROW_HEIGHT: f32 = 23.0;
pub const TABLE_HEADER_HEIGHT: f32 = 30.0;
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
    (display_width_units(text) * TEXT_UNIT_WIDTH + 2.0).ceil()
}

pub fn grid_stroke(ui: &Ui) -> Stroke {
    if ui.visuals().dark_mode {
        Stroke::new(1.0, Color32::from_rgb(54, 56, 61))
    } else {
        Stroke::new(1.0, Color32::from_rgb(225, 227, 232))
    }
}

pub fn header_fill(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(31, 33, 36)
    } else {
        Color32::from_rgb(246, 247, 249)
    }
}

pub fn row_fill(ui: &Ui, row: usize, selected: bool) -> Color32 {
    if selected {
        if ui.visuals().dark_mode {
            Color32::from_rgb(45, 49, 56)
        } else {
            Color32::from_rgb(227, 232, 240)
        }
    } else if ui.visuals().dark_mode {
        if row.is_multiple_of(2) {
            Color32::from_rgb(19, 20, 23)
        } else {
            Color32::from_rgb(23, 24, 27)
        }
    } else if row.is_multiple_of(2) {
        Color32::from_rgb(255, 255, 255)
    } else {
        Color32::from_rgb(248, 249, 251)
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
    id: Id,
    width: f32,
    height: f32,
    value: &mut String,
    fill: Color32,
    stroke: Stroke,
) -> GridCellResponse {
    let (rect, background_response) =
        ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    paint_cell_frame(ui, rect, fill, stroke);

    let inner_rect = Rect::from_min_max(
        pos2(rect.left() + CELL_PADDING_X, rect.top()),
        pos2(rect.right() - CELL_PADDING_X, rect.bottom()),
    );
    let mut child_ui = ui.child_ui(inner_rect, Layout::left_to_right(Align::Center));
    child_ui.set_clip_rect(inner_rect.intersect(ui.clip_rect()));
    child_ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    child_ui.spacing_mut().interact_size = vec2(0.0, height);

    let edit_response = child_ui.add_sized(
        [inner_rect.width(), height],
        TextEdit::singleline(value)
            .id(id)
            .desired_width(inner_rect.width())
            // egui's TextEdit defaults to LEFT_TOP; center the text like the
            // label cells so edited rows do not sit at the top edge.
            .vertical_align(Align::Center)
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

#[allow(clippy::too_many_arguments)]
pub fn resizable_header_cell(
    ui: &mut Ui,
    width: &mut f32,
    height: f32,
    text: RichText,
    _hover_text: String,
    id: Id,
    min_width: f32,
    max_width: f32,
) -> GridHeaderResponse {
    let current_width = *width;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(current_width, height), Sense::click());
    paint_cell_frame(ui, rect, header_fill(ui), grid_stroke(ui));
    // Keep the sticky header visually in front of the scrolling body. The
    // stronger lower rule remains visible even when the first row is flush
    // against the header boundary.
    ui.painter().line_segment(
        [
            pos2(rect.left(), rect.bottom() - 1.0),
            pos2(rect.right(), rect.bottom() - 1.0),
        ],
        Stroke::new(
            1.5,
            if ui.visuals().dark_mode {
                Color32::from_rgb(82, 86, 94)
            } else {
                Color32::from_rgb(168, 172, 180)
            },
        ),
    );
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

/// Compact flat horizontal scrollbar used below wide data grids.
pub fn flat_scrollbar(ui: &mut Ui, value: &mut f32, max: f32, width: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 12.0), Sense::click_and_drag());
    let track = rect.shrink2(vec2(0.0, 4.0));
    ui.painter()
        .rect_filled(track, 0.0, ui.visuals().widgets.noninteractive.bg_fill);
    let thumb_width = 42.0_f32.min(rect.width().max(1.0));
    let travel = (rect.width() - thumb_width).max(0.0);
    let fraction = if max > 0.0 { (*value / max).clamp(0.0, 1.0) } else { 0.0 };
    let thumb = Rect::from_min_size(
        pos2(rect.left() + travel * fraction, rect.top()),
        vec2(thumb_width, rect.height()),
    );
    ui.painter().rect_filled(
        thumb,
        0.0,
        if response.dragged() || response.hovered() {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().widgets.inactive.bg_fill
        },
    );
    if (response.dragged() || response.clicked()) && max > 0.0 {
        if let Some(pointer) = ui.ctx().pointer_hover_pos() {
            *value = (((pointer.x - rect.left() - thumb_width * 0.5) / travel.max(1.0))
                .clamp(0.0, 1.0))
                * max;
        }
    }
    response
}

fn paint_label(ui: &mut Ui, rect: Rect, height: f32, text: RichText) {
    // A small vertical inset prevents glyphs from touching the cell borders
    // when the row is scrolled or rendered at fractional scale factors.
    let inner_rect = rect.shrink2(vec2(CELL_PADDING_X, 2.0));
    let galley =
        WidgetText::from(text).into_galley(ui, Some(false), inner_rect.width(), TextStyle::Body);
    let text_pos = pos2(
        inner_rect.left(),
        rect.center().y - (galley.size().y * 0.5).min(height * 0.5),
    );
    ui.painter().with_clip_rect(inner_rect.intersect(ui.clip_rect())).add(TextShape::new(
        text_pos,
        galley,
        ui.visuals().text_color(),
    ));
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
