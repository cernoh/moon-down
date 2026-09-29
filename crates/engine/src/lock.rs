use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;

#[derive(Debug)]
pub struct DirLock {
    _file: File,
    path: PathBuf,
}

#[derive(Debug)]
pub enum LockError {
    AlreadyLocked,
    Io(io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyLocked => write!(f, "state dir already locked (second instance)"),
            Self::Io(e) => write!(f, "lock io error: {e}"),
        }
    }
}

/// Try to acquire an exclusive non-blocking lock for `state_dir`.
/// Holds the file open as long as the DirLock lives.
pub fn try_acquire(state_dir: &Path) -> Result<DirLock, LockError> {
    std::fs::create_dir_all(state_dir).map_err(|e| LockError::Io(e))?;
    let path = state_dir.join(".lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| LockError::Io(e))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(DirLock { _file: file, path }),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(LockError::AlreadyLocked),
        Err(e) => {
            // fs2 on linux returns WouldBlock as AlreadyLocked; other errors
            let msg = e.to_string().to_lowercase();
            if msg.contains("would block") || msg.contains("temporarily unavailable") {
                Err(LockError::AlreadyLocked)
            } else {
                Err(LockError::Io(e))
            }
        }
    }
}

 #[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn second_lock_refused() {
        let dir = tempdir().unwrap();
        let _a = try_acquire(dir.path()).unwrap();
        let b = try_acquire(dir.path());
        assert!(matches!(b, Err(LockError::AlreadyLocked)));
    }
    #[test]
    fn lock_released_on_drop() {
        let dir = tempdir().unwrap();
        {
            let _a = try_acquire(dir.path()).unwrap();
        }
        // after drop, should succeed
        let _b = try_acquire(dir.path()).unwrap();
    }
}
