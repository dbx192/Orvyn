use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};

pub fn install_cjk_fonts(ctx: &Context) {
    let Some((name, bytes)) = load_first_existing_font() else {
        tracing::warn!("no CJK font found; Chinese text may render as missing glyph boxes");
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(name.clone(), FontData::from_owned(bytes));

    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        if let Some(fallbacks) = fonts.families.get_mut(&family) {
            if !fallbacks.iter().any(|font| font == &name) {
                fallbacks.push(name.clone());
            }
        }
    }

    ctx.set_fonts(fonts);
}

fn load_first_existing_font() -> Option<(String, Vec<u8>)> {
    for path in cjk_font_candidates() {
        if let Ok(bytes) = std::fs::read(&path) {
            if !bytes.is_empty() {
                return Some((font_name(&path), bytes));
            }
        }
    }
    None
}

fn cjk_font_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(windows_dir) = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot"))
    {
        let fonts_dir = PathBuf::from(windows_dir).join("Fonts");
        paths.extend(windows_font_candidates(&fonts_dir));
    }

    paths.extend(windows_font_candidates(Path::new(r"C:\Windows\Fonts")));
    paths.extend(windows_font_candidates(Path::new("/mnt/c/Windows/Fonts")));

    paths.extend([
        PathBuf::from("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc"),
        PathBuf::from("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc"),
        PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
        PathBuf::from("/usr/share/fonts/opentype/source-han-sans/SourceHanSansSC-Regular.otf"),
    ]);

    paths
}

fn windows_font_candidates(fonts_dir: &Path) -> Vec<PathBuf> {
    ["simhei.ttf", "Deng.ttf", "msyh.ttc", "simsun.ttc"]
        .into_iter()
        .map(|font| fonts_dir.join(font))
        .collect()
}

fn font_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| format!("cjk-{stem}"))
        .unwrap_or_else(|| "cjk-fallback".to_string())
}
