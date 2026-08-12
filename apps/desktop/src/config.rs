use std::{env, fs, path::PathBuf};

pub(crate) fn data_dir() -> PathBuf {
    if let Some(path) = env::var_os("ORVYN_CONFIG_DIR") {
        return PathBuf::from(path);
    }
    #[cfg(windows)]
    if let Some(path) = env::var_os("APPDATA") {
        return PathBuf::from(path).join("Orvyn");
    }
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("orvyn")
}

pub(crate) fn master_key(dir: &PathBuf) -> std::io::Result<String> {
    if let Some(key) = env::var_os("ORVYN_MASTER_KEY") {
        return Ok(key.to_string_lossy().into_owned());
    }
    let path = dir.join("master.key");
    if let Ok(key) = fs::read_to_string(&path) {
        if !key.trim().is_empty() {
            return Ok(key.trim().to_string());
        }
    }
    fs::create_dir_all(dir)?;
    let key = uuid::Uuid::new_v4().to_string();
    fs::write(&path, &key)?;
    Ok(key)
}
