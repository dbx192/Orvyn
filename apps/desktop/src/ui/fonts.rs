use eframe::egui::{
    Context, FontData, FontDefinitions, FontFamily, FontTweak,
};
use std::path::{Path, PathBuf};

pub fn install_global_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    let mut installed = 0;

    fonts.font_data.insert(
        "orvyn-inter".to_string(),
        FontData::from_static(include_bytes!("../../fonts/Inter-Regular.ttf")),
    );
    // Noto Sans CJK joins two families whose primaries have different line
    // metrics, so a dedicated instance is tuned per anchor font (hhea
    // ascender / descender / units-per-em from each primary's tables).
    fonts.font_data.insert(
        "orvyn-cjk".to_string(),
        FontData::from_static(include_bytes!("../../fonts/NotoSansCJKsc-Regular.otf"))
            .tweak(cjk_tweak(latin_ascent_per_size(1984.0, -494.0, 2048.0))),
    );
    fonts.font_data.insert(
        "orvyn-cjk-mono".to_string(),
        FontData::from_static(include_bytes!("../../fonts/NotoSansCJKsc-Regular.otf"))
            .tweak(cjk_tweak(latin_ascent_per_size(1020.0, -300.0, 1000.0))),
    );
    fonts.font_data.insert(
        "orvyn-mono".to_string(),
        FontData::from_static(include_bytes!("../../fonts/JetBrainsMono-Regular.ttf")),
    );

    let proportional_fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let monospace_fallbacks = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    fonts.families.insert(
        FontFamily::Name("orvyn-inter".into()),
        ["orvyn-inter", "orvyn-cjk"]
            .into_iter()
            .map(str::to_string)
            .chain(proportional_fallbacks)
            .collect(),
    );
    fonts.families.insert(
        FontFamily::Name("orvyn-mono".into()),
        ["orvyn-mono", "orvyn-cjk-mono"]
            .into_iter()
            .map(str::to_string)
            .chain(monospace_fallbacks)
            .collect(),
    );
    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.insert(0, "orvyn-inter".to_string());
        family.push("orvyn-cjk".to_string());
    }
    if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
        family.insert(0, "orvyn-mono".to_string());
        family.push("orvyn-cjk".to_string());
    }
    installed += 3;

    for path in global_font_candidates() {
        if let Ok(bytes) = std::fs::read(&path) {
            if bytes.is_empty() {
                continue;
            }
            let name = font_name(&path);
            fonts
                .font_data
                .insert(name.clone(), FontData::from_owned(bytes));
            for family in fallback_families() {
                if let Some(fallbacks) = fonts.families.get_mut(&family) {
                    if !fallbacks.contains(&name) {
                        fallbacks.push(name.clone());
                    }
                }
            }
            installed += 1;
        }
    }
    if installed == 0 {
        tracing::warn!("no global fallback fonts found; CJK or Devanagari may not render");
    } else {
        ctx.set_fonts(fonts);
    }
}

fn fallback_families() -> [FontFamily; 4] {
    [
        FontFamily::Proportional,
        FontFamily::Monospace,
        FontFamily::Name("orvyn-inter".into()),
        FontFamily::Name("orvyn-mono".into()),
    ]
}

/// Why a tweak is needed: egui 0.27 lays every glyph of a row on one shared
/// baseline equal to the *largest* per-font `ascent` field among the glyphs
/// in that row, where a font's `ascent` field is its own hhea metric
/// normalized to the requested size plus `FontTweak::baseline_offset_factor`
/// (`FontTweak::default()` applies -0.0333 — egui's Ubuntu correction — to
/// every font without an explicit tweak). Noto Sans CJK's hhea line box is
/// 1.448em (asc 1.16 / desc -0.288) versus Inter's 1.210em (asc 0.96875) and
/// JetBrains Mono's 1.320em (asc 1.02), so an untweaked CJK font's ascent
/// field lands ~3.9px above Inter's @14pt: pure-CJK rows ("运行当前的语句")
/// drew visibly higher than mixed rows ("格式化 SQL").
///
/// The tweak shifts the CJK ascent field onto the anchor Latin font's
/// baseline while keeping scale at 1.0 (egui's emilk/egui#2068 fix already
/// normalizes all em boxes, so shrinking would make CJK glyphs ~20% smaller):
///   ascent_field = size * cjk_line * (cjk_asc_frac + f)  (cjk_asc_frac = 1160/1448)
///   f            = latin_ascent_per_size / cjk_line - cjk_asc_frac
fn cjk_tweak(latin_ascent_per_size: f32) -> FontTweak {
    const CJK_LINE: f32 = 1.448; // (1160 - -288) / upem(1000)
    const CJK_ASC_FRAC: f32 = 1160.0 / 1448.0; // hhea ascender / line box
    FontTweak {
        scale: 1.0,
        baseline_offset_factor: latin_ascent_per_size / CJK_LINE - CJK_ASC_FRAC,
        y_offset_factor: 0.0,
        y_offset: 0.0,
    }
}

/// Baseline distance (the `ascent` field epaint stores in `FontImpl`) per
/// point of requested size for an *untweaked* font, replicating epaint
/// 0.27's `FontImpl::new` math:
///   ascent = (hhea_asc + hhea_line * default_baseline_offset_factor) / upem
fn latin_ascent_per_size(hhea_asc: f32, hhea_desc: f32, upem: f32) -> f32 {
    let line = hhea_asc - hhea_desc;
    (hhea_asc + line * FontTweak::default().baseline_offset_factor) / upem
}

fn global_font_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(windows_dir) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot"))
    {
        paths.extend(windows_font_candidates(
            &PathBuf::from(windows_dir).join("Fonts"),
        ));
    }
    paths.extend(windows_font_candidates(Path::new(r"C:\Windows\Fonts")));
    paths.extend(windows_font_candidates(Path::new("/mnt/c/Windows/Fonts")));
    paths.extend([
        PathBuf::from("/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/lohit-devanagari/Lohit-Devanagari.ttf"),
        PathBuf::from("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc"),
        PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/opentype/source-han-sans/SourceHanSansSC-Regular.otf"),
        PathBuf::from("/System/Library/Fonts/Kohinoor.ttc"),
        PathBuf::from("/System/Library/Fonts/PingFang.ttc"),
    ]);
    paths
}

fn windows_font_candidates(fonts_dir: &Path) -> Vec<PathBuf> {
    [
        "Nirmala.ttf",
        "NirmalaS.ttf",
        "mangal.ttf",
        "simhei.ttf",
        "Deng.ttf",
        "msyh.ttc",
        "simsun.ttc",
    ]
    .into_iter()
    .map(|font| fonts_dir.join(font))
    .collect()
}

fn font_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| format!("global-{stem}"))
        .unwrap_or_else(|| "global-fallback".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_fallbacks_cover_the_custom_ui_families() {
        let families = fallback_families();
        assert!(families.contains(&FontFamily::Name("orvyn-inter".into())));
        assert!(families.contains(&FontFamily::Name("orvyn-mono".into())));
    }

    /// Pure-CJK rows must sit on the same baseline as Latin rows: egui 0.27
    /// lays a row's glyphs on the largest per-font ascent field in the row,
    /// so the tweak has to equalize Noto Sans CJK's ascent field with the
    /// anchor font's. Run with: cargo test -p orvyn cjk_rows -- --nocapture
    #[test]
    fn cjk_rows_share_the_latin_baseline() {
        use eframe::egui::text::LayoutJob;
        use eframe::epaint::text::Fonts;

        let mut defs = FontDefinitions::default();
        defs.font_data.insert(
            "orvyn-inter".to_string(),
            FontData::from_static(include_bytes!("../../fonts/Inter-Regular.ttf")),
        );
        defs.font_data.insert(
            "orvyn-cjk".to_string(),
            FontData::from_static(include_bytes!("../../fonts/NotoSansCJKsc-Regular.otf"))
                .tweak(cjk_tweak(latin_ascent_per_size(1984.0, -494.0, 2048.0))),
        );
        defs.font_data.insert(
            "orvyn-mono".to_string(),
            FontData::from_static(include_bytes!("../../fonts/JetBrainsMono-Regular.ttf")),
        );
        defs.font_data.insert(
            "orvyn-cjk-mono".to_string(),
            FontData::from_static(include_bytes!("../../fonts/NotoSansCJKsc-Regular.otf"))
                .tweak(cjk_tweak(latin_ascent_per_size(1020.0, -300.0, 1000.0))),
        );
        defs.families.insert(
            FontFamily::Name("orvyn-inter".into()),
            vec!["orvyn-inter".into(), "orvyn-cjk".into()],
        );
        defs.families.insert(
            FontFamily::Name("orvyn-mono".into()),
            vec!["orvyn-mono".into(), "orvyn-cjk-mono".into()],
        );

        let fonts = Fonts::new(1.0, 4096, defs);
        let baseline = |family: &str, text: &str| {
            let job = LayoutJob::simple(
                text.to_string(),
                eframe::egui::FontId::new(14.0, FontFamily::Name(family.into())),
                eframe::egui::Color32::WHITE,
                f32::INFINITY,
            );
            fonts.layout_job(job).rows[0].glyphs[0].ascent
        };

        for (family, latin_text, cjk_text) in [
            ("orvyn-inter", "BEGIN", "运行当前的语句"),
            ("orvyn-mono", "SELECT 1;", "运行当前的语句"),
        ] {
            let latin_ascent = baseline(family, latin_text);
            let cjk_ascent = baseline(family, cjk_text);
            println!("{family}: latin={latin_ascent:.4} cjk={cjk_ascent:.4}");
            assert!(
                (cjk_ascent - latin_ascent).abs() < 0.05,
                "CJK baseline {cjk_ascent} diverges from Latin {latin_ascent}"
            );
        }
    }
}
