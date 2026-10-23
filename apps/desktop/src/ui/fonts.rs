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
    // metrics, so a dedicated instance is tuned per anchor font.
    fonts.font_data.insert(
        "orvyn-cjk".to_string(),
        FontData::from_static(include_bytes!("../../fonts/NotoSansCJKsc-Regular.otf"))
            .tweak(cjk_tweak(1.209_961, 0.968_750)),
    );
    fonts.font_data.insert(
        "orvyn-cjk-mono".to_string(),
        FontData::from_static(include_bytes!("../../fonts/NotoSansCJKsc-Regular.otf"))
            .tweak(cjk_tweak(1.320_000, 1.020_000)),
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

/// egui 0.27 pre-scales every font by its hhea line box (its fix for
/// emilk/egui#2068) but takes the row height from the family's *first* font
/// while placing glyphs on a baseline derived from their own font's inflated
/// ascent. Noto Sans CJK's hhea line box is 1.448em versus Inter's 1.210em /
/// JetBrains Mono's 1.320em, so CJK glyphs drew ~20% larger and their baseline
/// landed near the bottom edge of every widget (the "text sits too low" bug).
///
/// The returned tweak renormalizes the CJK font against the primary Latin
/// font of the family it joins. `latin_line` / `latin_asc` are the primary's
/// hhea line height and ascender in em (Inter 2478/2048 = 1.20996, 0.96875;
/// JetBrains Mono 1320/1000 = 1.32, 1.02; CJK 1448/1000, 1.16):
///   scale                  = latin_line / cjk_line  (equal em boxes)
///   baseline_offset_factor = (latin_line * latin_asc - cjk_line * cjk_asc)
///                            / latin_line           (equal baselines)
///   y_offset_factor        = (1 - scale) * 0.5 * (cjk_asc + cjk_desc) / scale
///                            (cancels egui's built-in scale recentering)
fn cjk_tweak(latin_line: f32, latin_asc: f32) -> FontTweak {
    const CJK_LINE: f32 = 1.448; // (1160 - -288) / upem(1000)
    const CJK_ASC: f32 = 1.160; // 1160 / 1000
    const CJK_ASC_DESC: f32 = 0.872; // (1160 - 288) / 1000
    let scale = latin_line / CJK_LINE;
    FontTweak {
        scale,
        baseline_offset_factor: (latin_line * latin_asc - CJK_LINE * CJK_ASC) / latin_line,
        y_offset_factor: (1.0 - scale) * 0.5 * CJK_ASC_DESC / scale,
        y_offset: 0.0,
    }
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
}
