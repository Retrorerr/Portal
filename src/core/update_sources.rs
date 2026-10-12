//! Upstream release feeds Portal updates itself and its dependencies from.
//!
//! * The app: the newest stable release of Retrorerr/Portal on GitHub, whose
//!   fixed-name `Portal.apk` asset is the signed release build.
//! * The Mesa KGSL layer: the newest `mesa-*` release of
//!   lfdevs/mesa-for-android-container (the GL driver KWin renders with).
//! * The ChatGPT desktop app: OpenAI publishes one unversioned `latest` .deb
//!   and no apt repository, so apt never sees its updates.
//!
//! Everything else in the guest (Debian, Firefox from Mozilla's repository,
//! Claude from Anthropic's) is ordinary apt; see `system_updates`.
//!
//! This module only parses and decides. Every download it accepts must be
//! HTTPS from the expected release host and carry a SHA-256 digest, which the
//! Android side enforces byte for byte before anything is installed.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Newest non-draft, non-prerelease Portal release.
pub const APP_RELEASE_API: &str = "https://api.github.com/repos/Retrorerr/Portal/releases/latest";
/// Every release asset of the app must come from here.
pub const APP_DOWNLOAD_PREFIX: &str = "https://github.com/Retrorerr/Portal/releases/download/";
/// The release build under a name that never changes (CI uploads it with
/// every release; the README's download button relies on it too).
pub const APP_ASSET: &str = "Portal.apk";

/// Recent lfdevs releases; Mesa and Turnip releases alternate in one feed.
pub const MESA_RELEASES_API: &str =
    "https://api.github.com/repos/lfdevs/mesa-for-android-container/releases?per_page=30";
pub const MESA_DOWNLOAD_PREFIX: &str =
    "https://github.com/lfdevs/mesa-for-android-container/releases/download/";
const MESA_TAG_PREFIX: &str = "mesa-";
const MESA_ASSET_PREFIX: &str = "mesa-for-android-container_";
const MESA_ASSET_SUFFIX: &str = "_debian_trixie_arm64.tar.gz";

/// OpenAI's unversioned ChatGPT build (see `optional_apps::install_command`).
pub const CHATGPT_DEB_URL: &str =
    "https://persistent.oaistatic.com/codex-app-prod/linux/deb/latest/chatgpt_arm64.deb";
/// Guest-relative record of the build Portal last installed: the server's
/// ETag on the first line, its Last-Modified on the second.
pub const CHATGPT_STAMP_REL: &str = "var/lib/localdesktop/chatgpt-deb.stamp";

/// Minimum time between checks of every feed. Checks are small (one API
/// call per feed, apt's package lists), so they can run often.
pub const CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;

/// The maintainer's update gate, fetched before every check so a broken
/// upstream build can be stopped on every install without a new APK.
pub const POLICY_URL: &str =
    "https://raw.githubusercontent.com/Retrorerr/Portal/main/assets/update-policy.json";
/// The copy this APK was built with: the fallback when the fetch fails and
/// nothing newer is cached.
pub const BUILTIN_POLICY: &str = include_str!("../../assets/update-policy.json");

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MesaMode {
    /// Only builds the maintainer listed as tested.
    #[default]
    Vetted,
    /// Any newer build (each still goes through the trial and rollback).
    Latest,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MesaPolicy {
    #[serde(default)]
    pub mode: MesaMode,
    #[serde(default)]
    pub vetted: Vec<String>,
    #[serde(default)]
    pub blocked: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdatePolicy {
    pub schema: u32,
    /// Debian packages pinned at their installed version.
    #[serde(default)]
    pub debian_hold: Vec<String>,
    #[serde(default)]
    pub mesa_layer: MesaPolicy,
    /// Portal releases never offered (`2.5.0`).
    #[serde(default)]
    pub app_blocked: Vec<String>,
}

/// Debian package names: lowercase alphanumerics and `+-.`, as Debian
/// policy defines them. Anything else is never written into apt's config.
pub fn is_debian_package_name(name: &str) -> bool {
    name.len() >= 2
        && name.len() <= 128
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+-.".contains(c))
}

impl UpdatePolicy {
    /// Parse a policy. Unknown schemas and malformed package names are
    /// rejected whole, so a bad edit can never loosen anything.
    pub fn parse(text: &str) -> Option<Self> {
        let policy: Self = serde_json::from_str(text).ok()?;
        (policy.schema == 1
            && policy
                .debian_hold
                .iter()
                .all(|name| is_debian_package_name(name)))
        .then_some(policy)
    }

    pub fn builtin() -> Self {
        Self::parse(BUILTIN_POLICY).unwrap_or(Self {
            schema: 1,
            ..Default::default()
        })
    }

    /// Whether a Mesa layer version may be installed.
    pub fn allows_mesa(&self, version: &str) -> bool {
        if self.mesa_layer.blocked.iter().any(|bad| bad == version) {
            return false;
        }
        match self.mesa_layer.mode {
            MesaMode::Latest => true,
            MesaMode::Vetted => self.mesa_layer.vetted.iter().any(|good| good == version),
        }
    }

    pub fn allows_app(&self, version: &str) -> bool {
        !self.app_blocked.iter().any(|bad| bad == version)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GithubRelease {
    #[serde(default)]
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct GithubAsset {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    /// `sha256:<hex>`, computed by GitHub when the asset was uploaded.
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub browser_download_url: String,
}

/// One file to fetch and the exact bytes it must have.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Download {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AppRelease {
    /// `2.5.0`, without the tag's `v`.
    pub version: String,
    /// The release notes' opening, for the update card.
    pub notes: String,
    pub page: String,
    pub download: Download,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MesaRelease {
    /// Same shape as the layer pin: `26.3.0-20260824`.
    pub version: String,
    pub tag: String,
    pub download: Download,
}

/// `v2.4.1` or `2.4.1` (a `-suffix` or `+build` is ignored).
pub fn parse_semver(text: &str) -> Option<(u32, u32, u32)> {
    let text = text.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    let core = text.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// The hex digest from GitHub's `sha256:<hex>`, lowercased; None for any
/// other algorithm or a malformed value.
pub fn sha256_from_digest(digest: Option<&str>) -> Option<String> {
    let hex = digest?.trim().strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

fn checked_download(asset: &GithubAsset, prefix: &str, max_size: u64) -> anyhow::Result<Download> {
    anyhow::ensure!(
        asset.browser_download_url.starts_with(prefix),
        "{} is not served from {prefix}",
        asset.name
    );
    anyhow::ensure!(
        asset.size > 0 && asset.size <= max_size,
        "{} has an implausible size ({} bytes)",
        asset.name,
        asset.size
    );
    let sha256 = sha256_from_digest(asset.digest.as_deref())
        .ok_or_else(|| anyhow::anyhow!("{} has no SHA-256 checksum", asset.name))?;
    Ok(Download {
        url: asset.browser_download_url.clone(),
        size: asset.size,
        sha256,
    })
}

/// Opening of the release notes: whole lines up to `limit` characters.
fn notes_excerpt(body: &str, limit: usize) -> String {
    let mut excerpt = String::new();
    for line in body.lines().map(str::trim_end) {
        if line.is_empty() && excerpt.is_empty() {
            continue;
        }
        if excerpt.len() + line.len() + 1 > limit {
            break;
        }
        if !excerpt.is_empty() {
            excerpt.push('\n');
        }
        excerpt.push_str(line);
    }
    excerpt.trim_end().to_owned()
}

/// One plain line describing a release for the update card: the first line
/// of its notes that is prose, without Markdown markers, cut at a word.
pub fn release_summary(notes: &str) -> Option<String> {
    const LIMIT: usize = 110;
    let line = notes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with("```"))
        .map(|line| line.trim_start_matches(['-', '*', '+', '>', ' ']))
        .map(|line| line.replace("**", "").replace('`', ""))
        .find(|line| !line.trim().is_empty())?;
    let line = line.trim();
    if line.chars().count() <= LIMIT {
        return Some(line.to_owned());
    }
    let cut: String = line.chars().take(LIMIT).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    Some(format!("{}…", cut.trim_end_matches([',', ';', ':', '.', ' '])))
}

/// The release to update to, when `release` is newer than `installed`.
/// `download_prefix` is [`APP_DOWNLOAD_PREFIX`] except for test feeds.
pub fn app_update(
    release: &GithubRelease,
    installed: &str,
    download_prefix: &str,
    policy: &UpdatePolicy,
) -> anyhow::Result<Option<AppRelease>> {
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let latest = parse_semver(&release.tag_name)
        .ok_or_else(|| anyhow::anyhow!("release tag {:?} is not a version", release.tag_name))?;
    let current = parse_semver(installed)
        .ok_or_else(|| anyhow::anyhow!("installed version {installed:?} is not a version"))?;
    if latest <= current {
        return Ok(None);
    }
    let version = format!("{}.{}.{}", latest.0, latest.1, latest.2);
    if !policy.allows_app(&version) {
        return Ok(None);
    }
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == APP_ASSET)
        .ok_or_else(|| anyhow::anyhow!("release {} has no {APP_ASSET}", release.tag_name))?;
    let download = checked_download(asset, download_prefix, 1 << 30)?;
    Ok(Some(AppRelease {
        version,
        notes: notes_excerpt(release.body.as_deref().unwrap_or_default(), 600),
        page: release.html_url.clone().unwrap_or_default(),
        download,
    }))
}

/// `mesa-26.3.0-devel-20260824` becomes `26.3.0-20260824`, the layer pin's
/// version shape.
pub fn mesa_version_from_tag(tag: &str) -> Option<String> {
    let rest = tag.strip_prefix(MESA_TAG_PREFIX)?;
    let (base, date) = rest.rsplit_once('-')?;
    if date.len() != 8 || !date.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (major, minor, patch) = parse_semver(base.split('-').next()?)?;
    Some(format!("{major}.{minor}.{patch}-{date}"))
}

fn mesa_key(version: &str) -> Option<(u32, (u32, u32, u32))> {
    let (base, date) = version.rsplit_once('-')?;
    if date.len() != 8 {
        return None;
    }
    Some((date.parse().ok()?, parse_semver(base)?))
}

/// Order two layer versions: build date first (lfdevs' devel snapshots are
/// dated), then the Mesa version.
pub fn compare_mesa_versions(a: &str, b: &str) -> Option<Ordering> {
    Some(mesa_key(a)?.cmp(&mesa_key(b)?))
}

/// The newest lfdevs Mesa layer newer than `installed` that the policy
/// allows, skipping releases without a trixie arm64 build or a checksum,
/// and any version in `denied` (a build that failed to start Plasma here).
pub fn mesa_update(
    releases: &[GithubRelease],
    installed: &str,
    denied: &[String],
    policy: &UpdatePolicy,
) -> Option<MesaRelease> {
    let mut best: Option<MesaRelease> = None;
    for release in releases {
        if release.draft || release.prerelease {
            continue;
        }
        let Some(version) = mesa_version_from_tag(&release.tag_name) else {
            continue;
        };
        if denied.contains(&version) || !policy.allows_mesa(&version) {
            continue;
        }
        if compare_mesa_versions(&version, installed) != Some(Ordering::Greater) {
            continue;
        }
        let Some(download) = release
            .assets
            .iter()
            .filter(|asset| {
                asset.name.starts_with(MESA_ASSET_PREFIX) && asset.name.ends_with(MESA_ASSET_SUFFIX)
            })
            .find_map(|asset| checked_download(asset, MESA_DOWNLOAD_PREFIX, 256 << 20).ok())
        else {
            continue;
        };
        let newer = best.as_ref().is_none_or(|current| {
            compare_mesa_versions(&version, &current.version) == Some(Ordering::Greater)
        });
        if newer {
            best = Some(MesaRelease {
                version,
                tag: release.tag_name.clone(),
                download,
            });
        }
    }
    best
}

/// Unix seconds from an HTTP date (`Fri, 09 Oct 2026 00:56:33 GMT`).
pub fn parse_http_date(text: &str) -> Option<u64> {
    let mut fields = text.split_whitespace();
    fields.next()?; // weekday
    let day: u64 = fields.next()?.parse().ok()?;
    let month = match fields.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = fields.next()?.parse().ok()?;
    let mut clock = fields.next()?.split(':');
    let hour: u64 = clock.next()?.parse().ok()?;
    let minute: u64 = clock.next()?.parse().ok()?;
    let second: u64 = clock.next()?.parse().ok()?;
    if fields.next()? != "GMT" || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60
    {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days).ok().map(|days| days * 86400 + hour * 3600 + minute * 60 + second)
}

/// What Portal knows about the installed ChatGPT build.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DebStamp {
    pub etag: Option<String>,
    pub last_modified: Option<u64>,
}

impl DebStamp {
    pub fn parse(text: &str) -> Self {
        let mut lines = text.lines().map(str::trim);
        let etag = lines.next().filter(|line| !line.is_empty()).map(str::to_owned);
        let last_modified = lines.next().and_then(parse_http_date);
        Self {
            etag,
            last_modified,
        }
    }
}

/// Whether the server's build differs from the installed one. With a
/// stamp, the ETag decides; installs from before stamps existed compare the
/// server's Last-Modified with when dpkg installed the package. No answer
/// from the server is never a reason to download 450 MB.
pub fn deb_refresh_needed(
    stamp: Option<&DebStamp>,
    installed_at: Option<u64>,
    remote_etag: Option<&str>,
    remote_last_modified: Option<u64>,
) -> bool {
    if let (Some(stamp), Some(remote)) = (stamp, remote_etag) {
        if let Some(local) = &stamp.etag {
            return local != remote;
        }
    }
    if let (Some(stamp), Some(remote)) = (stamp, remote_last_modified) {
        if let Some(local) = stamp.last_modified {
            return remote > local;
        }
    }
    match (installed_at, remote_last_modified) {
        (Some(installed), Some(remote)) => remote > installed,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_summary_is_the_first_prose_line() {
        assert_eq!(
            release_summary("Bug fixes on top of v2.4.0.\n\n- one\n- two").as_deref(),
            Some("Bug fixes on top of v2.4.0.")
        );
        assert_eq!(
            release_summary("## What's new\n\n- **Faster** `Proton` starts\n").as_deref(),
            Some("Faster Proton starts")
        );
        assert_eq!(release_summary("\n# Title\n\n"), None);
        let long = "word ".repeat(40);
        let summary = release_summary(&long).unwrap();
        assert!(summary.ends_with('…') && summary.chars().count() <= 111, "{summary}");
    }

    fn asset(name: &str, prefix: &str, digest: Option<&str>) -> GithubAsset {
        GithubAsset {
            name: name.to_owned(),
            size: 18_085_784,
            digest: digest.map(str::to_owned),
            browser_download_url: format!("{prefix}v2.5.0/{name}"),
        }
    }

    fn open() -> UpdatePolicy {
        UpdatePolicy {
            schema: 1,
            ..Default::default()
        }
    }

    fn latest() -> UpdatePolicy {
        UpdatePolicy {
            schema: 1,
            mesa_layer: MesaPolicy {
                mode: MesaMode::Latest,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    const DIGEST: &str = "sha256:29C7ACAAE088D245BD2E12C2E38BD22F50E8AE29AB45133F20CC38CDF78BE2B8";

    fn app_release(tag: &str) -> GithubRelease {
        GithubRelease {
            tag_name: tag.to_owned(),
            body: Some("\n\nFixes the thing.\nAnd another.\n".to_owned()),
            html_url: Some("https://github.com/Retrorerr/Portal/releases/tag/v2.5.0".to_owned()),
            assets: vec![
                asset("Portal-2.5.0-debug.apk", APP_DOWNLOAD_PREFIX, Some(DIGEST)),
                asset(APP_ASSET, APP_DOWNLOAD_PREFIX, Some(DIGEST)),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn semver_accepts_tags_and_rejects_junk() {
        assert_eq!(parse_semver("v2.4.1"), Some((2, 4, 1)));
        assert_eq!(parse_semver("2.10.0-rc1"), Some((2, 10, 0)));
        assert_eq!(parse_semver("2.4"), None);
        assert_eq!(parse_semver("2.4.1.7"), None);
        assert_eq!(parse_semver("latest"), None);
    }

    #[test]
    fn digest_must_be_sha256_hex() {
        assert_eq!(
            sha256_from_digest(Some(DIGEST)).as_deref(),
            Some("29c7acaae088d245bd2e12c2e38bd22f50e8ae29ab45133f20cc38cdf78be2b8")
        );
        assert_eq!(sha256_from_digest(Some("sha512:abcd")), None);
        assert_eq!(sha256_from_digest(Some("sha256:xyz")), None);
        assert_eq!(sha256_from_digest(None), None);
    }

    #[test]
    fn app_update_only_offers_newer_stable_releases() {
        let update = app_update(&app_release("v2.5.0"), "2.4.1", APP_DOWNLOAD_PREFIX, &open())
            .unwrap()
            .unwrap();
        assert_eq!(update.version, "2.5.0");
        assert_eq!(update.notes, "Fixes the thing.\nAnd another.");
        assert!(update.download.url.ends_with("/Portal.apk"));
        assert_eq!(update.download.sha256.len(), 64);

        assert_eq!(app_update(&app_release("v2.4.1"), "2.4.1", APP_DOWNLOAD_PREFIX, &open()).unwrap(), None);
        assert_eq!(app_update(&app_release("v2.4.0"), "2.4.1", APP_DOWNLOAD_PREFIX, &open()).unwrap(), None);
        let mut pre = app_release("v9.0.0");
        pre.prerelease = true;
        assert_eq!(app_update(&pre, "2.4.1", APP_DOWNLOAD_PREFIX, &open()).unwrap(), None);
        // 2.10 is newer than 2.9, not older.
        assert!(app_update(&app_release("v2.10.0"), "2.9.9", APP_DOWNLOAD_PREFIX, &open())
            .unwrap()
            .is_some());
    }

    #[test]
    fn app_update_fails_closed_on_untrusted_assets() {
        let mut no_digest = app_release("v2.5.0");
        no_digest.assets[1].digest = None;
        assert!(app_update(&no_digest, "2.4.1", APP_DOWNLOAD_PREFIX, &open()).is_err());

        let mut elsewhere = app_release("v2.5.0");
        elsewhere.assets[1].browser_download_url = "https://example.com/Portal.apk".to_owned();
        assert!(app_update(&elsewhere, "2.4.1", APP_DOWNLOAD_PREFIX, &open()).is_err());

        let mut missing = app_release("v2.5.0");
        missing.assets.remove(1);
        assert!(app_update(&missing, "2.4.1", APP_DOWNLOAD_PREFIX, &open()).is_err());

        let mut empty = app_release("v2.5.0");
        empty.assets[1].size = 0;
        assert!(app_update(&empty, "2.4.1", APP_DOWNLOAD_PREFIX, &open()).is_err());
    }

    fn mesa_release(tag: &str, asset_name: &str) -> GithubRelease {
        GithubRelease {
            tag_name: tag.to_owned(),
            assets: vec![GithubAsset {
                name: asset_name.to_owned(),
                size: 11_648_933,
                digest: Some(DIGEST.to_owned()),
                browser_download_url: format!("{MESA_DOWNLOAD_PREFIX}{tag}/{asset_name}"),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn mesa_versions_come_from_tags() {
        assert_eq!(
            mesa_version_from_tag("mesa-26.3.0-devel-20260824").as_deref(),
            Some("26.3.0-20260824")
        );
        assert_eq!(mesa_version_from_tag("mesa-26.4.1-20261101").as_deref(), Some("26.4.1-20261101"));
        assert_eq!(mesa_version_from_tag("turnip-26.3.0-devel-20260824"), None);
        assert_eq!(mesa_version_from_tag("mesa-26.3.0-devel"), None);
        assert_eq!(
            compare_mesa_versions("26.3.0-20260824", "26.2.0-20260709"),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_mesa_versions("26.4.0-20260901", "26.4.0-20260901"),
            Some(Ordering::Equal)
        );
        assert_eq!(compare_mesa_versions("junk", "26.4.0-20260901"), None);
    }

    #[test]
    fn mesa_update_picks_the_newest_usable_build() {
        let releases = vec![
            mesa_release(
                "turnip-26.4.0-devel-20261010",
                "turnip_26.4.0-devel-20261010_debian_trixie_arm64.tar.gz",
            ),
            mesa_release(
                "mesa-26.4.0-devel-20261010",
                "mesa-for-android-container_26.4.0-devel-20261010_debian_trixie_arm64.tar.gz",
            ),
            mesa_release(
                "mesa-26.4.0-devel-20260920",
                "mesa-for-android-container_26.4.0-devel-20260920_debian_trixie_arm64.tar.gz",
            ),
            mesa_release(
                "mesa-26.3.0-devel-20260824",
                "mesa-for-android-container_26.3.0-devel-20260824_debian_trixie_arm64.tar.gz",
            ),
        ];
        let update = mesa_update(&releases, "26.3.0-20260824", &[], &latest()).unwrap();
        assert_eq!(update.version, "26.4.0-20261010");
        assert_eq!(update.tag, "mesa-26.4.0-devel-20261010");

        // A build that failed here before is skipped, not retried forever.
        let update = mesa_update(&releases, "26.3.0-20260824", &["26.4.0-20261010".to_owned()], &latest()).unwrap();
        assert_eq!(update.version, "26.4.0-20260920");

        assert_eq!(mesa_update(&releases, "26.4.0-20261010", &[], &latest()), None);
    }

    #[test]
    fn mesa_update_needs_a_trixie_build_with_a_checksum() {
        let mut bookworm = mesa_release(
            "mesa-26.4.0-devel-20261010",
            "mesa-for-android-container_26.4.0-devel-20261010_debian_bookworm_arm64.tar.gz",
        );
        assert_eq!(mesa_update(&[bookworm.clone()], "26.3.0-20260824", &[], &latest()), None);
        bookworm.assets[0].name =
            "mesa-for-android-container_26.4.0-devel-20261010_debian_trixie_arm64.tar.gz".to_owned();
        bookworm.assets[0].digest = None;
        assert_eq!(mesa_update(&[bookworm], "26.3.0-20260824", &[], &latest()), None);
    }

    #[test]
    fn policy_gates_mesa_builds_and_app_releases() {
        let releases = vec![
            mesa_release(
                "mesa-26.4.0-devel-20261010",
                "mesa-for-android-container_26.4.0-devel-20261010_debian_trixie_arm64.tar.gz",
            ),
            mesa_release(
                "mesa-26.4.0-devel-20260920",
                "mesa-for-android-container_26.4.0-devel-20260920_debian_trixie_arm64.tar.gz",
            ),
        ];
        // Vetted mode: nothing until the maintainer lists a build...
        let mut vetted = UpdatePolicy::builtin();
        assert_eq!(vetted.mesa_layer.mode, MesaMode::Vetted);
        assert_eq!(mesa_update(&releases, "26.3.0-20260824", &[], &vetted), None);
        // ...then exactly that build, even when a newer one exists.
        vetted.mesa_layer.vetted.push("26.4.0-20260920".to_owned());
        assert_eq!(
            mesa_update(&releases, "26.3.0-20260824", &[], &vetted)
                .unwrap()
                .version,
            "26.4.0-20260920"
        );
        // Blocked wins over latest.
        let mut blocked = latest();
        blocked.mesa_layer.blocked.push("26.4.0-20261010".to_owned());
        assert_eq!(
            mesa_update(&releases, "26.3.0-20260824", &[], &blocked)
                .unwrap()
                .version,
            "26.4.0-20260920"
        );

        let mut no_app = open();
        no_app.app_blocked.push("2.5.0".to_owned());
        assert_eq!(
            app_update(&app_release("v2.5.0"), "2.4.1", APP_DOWNLOAD_PREFIX, &no_app).unwrap(),
            None
        );
    }

    #[test]
    fn policy_rejects_unknown_schemas_and_bad_package_names() {
        assert!(UpdatePolicy::parse(BUILTIN_POLICY).is_some());
        assert!(UpdatePolicy::parse(r#"{"schema": 2}"#).is_none());
        assert!(
            UpdatePolicy::parse(r#"{"schema": 1, "debian_hold": ["libfoo; rm -rf /"]}"#).is_none()
        );
        assert!(
            UpdatePolicy::parse(r#"{"schema": 1, "debian_hold": ["Pin-Priority: 1000"]}"#)
                .is_none()
        );
        let policy =
            UpdatePolicy::parse(r#"{"schema": 1, "debian_hold": ["libqt6core6t64", "g++-14"]}"#)
                .unwrap();
        assert_eq!(policy.debian_hold, ["libqt6core6t64", "g++-14"]);
        assert_eq!(policy.mesa_layer.mode, MesaMode::Vetted);
        assert!(!policy.allows_mesa("26.4.0-20261010"));
    }

    #[test]
    fn http_dates_parse() {
        assert_eq!(parse_http_date("Fri, 09 Oct 2026 00:56:33 GMT"), Some(1_791_507_393));
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_http_date("Fri, 09 Oct 2026 00:56:33 PST"), None);
        assert_eq!(parse_http_date("yesterday"), None);
    }

    #[test]
    fn chatgpt_refresh_prefers_the_etag() {
        let stamp = DebStamp::parse("\"0x8DF25A0297FED4D\"\nFri, 09 Oct 2026 00:56:33 GMT\n");
        assert_eq!(stamp.etag.as_deref(), Some("\"0x8DF25A0297FED4D\""));
        assert!(!deb_refresh_needed(Some(&stamp), None, Some("\"0x8DF25A0297FED4D\""), Some(9)));
        assert!(deb_refresh_needed(Some(&stamp), None, Some("\"0xNEW\""), None));
        // No ETag from the server: Last-Modified decides.
        assert!(deb_refresh_needed(Some(&stamp), None, None, Some(1_791_507_394)));
        assert!(!deb_refresh_needed(Some(&stamp), None, None, Some(1_791_507_393)));
        // A server that sent no ETag leaves the first line empty.
        let dated = DebStamp::parse("\nFri, 09 Oct 2026 00:56:33 GMT\n");
        assert_eq!(dated.etag, None);
        assert_eq!(dated.last_modified, stamp.last_modified);
        // Pre-stamp installs compare with dpkg's install time.
        assert!(deb_refresh_needed(None, Some(100), Some("\"x\""), Some(200)));
        assert!(!deb_refresh_needed(None, Some(300), Some("\"x\""), Some(200)));
        // Nothing to compare: never download blind.
        assert!(!deb_refresh_needed(None, None, Some("\"x\""), None));
        assert!(!deb_refresh_needed(None, Some(100), None, None));
    }
}
