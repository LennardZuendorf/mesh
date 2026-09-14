//! Atomic, mode-preserving file writes: sibling temp → fsync → rename → parent fsync.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::error::{MeshError, Result};

/// Write `contents` to `path` atomically, preserving the destination's mode when it exists.
///
/// A fresh file gets `0o666 & !umask`. Any failure before the rename removes the temp file and
/// leaves the destination untouched.
pub fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("mesh");
    let mut temp = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .suffix(".tmp")
        .tempfile_in(parent)?;
    temp.write_all(contents.as_bytes())?;
    temp.flush()?;
    match_destination_mode(temp.as_file(), path);
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| MeshError::Io(e.error))?;
    fsync_dir(parent);
    Ok(())
}

/// The mode a brand-new file gets: `0o666 & !umask`.
pub fn fresh_file_mode() -> u32 {
    let current = rustix::process::umask(rustix::fs::Mode::empty());
    rustix::process::umask(current);
    0o666 & !current.bits()
}

fn match_destination_mode(file: &std::fs::File, path: &Path) {
    let mode = match std::fs::metadata(path) {
        Ok(meta) => meta.permissions().mode() & 0o7777,
        Err(_) => fresh_file_mode(),
    };
    let Some(mode) = rustix::fs::Mode::from_bits(mode) else {
        return;
    };
    let _ = rustix::fs::fchmod(file, mode);
}

fn fsync_dir(dir: &Path) {
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep/nested/file.md");
        atomic_write(&path, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn preserves_an_existing_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.md");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
        atomic_write(&path, "new").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o664);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn leaves_no_temp_files_behind() {
        let dir = tempfile::tempdir().unwrap();
        atomic_write(&dir.path().join("f.md"), "x").unwrap();
        let entries: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, ["f.md"]);
    }

    #[test]
    fn a_hardlinked_destination_is_severed_not_mutated() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let inside = dir.path().join("inside.md");
        std::fs::hard_link(&outside, &inside).unwrap();
        atomic_write(&inside, "overwritten").unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
        assert_eq!(std::fs::read_to_string(&inside).unwrap(), "overwritten");
    }
}
