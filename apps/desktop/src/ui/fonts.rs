use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};

pub fn install_global_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    let mut installed = 0;

    fonts.font_data.insert(
        "orvyn-inter".to_string(),
        FontData::from_static(include_bytes!("../../fonts/Inter-Regular.ttf")),
    );
    fonts.font_data.insert(
        "orvyn-cjk".to_string(),
        FontData::from_static(include_bytes!("../../fonts/NotoSansCJKsc-Regular.otf")),
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
        ["orvyn-mono", "orvyn-cjk"]
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
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
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
