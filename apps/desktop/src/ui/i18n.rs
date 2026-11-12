use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock},
};

const EN_US: &str = include_str!("../../locales/en-US.json");
const ZH_CN: &str = include_str!("../../locales/zh-CN.json");
const HI_IN: &str = include_str!("../../locales/hi-IN.json");
const ES_ES: &str = include_str!("../../locales/es-ES.json");
const FR_FR: &str = include_str!("../../locales/fr-FR.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Locale {
    #[serde(rename = "en-US")]
    EnUs,
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "hi-IN")]
    HiIn,
    #[serde(rename = "es-ES")]
    EsEs,
    #[serde(rename = "fr-FR")]
    FrFr,
}

impl Locale {
    pub const ALL: [Self; 5] = [Self::EnUs, Self::ZhCn, Self::HiIn, Self::EsEs, Self::FrFr];

    pub const fn tag(self) -> &'static str {
        match self {
            Self::EnUs => "en-US",
            Self::ZhCn => "zh-CN",
            Self::HiIn => "hi-IN",
            Self::EsEs => "es-ES",
            Self::FrFr => "fr-FR",
        }
    }

    pub const fn native_name(self) -> &'static str {
        match self {
            Self::EnUs => "English",
            Self::ZhCn => "简体中文",
            Self::HiIn => "हिन्दी",
            Self::EsEs => "Español",
            Self::FrFr => "Français",
        }
    }

    pub fn from_language_tag(tag: &str) -> Option<Self> {
        let normalized = tag.trim().replace('_', "-").to_ascii_lowercase();
        match normalized.as_str() {
            "en" | "en-us" => Some(Self::EnUs),
            "zh" | "zh-cn" | "zh-hans" | "zh-sg" => Some(Self::ZhCn),
            "hi" | "hi-in" => Some(Self::HiIn),
            "es" | "es-es" => Some(Self::EsEs),
            "fr" | "fr-fr" => Some(Self::FrFr),
            _ if normalized.starts_with("en-") => Some(Self::EnUs),
            _ if normalized.starts_with("zh-") => Some(Self::ZhCn),
            _ if normalized.starts_with("hi-") => Some(Self::HiIn),
            _ if normalized.starts_with("es-") => Some(Self::EsEs),
            _ if normalized.starts_with("fr-") => Some(Self::FrFr),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct AppSettings {
    version: u32,
    locale: Locale,
}

struct Localizer {
    locale: Locale,
    catalogs: BTreeMap<Locale, BTreeMap<String, String>>,
    config_dir: PathBuf,
}

static LOCALIZER: OnceLock<RwLock<Localizer>> = OnceLock::new();

pub fn initialize(config_dir: &Path) {
    let mut catalogs = embedded_catalogs();
    apply_external_overrides(config_dir, &mut catalogs);
    let locale = load_saved_locale(config_dir)
        .or_else(|| {
            sys_locale::get_locale()
                .as_deref()
                .and_then(Locale::from_language_tag)
        })
        .unwrap_or(Locale::EnUs);
    let value = Localizer {
        locale,
        catalogs,
        config_dir: config_dir.to_path_buf(),
    };
    let _ = LOCALIZER.set(RwLock::new(value));
}

pub fn locale() -> Locale {
    LOCALIZER
        .get()
        .and_then(|value| value.read().ok().map(|value| value.locale))
        .unwrap_or(Locale::EnUs)
}

pub fn set_locale(locale: Locale) -> Result<(), String> {
    let Some(localizer) = LOCALIZER.get() else {
        return Err("localization is not initialized".to_string());
    };
    let config_dir = {
        let mut value = localizer.write().map_err(|_| "localization lock failed")?;
        value.locale = locale;
        value.config_dir.clone()
    };
    fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    let settings = AppSettings { version: 1, locale };
    let bytes = serde_json::to_vec_pretty(&settings).map_err(|error| error.to_string())?;
    db_common::atomic_file::replace(&config_dir.join("app-settings.json"), &bytes).map_err(|error| error.to_string())
}

pub fn text(key: &str) -> String {
    let Some(localizer) = LOCALIZER.get() else {
        return key.to_string();
    };
    let Ok(value) = localizer.read() else {
        return key.to_string();
    };
    lookup(&value, key).to_string()
}

pub fn format(key: &str, args: &[(&str, String)]) -> String {
    let mut value = text(key);
    for (name, replacement) in args {
        value = value.replace(&format!("{{{name}}}"), replacement);
    }
    value
}

fn lookup<'a>(localizer: &'a Localizer, key: &'a str) -> &'a str {
    localizer
        .catalogs
        .get(&localizer.locale)
        .and_then(|catalog| catalog.get(key))
        .or_else(|| {
            localizer
                .catalogs
                .get(&Locale::EnUs)
                .and_then(|catalog| catalog.get(key))
        })
        .map(String::as_str)
        .unwrap_or(key)
}

fn embedded_catalogs() -> BTreeMap<Locale, BTreeMap<String, String>> {
    [
        (Locale::EnUs, EN_US),
        (Locale::ZhCn, ZH_CN),
        (Locale::HiIn, HI_IN),
        (Locale::EsEs, ES_ES),
        (Locale::FrFr, FR_FR),
    ]
    .into_iter()
    .map(|(locale, source)| {
        let catalog = serde_json::from_str(source)
            .unwrap_or_else(|error| panic!("invalid embedded locale {}: {error}", locale.tag()));
        (locale, catalog)
    })
    .collect()
}

fn apply_external_overrides(
    config_dir: &Path,
    catalogs: &mut BTreeMap<Locale, BTreeMap<String, String>>,
) {
    let Some(english) = catalogs.get(&Locale::EnUs).cloned() else {
        return;
    };
    for locale in Locale::ALL {
        let path = config_dir
            .join("locales")
            .join(format!("{}.json", locale.tag()));
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(overrides) = serde_json::from_str::<BTreeMap<String, String>>(&source) else {
            tracing::warn!(path = %path.display(), "ignoring invalid locale override");
            continue;
        };
        let Some(catalog) = catalogs.get_mut(&locale) else {
            continue;
        };
        for (key, value) in overrides {
            let Some(reference) = english.get(&key) else {
                tracing::warn!(
                    locale = locale.tag(),
                    key,
                    "ignoring unknown translation key"
                );
                continue;
            };
            if value.trim().is_empty() || placeholders(reference) != placeholders(&value) {
                tracing::warn!(
                    locale = locale.tag(),
                    key,
                    "ignoring invalid translation override"
                );
                continue;
            }
            catalog.insert(key, value);
        }
    }
}

fn load_saved_locale(config_dir: &Path) -> Option<Locale> {
    let bytes = fs::read(config_dir.join("app-settings.json")).ok()?;
    let settings: AppSettings = serde_json::from_slice(&bytes).ok()?;
    (settings.version == 1).then_some(settings.locale)
}

fn placeholders(value: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut rest = value;
    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('}') else { break };
        let name = &rest[..end];
        if !name.is_empty()
            && name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            result.insert(name.to_string());
        }
        rest = &rest[end + 1..];
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalogs_have_identical_keys_and_placeholders() {
        let catalogs = embedded_catalogs();
        let english = &catalogs[&Locale::EnUs];
        for locale in Locale::ALL {
            let catalog = &catalogs[&locale];
            assert_eq!(
                catalog.keys().collect::<Vec<_>>(),
                english.keys().collect::<Vec<_>>()
            );
            for (key, reference) in english {
                assert!(!catalog[key].trim().is_empty(), "{}:{key}", locale.tag());
                assert_eq!(
                    placeholders(&catalog[key]),
                    placeholders(reference),
                    "{}:{key}",
                    locale.tag()
                );
            }
        }
    }

    #[test]
    fn language_tags_match_supported_locales() {
        assert_eq!(Locale::from_language_tag("zh-Hans-CN"), Some(Locale::ZhCn));
        assert_eq!(Locale::from_language_tag("es-MX"), Some(Locale::EsEs));
        assert_eq!(Locale::from_language_tag("de-DE"), None);
    }
}
