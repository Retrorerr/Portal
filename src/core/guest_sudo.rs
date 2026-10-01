//! Passwordless `sudo` for the guest's `desktop` account.
//!
//! The guest is a stock Debian userland whose only login has no password, so
//! `sudo apt install <anything>` has to work without one. Four separate
//! things must hold for that; each was measured on the device (the check
//! fails with a different message when any one of them is missing):
//!
//! * `sudo` needs its setuid bit. PRoot's `fake_id0` emulates setuid on
//!   `execve` (from the file's real mode, or from the mode PRoot's metadata
//!   records when dpkg installed it), but Portal's image extraction strips
//!   the bit, and without it sudo stays uid 1000 and refuses to run.
//! * PAM's account check needs `/etc/shadow` entries for `root` and
//!   `desktop`; the image ships none, so sudo reports
//!   "account validation failure, is your account locked?".
//! * `desktop` must belong to the `sudo` group, which `/etc/sudoers`
//!   already grants (with a password).
//! * `/etc/sudoers.d/portal-desktop` drops that password: the account has none
//!   to type, and the guest is not a security boundary (every guest process
//!   runs as the same Android uid).
//!
//! The sync is idempotent and never overwrites what a user changed: existing
//! account entries are kept, and a sudoers drop-in that already exists is left
//! as it is. It only edits Portal's own image defaults, never packages, and
//! installs nothing. The image builder (`scripts/build_debian_rootfs.py`)
//! ships the group and drop-in too, so a new install starts with them; this
//! runs on every launch so existing installs converge (and to restore the
//! setuid bit and shadow entries no image can carry).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const DESKTOP_USER: &str = "desktop";
pub const SUDO_GROUP: &str = "sudo";
/// Debian's `base-passwd` reserves gid 27 for `sudo`.
pub const SUDO_GID: u32 = 27;
pub const SUDOERS_DROPIN_RELATIVE: &str = "etc/sudoers.d/portal-desktop";
/// Must stay byte-identical to `SUDOERS_DROPIN` in
/// `scripts/build_debian_rootfs.py` (tests/guest_access.rs checks it).
pub const SUDOERS_DROPIN: &str =
    "# Managed by Portal: the desktop account has no password, so sudo asks for none.\n\
%sudo ALL=(ALL:ALL) NOPASSWD: ALL\n";

/// What one pass changed and what it could not do.
#[derive(Debug, Default)]
pub struct SudoAccessReport {
    pub changed: Vec<&'static str>,
    pub failed: Vec<(&'static str, io::Error)>,
}

/// Bring `fs_root`'s account databases, `sudo` binary and sudoers drop-in to
/// the state described in the module docs. Each step is independent: one
/// failing (a missing file, a read-only tree) does not skip the others.
pub fn sync_sudo_access(fs_root: &Path) -> SudoAccessReport {
    let mut report = SudoAccessReport::default();
    let mut record = |step: &'static str, result: io::Result<bool>| match result {
        Ok(true) => report.changed.push(step),
        Ok(false) => {}
        Err(error) => report.failed.push((step, error)),
    };

    let passwd = fs::read_to_string(fs_root.join("etc/passwd")).unwrap_or_default();
    record(
        "group",
        edit_file(&fs_root.join("etc/group"), ensure_group_member),
    );
    record(
        "gshadow",
        edit_file(&fs_root.join("etc/gshadow"), ensure_gshadow_member),
    );
    record(
        "shadow",
        edit_file(&fs_root.join("etc/shadow"), |text| {
            ensure_shadow_entries(text, &passwd, days_since_epoch())
        }),
    );
    record("setuid", ensure_setuid(&fs_root.join("usr/bin/sudo")));
    record("sudoers", ensure_sudoers_dropin(fs_root));
    report
}

/// Read `path`, and when `edit` yields new text write it back atomically with
/// the file's own permissions. A missing file is not an error: the account
/// databases only exist once the image has been configured.
fn edit_file(path: &Path, edit: impl FnOnce(&str) -> Option<String>) -> io::Result<bool> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(updated) = edit(&text) else {
        return Ok(false);
    };
    let permissions = fs::metadata(path)?.permissions();
    let temporary = sibling_temporary(path);
    fs::write(&temporary, updated)?;
    fs::set_permissions(&temporary, permissions)?;
    // PRoot keys its ownership/mode metadata by path, so a rename keeps
    // reporting the file as root-owned with its original mode.
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    Ok(true)
}

fn sibling_temporary(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(".{name}.portal-tmp"))
}

/// `/etc/group`: make `desktop` a member of `sudo`, creating the group when
/// it is missing. `None` when nothing needs to change.
pub fn ensure_group_member(text: &str) -> Option<String> {
    ensure_member(text, || {
        let gid = free_gid(text);
        format!("{SUDO_GROUP}:x:{gid}:{DESKTOP_USER}")
    })
}

/// `/etc/gshadow` mirrors the member list. It is only updated when it
/// already has a `sudo` entry: a missing gshadow line is harmless to sudo,
/// and creating one would be guessing at the file's state.
pub fn ensure_gshadow_member(text: &str) -> Option<String> {
    text.lines()
        .any(|line| line.split(':').next() == Some(SUDO_GROUP))
        .then(|| ensure_member(text, String::new))
        .flatten()
}

/// Add `DESKTOP_USER` to the member list (the fourth field, in both
/// `/etc/group` and `/etc/gshadow`) of the `sudo` line, or append
/// `new_line()` when there is no such line.
fn ensure_member(text: &str, new_line: impl FnOnce() -> String) -> Option<String> {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    match lines
        .iter()
        .position(|line| line.split(':').next() == Some(SUDO_GROUP))
    {
        Some(index) => {
            let mut fields: Vec<&str> = lines[index].split(':').collect();
            if fields.len() != 4 {
                // Not a well-formed record: leave the user's file alone.
                return None;
            }
            let mut members: Vec<&str> = fields[3]
                .split(',')
                .filter(|member| !member.is_empty())
                .collect();
            if members.contains(&DESKTOP_USER) {
                return None;
            }
            members.push(DESKTOP_USER);
            let joined = members.join(",");
            fields[3] = &joined;
            lines[index] = fields.join(":");
        }
        None => lines.push(new_line()),
    }
    Some(lines.join("\n") + "\n")
}

/// The gid for a `sudo` group that has to be created: Debian's fixed 27,
/// unless another group already uses it, then the highest free system gid.
fn free_gid(group_text: &str) -> u32 {
    let used: Vec<u32> = group_text
        .lines()
        .filter_map(|line| line.split(':').nth(2)?.parse().ok())
        .collect();
    if !used.contains(&SUDO_GID) {
        return SUDO_GID;
    }
    (100..=999)
        .rev()
        .find(|gid| !used.contains(gid))
        .unwrap_or(SUDO_GID)
}

/// `/etc/shadow`: a locked (`*`) entry for `root` and `desktop` when the
/// passwd database has the account but shadow has no line for it. Existing
/// lines are never modified. `None` when nothing needs to change.
pub fn ensure_shadow_entries(text: &str, passwd: &str, last_change_days: u64) -> Option<String> {
    let mut updated = text.to_owned();
    for user in ["root", DESKTOP_USER] {
        let in_passwd = passwd
            .lines()
            .any(|line| line.split(':').next() == Some(user));
        let in_shadow = text
            .lines()
            .any(|line| line.split(':').next() == Some(user));
        if in_passwd && !in_shadow {
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str(&format!("{user}:*:{last_change_days}:0:99999:7:::\n"));
        }
    }
    (updated != text).then_some(updated)
}

fn days_since_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() / 86_400)
        .unwrap_or(0)
}

/// Give `/usr/bin/sudo` the setuid bit PRoot's `fake_id0` looks for. Absent
/// file (sudo not installed) is fine. A binary dpkg installed under PRoot
/// carries the bit in PRoot's metadata instead and keeps working with this
/// one added on top.
#[cfg(unix)]
fn ensure_setuid(path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let mode = match fs::metadata(path) {
        Ok(metadata) => metadata.permissions().mode(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if mode & 0o4000 != 0 {
        return Ok(false);
    }
    fs::set_permissions(path, fs::Permissions::from_mode((mode & 0o7777) | 0o4000))?;
    Ok(true)
}

#[cfg(not(unix))]
fn ensure_setuid(_: &Path) -> io::Result<bool> {
    Ok(false)
}

/// Install the passwordless drop-in unless a file already stands there
/// (whatever its content, so a user's edit is never overwritten) or sudo is
/// not installed.
fn ensure_sudoers_dropin(fs_root: &Path) -> io::Result<bool> {
    let dropin = fs_root.join(SUDOERS_DROPIN_RELATIVE);
    if fs::symlink_metadata(&dropin).is_ok() {
        return Ok(false);
    }
    let directory = dropin.parent().expect("drop-in path has a parent");
    if !directory.is_dir() {
        return Ok(false);
    }
    // sudo skips include-dir files whose names contain a dot, so a
    // half-written temporary can never be read as policy.
    let temporary = sibling_temporary(&dropin);
    fs::write(&temporary, SUDOERS_DROPIN)?;
    set_read_only_mode(&temporary)?;
    fs::rename(&temporary, &dropin).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    Ok(true)
}

/// sudo insists on mode 0440 (no write for group/other).
#[cfg(unix)]
fn set_read_only_mode(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o440))
}

#[cfg(not(unix))]
fn set_read_only_mode(_: &Path) -> io::Result<()> {
    Ok(())
}
