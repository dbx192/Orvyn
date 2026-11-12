use std::{
    env, fs,
    path::{Path, PathBuf},
};

pub(crate) fn data_dir() -> PathBuf {
    if let Some(path) = env::var_os("ORVYN_CONFIG_DIR") {
        return PathBuf::from(path);
    }
    #[cfg(windows)]
    if let Some(path) = env::var_os("APPDATA") {
        return PathBuf::from(path).join("Orvyn");
    }
    #[cfg(target_os = "macos")]
    if let Some(path) = env::var_os("HOME") {
        return PathBuf::from(path).join("Library/Application Support/Orvyn");
    }
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("orvyn")
}

pub(crate) fn master_key(dir: &Path) -> std::io::Result<String> {
    if let Some(key) = env::var_os("ORVYN_MASTER_KEY") {
        let key = key.to_string_lossy().into_owned();
        if key.trim().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ORVYN_MASTER_KEY is empty",
            ));
        }
        return Ok(key);
    }
    let path = dir.join("master.key");
    db_common::atomic_file::transaction(&path, || {
        match fs::read_to_string(&path) {
            Ok(key) if !key.trim().is_empty() => return Ok(key.trim().to_string()),
            Ok(_) => {
                return Err(db_common::AppError::validation(
                    "master.key is empty; restore the original key to recover saved credentials",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let key = uuid::Uuid::new_v4().to_string();
        // Atomic replacement uses a private tempfile (0600 on Unix).
        db_common::atomic_file::replace(&path, key.as_bytes())?;
        Ok(key)
    })
    .map_err(std::io::Error::other)
}
