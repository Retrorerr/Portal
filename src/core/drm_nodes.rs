//! Fake DRM render-node tree for guest clients.
//!
//! Android's app sandbox denies `/dev/dri` (SELinux), so libdrm inside the
//! guest finds no devices. Mesa's own clients cope (the kgsl winsys talks to
//! `/dev/kgsl-3d0` directly), but anything that enumerates DRM nodes through
//! libdrm does not: Chromium/Electron statically bundle libdrm, ask it for a
//! render node with `drmGetDevices2`, and fall back to software compositing
//! when none exists. KWin also advertises the kgsl node's device number as
//! its dmabuf main device, which those clients then look up through
//! `/sys/dev/char/<major>:<minor>/device/drm`.
//!
//! The tree built here is bound into the guest by PRoot so those lookups
//! succeed without any per-application flag:
//!
//! ```text
//! <base>/dri/renderD128 -> /dev/kgsl-3d0        bound over /dev/dri
//! <base>/sysfs/uevent                            bound over /sys/dev/char/M:m/device/uevent
//! <base>/sysfs/drm/                              bound over /sys/dev/char/M:m/device/drm
//! ```
//!
//! PRoot's `--bind=host:guest` splits on the FIRST colon only, so the guest
//! half may contain colons (`/sys/dev/char/456:0/...`) unescaped; escaping
//! them as `\:` yields a literal backslash and a bind that never matches.
//!
//! `renderD128` is a symlink so `stat` reports the kgsl character device's
//! real major:minor, which is exactly the device number KWin advertises. The
//! sysfs `subsystem` link already exists on the host (`platform`) and is left
//! alone. Opening the node still needs the `drmshim.so` preload, which
//! answers the DRM version query Mesa uses to pick its kgsl winsys.

use std::path::{Path, PathBuf};

/// Host path of the kgsl device whose number the fake node reports.
pub const KGSL_DEVICE: &str = "/dev/kgsl-3d0";

/// Name of the render node inside the fake `/dev/dri`.
pub const RENDER_NODE_NAME: &str = "renderD128";

/// Split a Linux `dev_t` into `(major, minor)` (glibc/bionic `major`/`minor`).
pub fn dev_major_minor(rdev: u64) -> (u32, u32) {
    let major = (((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfff)) as u32;
    let minor = ((rdev & 0xff) | ((rdev >> 12) & !0xff)) as u32;
    (major, minor)
}

/// Guest sysfs directory libdrm consults for a character device number.
pub fn guest_sysfs_device_dir(major: u32, minor: u32) -> String {
    format!("/sys/dev/char/{major}:{minor}/device")
}

/// Contents of the fake `uevent`. libdrm's platform-bus parser needs
/// `OF_FULLNAME` for the bus id and `OF_COMPATIBLE_*` for the device info.
pub fn fake_uevent() -> &'static str {
    "DRIVER=kgsl\n\
     OF_NAME=qcom,kgsl-3d0\n\
     OF_FULLNAME=/soc/qcom,kgsl-3d0@3d00000\n\
     OF_COMPATIBLE_0=qcom,kgsl-3d0\n\
     OF_COMPATIBLE_N=1\n"
}

/// The three bind mounts (`host`, `guest`) that expose the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeDrmBinds {
    pub dri_dir: PathBuf,
    pub uevent_file: PathBuf,
    pub drm_dir: PathBuf,
    pub major: u32,
    pub minor: u32,
}

impl FakeDrmBinds {
    /// `(host path, guest path)` pairs in bind order.
    pub fn pairs(&self) -> Vec<(PathBuf, PathBuf)> {
        let device_dir = guest_sysfs_device_dir(self.major, self.minor);
        vec![
            (self.dri_dir.clone(), PathBuf::from("/dev/dri")),
            (
                self.uevent_file.clone(),
                PathBuf::from(format!("{device_dir}/uevent")),
            ),
            (
                self.drm_dir.clone(),
                PathBuf::from(format!("{device_dir}/drm")),
            ),
        ]
    }
}

/// Build (or refresh) the fake tree under `base` for a kgsl node with the
/// given `dev_t`. Idempotent; only writes what differs.
#[cfg(unix)]
pub fn build_fake_drm_tree(base: &Path, kgsl_rdev: u64) -> std::io::Result<FakeDrmBinds> {
    use std::fs;
    use std::os::unix::fs::symlink;

    let (major, minor) = dev_major_minor(kgsl_rdev);
    let dri_dir = base.join("dri");
    let sysfs_dir = base.join("sysfs");
    let drm_dir = sysfs_dir.join("drm");
    let uevent_file = sysfs_dir.join("uevent");
    fs::create_dir_all(&dri_dir)?;
    fs::create_dir_all(&drm_dir)?;

    let node = dri_dir.join(RENDER_NODE_NAME);
    let current = fs::read_link(&node).ok();
    if current.as_deref() != Some(Path::new(KGSL_DEVICE)) {
        let _ = fs::remove_file(&node);
        symlink(KGSL_DEVICE, &node)?;
    }

    let uevent = fake_uevent();
    if fs::read_to_string(&uevent_file).ok().as_deref() != Some(uevent) {
        fs::write(&uevent_file, uevent)?;
    }

    Ok(FakeDrmBinds {
        dri_dir,
        uevent_file,
        drm_dir,
        major,
        minor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_device_numbers_like_libc() {
        // makedev(456, 0) on 64-bit glibc/bionic.
        let rdev = (456u64 & 0xfff) << 8 | (456u64 & !0xfff) << 32;
        assert_eq!(dev_major_minor(rdev), (456, 0));
        // makedev(226, 128), the conventional DRM render node.
        let rdev = 226u64 << 8 | 128;
        assert_eq!(dev_major_minor(rdev), (226, 128));
        // A minor above 255 uses the high bits.
        let rdev = 10u64 << 8 | 0x2f | (0x3u64 << 20);
        assert_eq!(dev_major_minor(rdev), (10, 0x2f | (0x3 << 8)));
    }

    #[test]
    fn guest_paths_follow_the_device_number() {
        assert_eq!(
            guest_sysfs_device_dir(456, 0),
            "/sys/dev/char/456:0/device"
        );
    }

    #[test]
    fn uevent_carries_what_libdrm_platform_parsing_reads() {
        let text = fake_uevent();
        assert!(text.lines().any(|l| l.starts_with("OF_FULLNAME=")));
        assert!(text.lines().any(|l| l == "OF_COMPATIBLE_N=1"));
        assert!(text.lines().any(|l| l.starts_with("OF_COMPATIBLE_0=")));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn bind_pairs_cover_dev_dri_and_both_sysfs_entries() {
        let binds = FakeDrmBinds {
            dri_dir: PathBuf::from("/h/dri"),
            uevent_file: PathBuf::from("/h/sysfs/uevent"),
            drm_dir: PathBuf::from("/h/sysfs/drm"),
            major: 456,
            minor: 0,
        };
        let pairs = binds.pairs();
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0].1, PathBuf::from("/dev/dri"));
        assert_eq!(
            pairs[1].1,
            PathBuf::from("/sys/dev/char/456:0/device/uevent")
        );
        assert_eq!(pairs[2].1, PathBuf::from("/sys/dev/char/456:0/device/drm"));
    }

    #[cfg(unix)]
    #[test]
    fn tree_is_built_idempotently_with_a_kgsl_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let rdev = 456u64 << 8;
        let first = build_fake_drm_tree(temp.path(), rdev).unwrap();
        let second = build_fake_drm_tree(temp.path(), rdev).unwrap();
        assert_eq!(first, second);
        assert_eq!((first.major, first.minor), (456, 0));

        let node = first.dri_dir.join(RENDER_NODE_NAME);
        assert_eq!(
            std::fs::read_link(&node).unwrap(),
            std::path::PathBuf::from(KGSL_DEVICE)
        );
        assert_eq!(
            std::fs::read_to_string(&first.uevent_file).unwrap(),
            fake_uevent()
        );
        assert!(first.drm_dir.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_wrong_symlink_or_stale_uevent_is_repaired() {
        let temp = tempfile::tempdir().unwrap();
        let first = build_fake_drm_tree(temp.path(), 456u64 << 8).unwrap();
        let node = first.dri_dir.join(RENDER_NODE_NAME);
        std::fs::remove_file(&node).unwrap();
        std::os::unix::fs::symlink("/dev/null", &node).unwrap();
        std::fs::write(&first.uevent_file, "stale\n").unwrap();

        build_fake_drm_tree(temp.path(), 456u64 << 8).unwrap();
        assert_eq!(
            std::fs::read_link(&node).unwrap(),
            std::path::PathBuf::from(KGSL_DEVICE)
        );
        assert_eq!(
            std::fs::read_to_string(&first.uevent_file).unwrap(),
            fake_uevent()
        );
    }
}
