//! Boot-time clearing of the guest's `/tmp`.
//!
//! Debian 13 mounts `/tmp` as a tmpfs, so every boot starts empty. Portal's
//! guest `/tmp` is a plain directory in the runtime, so without this it keeps
//! everything every earlier session left: dead D-Bus and X sockets, crashed
//! apps' shared-memory files, Steam's per-launch directories, downloads.
//!
//! Portal's "boot" is a new app process: Android kills the whole app cgroup
//! with the old one, so nothing older than the current process can still be
//! in use. Host components (the Wayland and audio sockets, the IME FIFOs, the
//! VA bridge) may already have written into `/tmp` in this process, so only
//! entries whose whole tree predates the process are removed.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

/// Directories the session needs to exist; their stale contents are cleared
/// but the directories themselves are kept.
const KEPT_DIRECTORIES: [&str; 2] = [".X11-unix", ".ICE-unix"];

/// Remove the entries of `tmp` last modified before `boot`. Returns how many
/// top-level entries were removed. Failures are skipped: a leftover is
/// harmless, a half-cleared tree is retried at the next boot.
pub fn clear_stale(tmp: &Path, boot: SystemTime) -> usize {
    let Ok(entries) = fs::read_dir(tmp) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if KEPT_DIRECTORIES.iter().any(|kept| name == *kept) {
            removed += clear_stale(&path, boot);
            continue;
        }
        if newest_modification(&path).is_some_and(|newest| newest < boot) {
            let result = match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path),
                Ok(_) => fs::remove_file(&path),
                Err(error) => Err(error),
            };
            if result.is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// The newest modification time anywhere in the tree at `path` (symlinks are
/// not followed); `None` when any part of it cannot be read, so an
/// unreadable tree is never treated as stale.
fn newest_modification(path: &Path) -> Option<SystemTime> {
    let metadata = fs::symlink_metadata(path).ok()?;
    let mut newest = metadata.modified().ok()?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).ok()? {
            newest = newest.max(newest_modification(&entry.ok()?.path())?);
        }
    }
    Some(newest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::Duration;

    fn age(path: &Path, when: SystemTime) {
        File::options()
            .write(true)
            .open(path)
            .or_else(|_| File::open(path))
            .and_then(|file| file.set_modified(when))
            .unwrap();
    }

    #[test]
    fn clears_what_earlier_sessions_left_and_keeps_this_one() {
        let root = tempfile::tempdir().unwrap();
        let tmp = root.path();
        let boot = SystemTime::now();
        let before = boot.checked_sub(Duration::from_secs(3600)).unwrap();

        fs::write(tmp.join("u1000-Shm_old"), b"x").unwrap();
        age(&tmp.join("u1000-Shm_old"), before);
        fs::create_dir_all(tmp.join("steam/sub")).unwrap();
        fs::write(tmp.join("steam/sub/f"), b"x").unwrap();
        for path in ["steam/sub/f", "steam/sub", "steam"] {
            age(&tmp.join(path), before);
        }
        // An old directory a current-process component wrote into is kept.
        fs::create_dir_all(tmp.join("termux-va")).unwrap();
        fs::write(tmp.join("termux-va/live"), b"x").unwrap();
        age(&tmp.join("termux-va"), before);
        fs::write(tmp.join("wayland-0"), b"x").unwrap();
        fs::create_dir_all(tmp.join(".X11-unix")).unwrap();
        fs::write(tmp.join(".X11-unix/X0"), b"x").unwrap();
        age(&tmp.join(".X11-unix/X0"), before);
        age(&tmp.join(".X11-unix"), before);

        assert_eq!(clear_stale(tmp, boot), 3);
        assert!(!tmp.join("u1000-Shm_old").exists());
        assert!(!tmp.join("steam").exists());
        assert!(tmp.join("termux-va/live").exists());
        assert!(tmp.join("wayland-0").exists());
        assert!(tmp.join(".X11-unix").is_dir());
        assert!(!tmp.join(".X11-unix/X0").exists());
    }

    #[test]
    fn missing_directory_is_nothing_to_do() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            clear_stale(&root.path().join("absent"), SystemTime::now()),
            0
        );
    }
}
