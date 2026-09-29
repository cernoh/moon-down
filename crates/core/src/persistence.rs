use std::fs;
use std::path::Path;

use crate::queue::Queue;

/// Save queue state to `path` atomically via temp file + rename.
pub fn save_state(queue: &Queue, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let data = serde_json::to_vec_pretty(queue).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    // Write to temp file in same directory for atomic rename.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp_path = Path::new(&tmp);
    // Use a unique suffix to avoid collision on concurrent writers (not expected, but safe)
    let tmp_path = if tmp_path.exists() {
        let mut with_pid = tmp_path.as_os_str().to_owned();
        with_pid.push(format!(".{}", std::process::id()));
        Path::new(&with_pid).to_path_buf()
    } else {
        tmp_path.to_path_buf()
    };
    fs::write(&tmp_path, &data)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Restore queue state from `path`. If file does not exist returns default queue.
pub fn load_state(path: &Path) -> std::io::Result<Queue> {
    if !path.exists() {
        return Ok(Queue::new());
    }
    let data = fs::read(path)?;
    let queue: Queue = serde_json::from_slice(&data).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(queue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{MemberState, PackageStatus};
    use tempfile::tempdir;

    #[test]
    fn atomic_write_and_restore() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");

        let mut q = Queue::new();
        q.add_package("pkg-a", "/tmp/a", vec![("file1".into(), "https://example.com/a".into())]);
        let mid = q.packages[0].members[0].id;
        q.set_member_handle(mid, "GID123".into());
        q.set_member_progress(mid, 1000, 400);
        q.set_member_state(mid, MemberState::Downloading);

        save_state(&q, &path).unwrap();
        // temp file must not remain
        assert!(!dir.path().join("state.json.tmp").exists());

        // simulate restart
        let restored = load_state(&path).unwrap();
        assert_eq!(restored.packages.len(), 1);
        assert_eq!(restored.packages[0].members[0].handle.as_deref(), Some("GID123"));
        assert_eq!(restored.packages[0].members[0].total_bytes, 1000);
        assert_eq!(restored.packages[0].members[0].completed_bytes, 400);
        assert_eq!(restored.packages[0].status(), PackageStatus::Active);
        assert!((restored.overall_progress() - 0.4).abs() < 1e-9);
        // ids preserved
        assert_eq!(restored.next_package_id, q.next_package_id);
        assert_eq!(restored.next_member_id, q.next_member_id);
    }

    #[test]
    fn save_uses_tmp_plus_rename() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sub/state.json");
        let q = Queue::new();
        save_state(&q, &path).unwrap();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("packages"));
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.json");
        let q = load_state(&path).unwrap();
        assert!(q.packages.is_empty());
    }

    #[test]
    fn restore_preserves_queue_order() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut q = Queue::new();
        q.add_package("b", "/tmp", vec![("m".into(), "s".into())]);
        q.add_package("a", "/tmp", vec![("m".into(), "s".into())]);
        q.add_package("c", "/tmp", vec![("m".into(), "s".into())]);
        save_state(&q, &path).unwrap();
        let restored = load_state(&path).unwrap();
        let names: Vec<_> = restored.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["b", "a", "c"]);
    }
}
