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
//! <base>/sysfs/drm/renderD128 -> /sys/class/drm/renderD128
//!                                                bound over /sys/dev/char/M:m/device/drm
//! <base>/sysfs/class-drm/renderD128/             bound over /sys/class/drm
//! ```
//!
//! `uevent` and `drm/` are also bound in the device's canonical directory
//! (`/sys/devices/platform/soc/<addr>.qcom,kgsl-3d0`), where
//! `/sys/dev/char/M:m/device` really leads. `realpath()` follows those real
//! sysfs links, so sd-device (which wants the canonical syspath) and
//! bubblewrap (which canonicalizes bind sources) reach the fake entries too.
//!
//! `/sys/class/drm/renderD128` is the node's sysfs device for udev: systemd's
//! sd-device enumerates the `drm` subsystem by listing `/sys/class` and
//! reads the node's name and number from its `uevent`. GStreamer's VA
//! plugin finds VA devices only that way. Android's own entries there (the
//! display's card0 and renderD128) are unreadable to apps, as is the
//! `/sys/class` listing itself; tawcroot lists binds beneath an unreadable
//! sysfs directory and reports them as sysfs, which sd-device checks.
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

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

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

/// `uevent` of the render node's own sysfs device, as udev reads it.
pub fn class_uevent(major: u32, minor: u32) -> String {
    format!("MAJOR={major}\nMINOR={minor}\nDEVNAME=dri/{RENDER_NODE_NAME}\nDEVTYPE=drm_minor\n")
}

/// The bind mounts (`host`, `guest`) that expose the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeDrmBinds {
    pub dri_dir: PathBuf,
    pub uevent_file: PathBuf,
    pub drm_dir: PathBuf,
    pub class_dir: PathBuf,
    /// Guest path `/sys/dev/char/M:m/device` resolves to, when readable.
    pub canonical_device_dir: Option<PathBuf>,
    pub major: u32,
    pub minor: u32,
}

impl FakeDrmBinds {
    /// `(host path, guest path)` pairs in bind order.
    pub fn pairs(&self) -> Vec<(PathBuf, PathBuf)> {
        let device_dir = guest_sysfs_device_dir(self.major, self.minor);
        let mut pairs = vec![
            (self.dri_dir.clone(), PathBuf::from("/dev/dri")),
            (
                self.uevent_file.clone(),
                PathBuf::from(format!("{device_dir}/uevent")),
            ),
            (
                self.drm_dir.clone(),
                PathBuf::from(format!("{device_dir}/drm")),
            ),
            (self.class_dir.clone(), PathBuf::from("/sys/class/drm")),
        ];
        if let Some(canonical) = &self.canonical_device_dir {
            pairs.push((self.uevent_file.clone(), canonical.join("uevent")));
            pairs.push((self.drm_dir.clone(), canonical.join("drm")));
        }
        pairs
    }
}

/// Where `/sys/dev/char/M:m/device` leads under the sysfs mounted at
/// `sys` (`/sys` on the device), as a `/sys/...` path; `None` when it cannot
/// be resolved or is the same directory.
#[cfg(unix)]
pub fn canonical_device_dir(sys: &Path, major: u32, minor: u32) -> Option<PathBuf> {
    let link = sys.join(format!("dev/char/{major}:{minor}/device"));
    let real = std::fs::canonicalize(&link).ok()?;
    let sys = std::fs::canonicalize(sys).ok()?;
    let canonical = Path::new("/sys").join(real.strip_prefix(&sys).ok()?);
    (canonical != Path::new(&guest_sysfs_device_dir(major, minor))).then_some(canonical)
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
    let class_dir = sysfs_dir.join("class-drm");
    let class_node = class_dir.join(RENDER_NODE_NAME);
    fs::create_dir_all(&dri_dir)?;
    fs::create_dir_all(&drm_dir)?;
    fs::create_dir_all(&class_node)?;

    let ensure_link = |target: &str, link: &Path| -> std::io::Result<()> {
        if fs::read_link(link).ok().as_deref() != Some(Path::new(target)) {
            let _ = fs::remove_file(link);
            symlink(target, link)?;
        }
        Ok(())
    };
    let ensure_file = |path: &Path, contents: &str| -> std::io::Result<()> {
        if fs::read_to_string(path).ok().as_deref() != Some(contents) {
            fs::write(path, contents)?;
        }
        Ok(())
    };

    ensure_link(KGSL_DEVICE, &dri_dir.join(RENDER_NODE_NAME))?;
    ensure_file(&uevent_file, fake_uevent())?;
    let class_path = format!("/sys/class/drm/{RENDER_NODE_NAME}");
    ensure_link(&class_path, &drm_dir.join(RENDER_NODE_NAME))?;
    ensure_file(&class_dir.join("version"), "drm 1.1.0 20060810\n")?;
    ensure_file(&class_node.join("uevent"), &class_uevent(major, minor))?;
    ensure_file(&class_node.join("dev"), &format!("{major}:{minor}\n"))?;
    ensure_link("/sys/class/drm", &class_node.join("subsystem"))?;
    ensure_link(
        &guest_sysfs_device_dir(major, minor),
        &class_node.join("device"),
    )?;

    Ok(FakeDrmBinds {
        dri_dir,
        uevent_file,
        drm_dir,
        class_dir,
        canonical_device_dir: canonical_device_dir(Path::new("/sys"), major, minor),
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
        let rdev = (456u64 & 0xfff) << 8;
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
        assert_eq!(guest_sysfs_device_dir(456, 0), "/sys/dev/char/456:0/device");
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
    fn bind_pairs_cover_dev_dri_and_the_sysfs_entries() {
        let binds = FakeDrmBinds {
            dri_dir: PathBuf::from("/h/dri"),
            uevent_file: PathBuf::from("/h/sysfs/uevent"),
            drm_dir: PathBuf::from("/h/sysfs/drm"),
            class_dir: PathBuf::from("/h/sysfs/class-drm"),
            canonical_device_dir: None,
            major: 456,
            minor: 0,
        };
        let pairs = binds.pairs();
        assert_eq!(pairs.len(), 4);
        assert_eq!(pairs[0].1, PathBuf::from("/dev/dri"));
        assert_eq!(
            pairs[1].1,
            PathBuf::from("/sys/dev/char/456:0/device/uevent")
        );
        assert_eq!(pairs[2].1, PathBuf::from("/sys/dev/char/456:0/device/drm"));
        assert_eq!(pairs[3].1, PathBuf::from("/sys/class/drm"));
    }

    #[test]
    fn canonical_device_dir_gets_the_sysfs_entries_too() {
        let binds = FakeDrmBinds {
            dri_dir: PathBuf::from("/h/dri"),
            uevent_file: PathBuf::from("/h/sysfs/uevent"),
            drm_dir: PathBuf::from("/h/sysfs/drm"),
            class_dir: PathBuf::from("/h/sysfs/class-drm"),
            canonical_device_dir: Some(PathBuf::from("/sys/devices/platform/soc/gpu")),
            major: 456,
            minor: 0,
        };
        let pairs = binds.pairs();
        assert_eq!(pairs.len(), 6);
        assert_eq!(
            pairs[4],
            (
                PathBuf::from("/h/sysfs/uevent"),
                PathBuf::from("/sys/devices/platform/soc/gpu/uevent")
            )
        );
        assert_eq!(
            pairs[5],
            (
                PathBuf::from("/h/sysfs/drm"),
                PathBuf::from("/sys/devices/platform/soc/gpu/drm")
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn canonical_device_dir_follows_the_sysfs_links() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let sys = temp.path();
        std::fs::create_dir_all(sys.join("devices/platform/soc/gpu/kgsl/kgsl-3d0")).unwrap();
        std::fs::create_dir_all(sys.join("dev/char")).unwrap();
        symlink(
            "../../devices/platform/soc/gpu/kgsl/kgsl-3d0",
            sys.join("dev/char/456:0"),
        )
        .unwrap();
        symlink(
            "../../../gpu",
            sys.join("devices/platform/soc/gpu/kgsl/kgsl-3d0/device"),
        )
        .unwrap();
        assert_eq!(
            canonical_device_dir(sys, 456, 0),
            Some(PathBuf::from("/sys/devices/platform/soc/gpu"))
        );
        assert_eq!(canonical_device_dir(sys, 456, 1), None);
    }

    #[test]
    fn class_uevent_names_the_render_node_for_udev() {
        let text = class_uevent(456, 0);
        assert!(text.lines().any(|l| l == "MAJOR=456"));
        assert!(text.lines().any(|l| l == "MINOR=0"));
        assert!(text.lines().any(|l| l == "DEVNAME=dri/renderD128"));
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
        assert_eq!(
            std::fs::read_link(first.drm_dir.join(RENDER_NODE_NAME)).unwrap(),
            std::path::PathBuf::from("/sys/class/drm/renderD128")
        );
        let class_node = first.class_dir.join(RENDER_NODE_NAME);
        assert_eq!(
            std::fs::read_to_string(class_node.join("uevent")).unwrap(),
            class_uevent(456, 0)
        );
        assert_eq!(
            std::fs::read_to_string(class_node.join("dev")).unwrap(),
            "456:0\n"
        );
        assert_eq!(
            std::fs::read_link(class_node.join("subsystem")).unwrap(),
            std::path::PathBuf::from("/sys/class/drm")
        );
        assert_eq!(
            std::fs::read_link(class_node.join("device")).unwrap(),
            std::path::PathBuf::from("/sys/dev/char/456:0/device")
        );
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
