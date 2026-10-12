//! Portal's own updates, from the newest stable release of
//! Retrorerr/Portal on GitHub.
//!
//! The check reads the release feed and picks the fixed-name `Portal.apk`
//! (see `core::update_sources::app_update`). The APK is downloaded into the
//! app's files, verified against the SHA-256 GitHub recorded at upload, and
//! handed to Android's PackageInstaller by `AppUpdateInstaller.kt`, which
//! also confirms the package name, a higher version code and a signing
//! certificate shared with the installed app before Android's own checks.
//! Android shows its confirmation and replaces the app; Portal's process
//! ends there.
//!
//! The release APK is the Stable package, so Portal Debug never updates
//! from it. A debug build can be pointed at a test feed by writing a feed
//! URL to `files/app-update-feed` (debug builds only).

use crate::core::{
    config::{APP_FILES_ROOT, VERSION},
    update_sources::{
        app_update, parse_semver, AppRelease, GithubRelease, UpdatePolicy, APP_DOWNLOAD_PREFIX, APP_RELEASE_API,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard, OnceLock},
    time::Duration,
};

/// How long the user has to answer Android's install confirmation.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

fn updates_dir() -> PathBuf {
    Path::new(APP_FILES_ROOT).join("updates")
}

/// The feed and the prefix every asset must come from.
fn feed() -> Option<(String, String)> {
    if !cfg!(feature = "portal-debug") {
        return Some((APP_RELEASE_API.to_owned(), APP_DOWNLOAD_PREFIX.to_owned()));
    }
    let url = fs::read_to_string(Path::new(APP_FILES_ROOT).join("app-update-feed")).ok()?;
    let url = url.trim().to_owned();
    let scheme_end = url.find("://")? + 3;
    let origin_end = url[scheme_end..].find('/').map_or(url.len(), |at| scheme_end + at);
    let prefix = format!("{}/", &url[..origin_end]);
    (url.starts_with("http://") || url.starts_with("https://")).then_some((url, prefix))
}

/// Whether this build updates itself at all.
pub fn enabled() -> bool {
    feed().is_some()
}

/// The release to update to, if one is newer than this build.
pub fn check(policy: &UpdatePolicy) -> anyhow::Result<Option<AppRelease>> {
    let Some((url, prefix)) = feed() else {
        return Ok(None);
    };
    let release: GithubRelease = crate::android::update_net::get_json(&url)?;
    app_update(&release, VERSION, &prefix, policy)
}

pub fn apk_path(release: &AppRelease) -> PathBuf {
    updates_dir().join(format!("Portal-{}.apk", release.version))
}

/// Whether the release's APK is already downloaded and verified.
pub fn is_downloaded(release: &AppRelease) -> bool {
    crate::android::update_net::file_matches(&apk_path(release), &release.download)
}

/// The background pre-download and a user's update share one file.
fn download_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Remove downloads of any release other than `keep`.
pub fn discard_stale(keep: Option<&AppRelease>) {
    let _download = download_lock();
    discard_stale_locked(keep);
}

fn discard_stale_locked(keep: Option<&AppRelease>) {
    let keep = keep.map(apk_path);
    let Ok(entries) = fs::read_dir(updates_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("Portal-") {
            continue;
        }
        let kept = keep.as_ref().is_some_and(|keep| {
            path == *keep || name == format!("{}.part", keep.file_name().unwrap_or_default().to_string_lossy())
        });
        if !kept {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Remove downloads of this build or older: Android has installed them, so
/// the APK need not wait for the next check to go.
pub fn discard_installed() {
    let _download = download_lock();
    let Ok(entries) = fs::read_dir(updates_dir()) else {
        return;
    };
    let current = parse_semver(VERSION);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let version = name
            .strip_prefix("Portal-")
            .and_then(|rest| rest.strip_suffix(".apk").or_else(|| rest.strip_suffix(".apk.part")))
            .and_then(parse_semver);
        if version.is_some_and(|version| Some(version) <= current) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Download and verify the release's APK.
pub fn download(
    release: &AppRelease,
    offline_budget: Duration,
    progress: impl FnMut(u64, u64),
) -> anyhow::Result<PathBuf> {
    let _download = download_lock();
    discard_stale_locked(Some(release));
    let path = apk_path(release);
    crate::android::update_net::download_verified(&release.download, &path, offline_budget, progress)?;
    Ok(path)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstallOutcome {
    /// Android replaced the app (Portal's process normally ends first).
    Installed,
    Cancelled,
    /// Portal may not install apps yet ("Install unknown apps").
    NeedsPermission,
    Failed(String),
}

impl InstallOutcome {
    fn from_code(code: i32, message: String) -> Self {
        match code {
            0 => Self::Installed,
            1 => Self::Cancelled,
            2 => Self::NeedsPermission,
            _ => Self::Failed(if message.is_empty() {
                "Android could not install the update".to_owned()
            } else {
                message
            }),
        }
    }
}

fn result_slot() -> &'static (Mutex<Option<InstallOutcome>>, Condvar) {
    static SLOT: OnceLock<(Mutex<Option<InstallOutcome>>, Condvar)> = OnceLock::new();
    SLOT.get_or_init(|| (Mutex::new(None), Condvar::new()))
}

/// Called from Kotlin when the install session ends.
pub fn report_install_result(code: i32, message: String) {
    let outcome = InstallOutcome::from_code(code, message);
    log::info!("app-update: install result {outcome:?}");
    let (lock, ready) = result_slot();
    if let Ok(mut slot) = lock.lock() {
        *slot = Some(outcome);
        ready.notify_all();
    }
}

/// Hand a verified APK to Android and wait for the user's answer.
pub fn install(apk: &Path) -> InstallOutcome {
    let (lock, ready) = result_slot();
    if let Ok(mut slot) = lock.lock() {
        *slot = None;
    }
    let android_app = crate::android::utils::application_context::get_application_context().android_app;
    if !crate::android::utils::compose_overlay::request_app_install(&android_app, apk) {
        return InstallOutcome::Failed("Portal could not start Android's installer".to_owned());
    }
    let Ok(slot) = lock.lock() else {
        return InstallOutcome::Failed("the install result was lost".to_owned());
    };
    match ready.wait_timeout_while(slot, INSTALL_TIMEOUT, |slot| slot.is_none()) {
        Ok((mut slot, timeout)) if !timeout.timed_out() => slot.take().unwrap_or(InstallOutcome::Cancelled),
        _ => InstallOutcome::Cancelled,
    }
}
