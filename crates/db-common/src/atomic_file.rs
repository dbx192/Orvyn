//! Durable replacement and cross-process serialization for local state files.
use crate::Result;
use fs2::FileExt;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

/// Hold a stable sidecar lock through the entire read/modify/write transaction.
/// Locking the data file itself is unsafe because replacement changes its inode.
pub fn transaction<T>(path: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.lock_exclusive()?;
    // Closing the handle releases the lock on both success and error paths.
    operation()
}

/// Write in the destination directory, sync, then atomically replace the target.
/// A failed write leaves the previous file intact. Temporary files are removed on error.
pub fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_transactions_preserve_every_update() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("counter");
        replace(&path, b"0").unwrap();
        std::thread::scope(|scope| {
            for _ in 0..12 {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..10 {
                        transaction(path, || {
                            let value = fs::read_to_string(path)?.parse::<u32>().unwrap();
                            replace(path, (value + 1).to_string().as_bytes())
                        })
                        .unwrap();
                    }
                });
            }
        });
        assert_eq!(fs::read_to_string(path).unwrap(), "120");
    }
}
