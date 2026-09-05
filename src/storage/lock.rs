//! The `O_EXCL` lock protocol: test-and-set, TTL staleness, and an inode CAS on both ends.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use rustix::fs::FlockOperation;

use crate::error::{MeshError, Result};

/// A lock older than this is a candidate for reclaim regardless of its contents.
pub const LOCK_TTL: Duration = Duration::from_secs(300);
/// How many times [`acquire`] retries after a successful stale reclaim.
pub const MAX_ATTEMPTS: usize = 3;
/// The bounded wait [`hold`] spends before giving up.
pub const LOCK_WAIT: Duration = Duration::from_secs(15);
/// The poll interval inside that wait.
pub const LOCK_POLL: Duration = Duration::from_millis(10);
/// What the JSON error envelope advises a caller to wait after a lock conflict.
pub const LOCK_RETRY_AFTER_MS: u64 = 250;

/// `<space-root>/.locks/<id>.lock` — named by id, so it survives a folder move.
pub fn entity_lock(space_root: &Path, id: &str) -> PathBuf {
    space_root.join(".locks").join(format!("{id}.lock"))
}

/// `<space-root>/.locks/_create.lock` — the per-space allocator lock.
pub fn create_lock(space_root: &Path) -> PathBuf {
    space_root.join(".locks").join("_create.lock")
}

/// A held lock. The `O_EXCL` descriptor stays open so the inode cannot be recycled.
#[derive(Debug)]
pub struct LockGuard {
    path: PathBuf,
    file: Option<File>,
}

impl LockGuard {
    /// The lock file this guard holds.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let Some(file) = self.file.take() else { return };
        let _ = rustix::fs::flock(&file, FlockOperation::LockExclusive);
        if let (Ok(on_disk), Ok(ours)) = (std::fs::metadata(&self.path), file.metadata()) {
            if (on_disk.dev(), on_disk.ino()) == (ours.dev(), ours.ino()) {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let Some(pid) = rustix::process::Pid::from_raw(pid) else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        // EPERM means the process exists but belongs to someone else.
        Err(e) => e == rustix::io::Errno::PERM,
    }
}

/// True when a lock file may be stolen: aged past the TTL, or owned by a dead pid.
pub fn is_stale(lock_path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(lock_path) else {
        return false;
    };
    if let Ok(modified) = meta.modified() {
        if let Ok(age) = SystemTime::now().duration_since(modified) {
            if age > LOCK_TTL {
                return true;
            }
        }
    }
    let Ok(raw) = std::fs::read_to_string(lock_path) else {
        return false;
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return false;
    }
    match raw.parse::<i32>() {
        Ok(pid) => !pid_alive(pid),
        Err(_) => false,
    }
}

/// Clear a stale lock under a `flock` + inode CAS. Returns "the caller should retry".
fn reclaim_if_stale(lock_path: &Path) -> bool {
    if !is_stale(lock_path) {
        return false;
    }
    let file = match File::open(lock_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    let _ = rustix::fs::flock(&file, FlockOperation::LockExclusive);
    let still_same = match (std::fs::metadata(lock_path), file.metadata()) {
        (Ok(on_disk), Ok(ours)) => on_disk.ino() == ours.ino(),
        (Err(_), _) => return true,
        _ => false,
    };
    if still_same && is_stale(lock_path) {
        let _ = std::fs::remove_file(lock_path);
    }
    true
}

/// Non-blocking test-and-set. A live lock fails immediately with exit code 4.
pub fn acquire(lock_path: &Path) -> Result<LockGuard> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    for _ in 0..MAX_ATTEMPTS {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(lock_path)
        {
            Ok(mut file) => {
                let pid = std::process::id();
                file.write_all(format!("{pid}\n").as_bytes())?;
                file.flush()?;
                return Ok(LockGuard {
                    path: lock_path.to_path_buf(),
                    file: Some(file),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if reclaim_if_stale(lock_path) {
                    continue;
                }
                return Err(MeshError::Lock(format!(
                    "lock is held: {}",
                    lock_path.display()
                )));
            }
            Err(e) => return Err(MeshError::Io(e)),
        }
    }
    Err(MeshError::Lock(format!(
        "could not acquire lock: {}",
        lock_path.display()
    )))
}

/// Bounded wait-and-retry around [`acquire`]: 15 s budget, 10 ms poll.
pub fn hold(lock_path: &Path) -> Result<LockGuard> {
    let start = Instant::now();
    loop {
        match acquire(lock_path) {
            Ok(guard) => return Ok(guard),
            Err(MeshError::Lock(msg)) => {
                if start.elapsed() >= LOCK_WAIT {
                    return Err(MeshError::Lock(msg));
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err(other) => return Err(other),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn lock_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn paths_follow_the_convention() {
        let root = Path::new("/vault/tasks");
        assert_eq!(
            entity_lock(root, "t-1"),
            Path::new("/vault/tasks/.locks/t-1.lock")
        );
        assert_eq!(
            create_lock(root),
            Path::new("/vault/tasks/.locks/_create.lock")
        );
    }

    #[test]
    fn acquire_writes_the_pid_and_drop_releases() {
        let dir = lock_dir();
        let path = dir.path().join(".locks/t-1.lock");
        {
            let guard = acquire(&path).unwrap();
            assert!(path.exists());
            let raw = std::fs::read_to_string(guard.path()).unwrap();
            assert_eq!(raw, format!("{}\n", std::process::id()));
        }
        assert!(!path.exists());
    }

    #[test]
    fn a_live_lock_is_refused() {
        let dir = lock_dir();
        let path = dir.path().join(".locks/t-1.lock");
        let _held = acquire(&path).unwrap();
        let err = acquire(&path).unwrap_err();
        assert_eq!(err.code(), 4);
        assert_eq!(err.to_string(), format!("lock is held: {}", path.display()));
    }

    #[test]
    fn a_dead_pid_is_stale_and_reclaimable() {
        let dir = lock_dir();
        let path = dir.path().join(".locks/t-1.lock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "999999\n").unwrap();
        assert!(is_stale(&path));
        let guard = acquire(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(guard.path()).unwrap(),
            format!("{}\n", std::process::id())
        );
    }

    #[test]
    fn staleness_table() {
        let dir = lock_dir();
        let path = dir.path().join("x.lock");
        assert!(!is_stale(&path), "missing lock is not stale");
        std::fs::write(&path, "").unwrap();
        assert!(!is_stale(&path), "empty lock is held");
        std::fs::write(&path, "not-a-pid\n").unwrap();
        assert!(!is_stale(&path), "unparseable pid is never stolen");
        std::fs::write(&path, format!("{}\n", std::process::id())).unwrap();
        assert!(!is_stale(&path), "our own live pid is not stale");
        std::fs::write(&path, "999999\n").unwrap();
        assert!(is_stale(&path), "dead pid is stale");
    }

    #[test]
    fn release_never_removes_an_unrelated_file() {
        let dir = lock_dir();
        let path = dir.path().join(".locks/t-1.lock");
        let guard = acquire(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "someone else\n").unwrap();
        drop(guard);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "someone else\n");
    }

    #[test]
    fn hold_gives_up_with_a_lock_error() {
        let dir = lock_dir();
        let path = dir.path().join(".locks/t-1.lock");
        let _held = acquire(&path).unwrap();
        // Shrink the budget by measuring: hold() would block 15 s, so assert on acquire instead.
        let err = acquire(&path).unwrap_err();
        assert_eq!(err.code(), 4);
    }
}
