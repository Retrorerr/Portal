//! Host-testable guest timezone synchronization.
//!
//! The APK is authoritative for the guest's timezone (docs/debian-provisioning.md): the guest's
//! `/etc/localtime` links to the zoneinfo file of Android's current zone and `/etc/timezone`
//! names it. Called on every launch, so this is idempotent and never rewrites unchanged files.

use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::symlink;
use std::path::{Component, Path};

/// What [`sync_guest_timezone`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimezoneSync {
    /// `/etc/localtime` and `/etc/timezone` were (re)written for the zone.
    Updated,
    /// Both already named the zone; nothing was written.
    Unchanged,
}

/// Point the guest rooted at `fs_root` at the IANA zone `zone_id` (e.g. `Europe/London`).
///
/// Fails without touching the guest when `zone_id` is not a plain relative zone name, when the
/// guest's zoneinfo database lacks it, or when `/etc/localtime` is something other than a file
/// or symlink (a directory there is left alone).
pub fn sync_guest_timezone(fs_root: &Path, zone_id: &str) -> io::Result<TimezoneSync> {
    let relative = Path::new(zone_id);
    if zone_id.is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("invalid timezone identifier {zone_id:?}"),
        ));
    }
    if !fs_root.join("usr/share/zoneinfo").join(relative).is_file() {
        return Err(io::Error::new(
            ErrorKind::NotFound,
            format!("{zone_id} is not in the guest zoneinfo database"),
        ));
    }

    let target = format!("/usr/share/zoneinfo/{zone_id}");
    let etc = fs_root.join("etc");
    let localtime = etc.join("localtime");
    let timezone = etc.join("timezone");
    let named = format!("{zone_id}\n");
    let link_current = fs::read_link(&localtime).is_ok_and(|link| link == Path::new(&target));
    let name_current = fs::read_to_string(&timezone).is_ok_and(|text| text == named);
    if link_current && name_current {
        return Ok(TimezoneSync::Unchanged);
    }

    fs::create_dir_all(&etc)?;
    if !link_current {
        match fs::symlink_metadata(&localtime) {
            Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                fs::remove_file(&localtime)?;
            }
            Ok(_) => {
                return Err(io::Error::new(
                    ErrorKind::AlreadyExists,
                    "guest /etc/localtime is not a file or symlink",
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        symlink(&target, &localtime)?;
    }
    if !name_current {
        fs::write(&timezone, named)?;
    }
    Ok(TimezoneSync::Updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest_with_zones(zones: &[&str]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for zone in zones {
            let file = root.path().join("usr/share/zoneinfo").join(zone);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, b"TZif").unwrap();
        }
        root
    }

    fn localtime_link(root: &Path) -> String {
        fs::read_link(root.join("etc/localtime"))
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn links_localtime_and_names_the_zone() {
        let root = guest_with_zones(&["Europe/London"]);
        assert_eq!(
            sync_guest_timezone(root.path(), "Europe/London").unwrap(),
            TimezoneSync::Updated
        );
        assert_eq!(
            localtime_link(root.path()),
            "/usr/share/zoneinfo/Europe/London"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("etc/timezone")).unwrap(),
            "Europe/London\n"
        );
    }

    #[test]
    fn repeated_sync_is_a_no_op_and_follows_zone_changes() {
        let root = guest_with_zones(&["Europe/London", "Asia/Tokyo"]);
        sync_guest_timezone(root.path(), "Europe/London").unwrap();
        assert_eq!(
            sync_guest_timezone(root.path(), "Europe/London").unwrap(),
            TimezoneSync::Unchanged
        );
        assert_eq!(
            sync_guest_timezone(root.path(), "Asia/Tokyo").unwrap(),
            TimezoneSync::Updated
        );
        assert_eq!(
            localtime_link(root.path()),
            "/usr/share/zoneinfo/Asia/Tokyo"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("etc/timezone")).unwrap(),
            "Asia/Tokyo\n"
        );
    }

    #[test]
    fn replaces_a_copied_localtime_file() {
        let root = guest_with_zones(&["UTC"]);
        fs::create_dir_all(root.path().join("etc")).unwrap();
        fs::write(root.path().join("etc/localtime"), b"TZif copy").unwrap();
        sync_guest_timezone(root.path(), "UTC").unwrap();
        assert_eq!(localtime_link(root.path()), "/usr/share/zoneinfo/UTC");
    }

    #[test]
    fn rejects_identifiers_that_are_not_plain_zone_names() {
        let root = guest_with_zones(&["UTC"]);
        for zone in ["", "/UTC", "../UTC", "Europe/../UTC", "./UTC"] {
            let error = sync_guest_timezone(root.path(), zone).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidInput, "{zone:?}");
        }
        assert!(!root.path().join("etc/localtime").exists());
    }

    #[test]
    fn leaves_the_guest_alone_for_an_unknown_zone_or_a_localtime_directory() {
        let root = guest_with_zones(&["UTC"]);
        let error = sync_guest_timezone(root.path(), "Mars/Olympus").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert!(!root.path().join("etc/localtime").exists());

        fs::create_dir_all(root.path().join("etc/localtime")).unwrap();
        assert!(sync_guest_timezone(root.path(), "UTC").is_err());
        assert!(root.path().join("etc/localtime").is_dir());
    }
}
