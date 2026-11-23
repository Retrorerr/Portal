//! Workspaces: separate Debian installs, one of which runs at a time.
//!
//! Each workspace has its own directory holding everything that belongs to
//! one Debian install: `runtime-B` with its A/B promotion siblings, the
//! tawcroot hardlink store (which tawcroot derives as the rootfs's sibling),
//! the first-run install plan, deferred optional apps and the package update
//! state. Everything else in the app files directory (Portal settings, the
//! APK updater, the Mesa layer and library overlays, diagnostics) is shared.
//!
//! The first workspace, `main`, is the app files directory itself, where
//! every install made before workspaces existed already keeps those files.
//! An existing install therefore becomes a workspace without moving a byte,
//! and a Portal that predates workspaces still finds it. Later workspaces
//! live in `<files>/workspaces/<id>/`.
//!
//! The registry is written only once something differs from that implicit
//! single-workspace state, so an install that never makes a second
//! workspace never has one.
//!
//! A new workspace starts as an empty directory: a fresh Debian with its own
//! home, apps and settings, shared with no other workspace. Portal restarts
//! into it once (`open_next`) and the ordinary first-run installer sets it
//! up there. Until that install has committed the workspace needs setup:
//! opening it resumes the installer, and the installer can give it up and
//! go back to a workspace that works.
//!
//! Any workspace can be deleted, Main included: Main's install files leave
//! the app files directory and Portal's own stay. Deleting the open one
//! empties it and restarts into its installer, which can then give it up.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

/// The workspace every install starts with: the app files directory itself.
pub const MAIN_ID: &str = "main";
/// What the first workspace is called until the user renames it.
pub const MAIN_NAME: &str = "Main";
/// The rootfs inside a workspace directory.
pub const ROOTFS_DIR: &str = "runtime-B";
/// Where later workspaces live, and the registry beside them.
pub const WORKSPACES_DIR: &str = "workspaces";
pub const REGISTRY_FILE: &str = "registry.json";
/// Longest name the veil has room for, in characters.
pub const MAX_NAME_CHARS: usize = 40;
const MAX_ID_CHARS: usize = 32;
const REGISTRY_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    /// Seconds since the Unix epoch; 0 when unknown (the migrated main).
    #[serde(default)]
    pub created_unix: u64,
    #[serde(default)]
    pub last_used_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    /// The workspace Portal runs now, or ran last.
    pub active: String,
    /// The workspace Portal opens at launch; `None` means the last one used.
    #[serde(default)]
    pub open_at_launch: Option<String>,
    /// Opened by the next launch only, ahead of `open_at_launch`: the
    /// workspace Portal restarts into. Older Portals ignore the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_next: Option<String>,
    pub workspaces: Vec<Workspace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    EmptyName,
    NameTooLong,
    NameHasControlCharacters,
    NameTaken,
    Unknown(String),
    ActiveCannotBeRemoved,
    LastCannotBeRemoved,
    AlreadyActive,
    FilesMissing,
    /// Its Debian install hasn't committed: opening it resumes the installer.
    NeedsSetup,
    /// Only a workspace still being set up can be given up from its installer.
    AlreadySetUp,
    /// Another workspace opened while the open one was being deleted.
    NotOpen,
    FolderUnavailable,
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "Give the workspace a name."),
            Self::NameTooLong => write!(f, "Keep the name to {MAX_NAME_CHARS} characters."),
            Self::NameHasControlCharacters => {
                write!(f, "The name can't contain control characters.")
            }
            Self::NameTaken => write!(f, "That name is taken."),
            Self::Unknown(id) => write!(f, "There is no workspace {id}."),
            Self::ActiveCannotBeRemoved => {
                write!(f, "Switch to another workspace before deleting this one.")
            }
            Self::LastCannotBeRemoved => write!(f, "Portal needs at least one workspace."),
            Self::AlreadyActive => write!(f, "That workspace is already open."),
            Self::FilesMissing => write!(f, "That workspace's files are missing."),
            Self::NeedsSetup => write!(f, "That workspace isn't set up yet."),
            Self::AlreadySetUp => write!(f, "That workspace is already set up."),
            Self::NotOpen => write!(f, "That workspace isn't open any more."),
            Self::FolderUnavailable => {
                write!(f, "Portal couldn't make the workspace's folder.")
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

/// The directory holding one workspace's files.
pub fn workspace_dir(app_files: &Path, id: &str) -> PathBuf {
    if id == MAIN_ID {
        app_files.to_path_buf()
    } else {
        app_files.join(WORKSPACES_DIR).join(id)
    }
}

/// The Debian rootfs of one workspace.
pub fn workspace_rootfs(app_files: &Path, id: &str) -> PathBuf {
    workspace_dir(app_files, id).join(ROOTFS_DIR)
}

pub fn registry_path(app_files: &Path) -> PathBuf {
    app_files.join(WORKSPACES_DIR).join(REGISTRY_FILE)
}

/// Tells whether a rootfs holds a committed install. Production passes
/// [`system_ready`]; tests pass their own.
pub type SystemReady<'a> = &'a dyn Fn(&Path) -> bool;

/// Whether a workspace's Debian install has committed. Main counts while its
/// rootfs is there at all: it is the install Portal had before workspaces
/// (legacy markers included), and its own launch path repairs it rather than
/// offering a fresh install. Once deleted, it needs setup like any other.
pub fn is_set_up(app_files: &Path, id: &str, system_ready: SystemReady) -> bool {
    let rootfs = workspace_rootfs(app_files, id);
    if id == MAIN_ID {
        fs::symlink_metadata(rootfs).is_ok()
    } else {
        system_ready(&rootfs)
    }
}

/// Files in the app files directory that belong to Main's Debian install
/// rather than to Portal itself: the rootfs with its A/B promotion and
/// quarantine siblings (`runtime-B.*`), the runtime download, tawcroot's
/// hardlink store and tmpfs beside the rootfs, the install plan and the
/// deferred optional apps. An entry matches by its name or `<name>.` prefix.
/// Everything else there (settings, the APK updater, the Mesa layer,
/// diagnostics, `workspaces/`) is shared and is never deleted with Main.
const MAIN_INSTALL_ENTRIES: &[&str] = &[
    ROOTFS_DIR,
    // With its resumable `.part` and `.part.offset`.
    "portal-runtime.tar.xz",
    "tawcroot",
    ".tawcroot-tmpfs",
    crate::core::install_plan::INSTALL_PLAN_FILE,
    DEFERRED_OPTIONAL_APPS_FILE,
];
/// Optional apps the installer left for the desktop's first start.
pub const DEFERRED_OPTIONAL_APPS_FILE: &str = "optional-apps-deferred";
/// Where Main's files go while they are deleted, so an interrupted delete
/// leaves nothing that reads as an install.
const MAIN_TRASH_DIR: &str = ".deleting-main";

fn is_main_install_entry(name: &str) -> bool {
    MAIN_INSTALL_ENTRIES.iter().any(|entry| {
        name == *entry
            || name
                .strip_prefix(entry)
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

/// The production test of a committed install: the runtime's completion
/// marker over a valid Debian 13 tree.
pub fn system_ready(rootfs: &Path) -> bool {
    crate::core::provisioning::RuntimeArtifact::production().is_bootable(rootfs)
}

/// Trims a name and folds runs of whitespace into one space, then checks it
/// fits the veil.
pub fn normalise_name(name: &str) -> Result<String, WorkspaceError> {
    if name.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return Err(WorkspaceError::NameHasControlCharacters);
    }
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(WorkspaceError::EmptyName);
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(WorkspaceError::NameTooLong);
    }
    Ok(name)
}

/// A directory-safe id: lowercase ASCII letters, digits and single dashes.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_CHARS
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
}

fn slug(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    // Leave room for a "-<n>" suffix.
    slug.truncate(MAX_ID_CHARS - 4);
    let slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() {
        "workspace".to_owned()
    } else {
        slug
    }
}

impl Registry {
    /// The implicit registry of an install that has never made a second
    /// workspace: just `main`, active.
    pub fn implicit() -> Self {
        Self {
            version: REGISTRY_VERSION,
            active: MAIN_ID.to_owned(),
            open_at_launch: None,
            open_next: None,
            workspaces: vec![Workspace {
                id: MAIN_ID.to_owned(),
                name: MAIN_NAME.to_owned(),
                created_unix: 0,
                last_used_unix: 0,
            }],
        }
    }

    pub fn get(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Result<&mut Workspace, WorkspaceError> {
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)
            .ok_or_else(|| WorkspaceError::Unknown(id.to_owned()))
    }

    /// Records that the running workspace was in use until now, as Portal
    /// restarts out of it. [`Registry::setup_return`] goes back by this.
    fn mark_left(&mut self, now_unix: u64) {
        let active = self.active.clone();
        if let Ok(workspace) = self.get_mut(&active) {
            workspace.last_used_unix = now_unix;
        }
    }

    pub fn active(&self) -> &Workspace {
        self.get(&self.active)
            .expect("a validated registry names an existing active workspace")
    }

    /// The workspace to open when Portal starts.
    pub fn launch_target(&self) -> &Workspace {
        self.open_at_launch
            .as_deref()
            .and_then(|id| self.get(id))
            .unwrap_or_else(|| self.active())
    }

    /// Checks the invariants every other method relies on.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.version >= 1 && self.version <= REGISTRY_VERSION,
            "unsupported workspace registry version {}",
            self.version
        );
        anyhow::ensure!(
            !self.workspaces.is_empty(),
            "the registry lists no workspaces"
        );
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for workspace in &self.workspaces {
            anyhow::ensure!(
                is_valid_id(&workspace.id),
                "invalid workspace id {:?}",
                workspace.id
            );
            anyhow::ensure!(
                ids.insert(workspace.id.as_str()),
                "duplicate workspace id {}",
                workspace.id
            );
            let name = normalise_name(&workspace.name)
                .map_err(|error| anyhow::anyhow!("workspace {}: {error}", workspace.id))?;
            anyhow::ensure!(
                name == workspace.name,
                "workspace {} has an unnormalised name",
                workspace.id
            );
            anyhow::ensure!(
                names.insert(name.to_lowercase()),
                "duplicate workspace name {}",
                workspace.name
            );
        }
        anyhow::ensure!(
            ids.contains(self.active.as_str()),
            "active workspace {} is not listed",
            self.active
        );
        if let Some(launch) = &self.open_at_launch {
            anyhow::ensure!(
                ids.contains(launch.as_str()),
                "launch workspace {launch} is not listed"
            );
        }
        if let Some(next) = &self.open_next {
            anyhow::ensure!(
                ids.contains(next.as_str()),
                "next workspace {next} is not listed"
            );
        }
        Ok(())
    }

    fn name_taken(&self, name: &str, except: Option<&str>) -> bool {
        let name = name.to_lowercase();
        self.workspaces.iter().any(|workspace| {
            Some(workspace.id.as_str()) != except && workspace.name.to_lowercase() == name
        })
    }

    fn unique_id(&self, name: &str, also_taken: &dyn Fn(&str) -> bool) -> String {
        let base = slug(name);
        let mut id = base.clone();
        let mut n = 2;
        // `main` is reserved for the app files directory itself.
        while id == MAIN_ID || self.get(&id).is_some() || also_taken(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// Registers a new workspace. Its files are the caller's to create.
    pub fn add(&mut self, name: &str, now_unix: u64) -> Result<Workspace, WorkspaceError> {
        self.add_avoiding(name, now_unix, &|_| false)
    }

    fn add_avoiding(
        &mut self,
        name: &str,
        now_unix: u64,
        also_taken: &dyn Fn(&str) -> bool,
    ) -> Result<Workspace, WorkspaceError> {
        let name = normalise_name(name)?;
        if self.name_taken(&name, None) {
            return Err(WorkspaceError::NameTaken);
        }
        let workspace = Workspace {
            id: self.unique_id(&name, also_taken),
            name,
            created_unix: now_unix,
            last_used_unix: 0,
        };
        self.workspaces.push(workspace.clone());
        Ok(workspace)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), WorkspaceError> {
        let name = normalise_name(name)?;
        if self.name_taken(&name, Some(id)) {
            return Err(WorkspaceError::NameTaken);
        }
        self.get_mut(id)?.name = name;
        Ok(())
    }

    /// Unregisters a workspace that is not running, Main included. Deleting
    /// its files is the caller's job, after the registry without it has been
    /// saved.
    pub fn remove(&mut self, id: &str) -> Result<Workspace, WorkspaceError> {
        let index = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == id)
            .ok_or_else(|| WorkspaceError::Unknown(id.to_owned()))?;
        if self.active == id {
            return Err(WorkspaceError::ActiveCannotBeRemoved);
        }
        if self.workspaces.len() == 1 {
            return Err(WorkspaceError::LastCannotBeRemoved);
        }
        if self.open_at_launch.as_deref() == Some(id) {
            self.open_at_launch = None;
        }
        if self.open_next.as_deref() == Some(id) {
            self.open_next = None;
        }
        Ok(self.workspaces.remove(index))
    }

    pub fn set_active(&mut self, id: &str, now_unix: u64) -> Result<(), WorkspaceError> {
        self.get_mut(id)?.last_used_unix = now_unix;
        self.active = id.to_owned();
        Ok(())
    }

    /// The workspace a switch to `id` would open, if it can open. One whose
    /// install hasn't committed opens through a restart into its installer
    /// instead ([`Registry::request_setup`]).
    pub fn switch_target(
        &self,
        app_files: &Path,
        id: &str,
        system_ready: SystemReady,
    ) -> Result<Workspace, WorkspaceError> {
        let target = self
            .get(id)
            .ok_or_else(|| WorkspaceError::Unknown(id.to_owned()))?;
        if self.active == id {
            return Err(WorkspaceError::AlreadyActive);
        }
        if !workspace_dir(app_files, id).is_dir() {
            return Err(WorkspaceError::FilesMissing);
        }
        if !is_set_up(app_files, id, system_ready) {
            return Err(WorkspaceError::NeedsSetup);
        }
        Ok(target.clone())
    }

    /// Registers a new, empty workspace, makes its folder and asks the next
    /// launch to open it. A folder left behind by an earlier workspace is
    /// never reused, so a new workspace is always a clean system.
    pub fn create(
        &mut self,
        app_files: &Path,
        name: &str,
        now_unix: u64,
    ) -> Result<Workspace, WorkspaceError> {
        let workspace = self.add_avoiding(name, now_unix, &|id| {
            fs::symlink_metadata(workspace_dir(app_files, id)).is_ok()
        })?;
        if fs::create_dir_all(workspace_dir(app_files, &workspace.id)).is_err() {
            self.workspaces.retain(|listed| listed.id != workspace.id);
            return Err(WorkspaceError::FolderUnavailable);
        }
        self.mark_left(now_unix);
        self.open_next = Some(workspace.id.clone());
        Ok(workspace)
    }

    /// Asks the next launch to open `id`, whose install hasn't committed, so
    /// its installer starts or resumes there.
    pub fn request_setup(
        &mut self,
        app_files: &Path,
        id: &str,
        now_unix: u64,
        system_ready: SystemReady,
    ) -> Result<Workspace, WorkspaceError> {
        let target = self
            .get(id)
            .ok_or_else(|| WorkspaceError::Unknown(id.to_owned()))?
            .clone();
        if is_set_up(app_files, id, system_ready) {
            return Err(WorkspaceError::AlreadySetUp);
        }
        // A folder that went missing starts again empty.
        fs::create_dir_all(workspace_dir(app_files, id))
            .map_err(|_| WorkspaceError::FolderUnavailable)?;
        self.mark_left(now_unix);
        self.open_next = Some(id.to_owned());
        Ok(target)
    }

    /// Where the running workspace's installer goes back to if its setup is
    /// given up: the most recently used workspace that is set up. `None`
    /// when the running workspace is set up, or when no other one is (its
    /// installer is then the only way on).
    pub fn setup_return(&self, app_files: &Path, system_ready: SystemReady) -> Option<&Workspace> {
        if is_set_up(app_files, &self.active, system_ready) {
            return None;
        }
        self.workspaces
            .iter()
            .filter(|workspace| {
                workspace.id != self.active && is_set_up(app_files, &workspace.id, system_ready)
            })
            .max_by_key(|workspace| workspace.last_used_unix)
    }

    /// Empties the running workspace for a fresh install, as deleting it
    /// from inside does: it stays listed, needing setup, and the next launch
    /// opens its installer, whose "Back to …" gives it up for good. Launch
    /// stops preferring it, so Portal doesn't start into an installer every
    /// time. Deleting its files is the caller's job, after this is saved.
    pub fn reset_active(&mut self, id: &str, now_unix: u64) -> Result<Workspace, WorkspaceError> {
        if self.active != id {
            return Err(WorkspaceError::NotOpen);
        }
        if self.open_at_launch.as_deref() == Some(id) {
            self.open_at_launch = None;
        }
        self.open_next = Some(id.to_owned());
        let workspace = self.get_mut(id)?;
        workspace.last_used_unix = now_unix;
        Ok(workspace.clone())
    }

    /// Gives up the running workspace's unfinished setup: Portal goes back
    /// to [`Registry::setup_return`] and the workspace is unregistered.
    /// Returns (the one given up, the one to open). Deleting its files is the
    /// caller's job, after this registry is saved.
    pub fn abandon_setup(
        &mut self,
        app_files: &Path,
        system_ready: SystemReady,
    ) -> Result<(Workspace, Workspace), WorkspaceError> {
        let id = self.active.clone();
        let back = self
            .setup_return(app_files, system_ready)
            .ok_or(WorkspaceError::AlreadySetUp)?
            .clone();
        self.active = back.id.clone();
        self.open_next = Some(back.id.clone());
        let removed = self.remove(&id)?;
        Ok((removed, back))
    }

    /// What the veil's Workspaces card and the installer show, in the order
    /// the workspaces were made, so no card moves when another one opens.
    pub fn listing_json(&self, app_files: &Path, system_ready: SystemReady) -> String {
        let workspaces = &self.workspaces;
        serde_json::json!({
            "active": self.active,
            "openAtLaunch": self.open_at_launch,
            "setupReturn": self.setup_return(app_files, system_ready).map(|workspace| {
                serde_json::json!({ "id": workspace.id, "name": workspace.name })
            }),
            "workspaces": workspaces
                .iter()
                .map(|workspace| serde_json::json!({
                    "id": workspace.id,
                    "name": workspace.name,
                    "lastUsed": workspace.last_used_unix,
                    "needsSetup": !is_set_up(app_files, &workspace.id, system_ready),
                }))
                .collect::<Vec<_>>(),
        })
        .to_string()
    }

    pub fn set_open_at_launch(&mut self, id: Option<&str>) -> Result<(), WorkspaceError> {
        if let Some(id) = id {
            self.get_mut(id)?;
        }
        self.open_at_launch = id.map(str::to_owned);
        Ok(())
    }

    /// Reads the registry. A missing file is the implicit single-workspace
    /// registry; an unreadable or invalid one is an error.
    pub fn read(app_files: &Path) -> anyhow::Result<Self> {
        let path = registry_path(app_files);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::implicit())
            }
            Err(error) => return Err(error.into()),
        };
        Self::parse(&text)
    }

    /// Reads the registry, falling back to `main` when it cannot be used. A
    /// file that was read but is malformed is kept aside for diagnostics;
    /// one that could not be read at all is left alone for the next launch.
    pub fn load(app_files: &Path) -> Self {
        let path = registry_path(app_files);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::implicit(),
            Err(error) => {
                log::warn!(
                    "Workspace registry {} is unreadable ({error}); opening Main",
                    path.display()
                );
                return Self::implicit();
            }
        };
        match Self::parse(&text) {
            Ok(registry) => registry,
            Err(error) => {
                log::warn!(
                    "Workspace registry {} is malformed ({error}); opening Main",
                    path.display()
                );
                let aside = path.with_extension("json.invalid");
                if let Err(error) = fs::rename(&path, &aside) {
                    log::warn!("Could not set the malformed registry aside: {error}");
                }
                Self::implicit()
            }
        }
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let registry: Self = serde_json::from_str(text)?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn save(&self, app_files: &Path) -> anyhow::Result<()> {
        self.validate()?;
        let json = serde_json::to_vec_pretty(self)?;
        crate::core::provisioning::write_atomic(&registry_path(app_files), &json)
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The workspace directory this process runs; `None` until [`activate`].
static ACTIVE_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// The directory of the workspace this process runs. Before [`activate`],
/// and if it never succeeds, that is `main`: the app files directory, as
/// before workspaces existed.
pub fn active_dir(app_files: &Path) -> PathBuf {
    ACTIVE_DIR
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .unwrap_or_else(|| app_files.to_path_buf())
}

/// Points this process at one workspace's directory. Only call it while no
/// guest process runs.
pub fn set_active_dir(dir: PathBuf) {
    *ACTIVE_DIR
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(dir);
}

/// Serialises registry edits within this process: the veil and the switch
/// worker both read, change and save it.
static EDIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Reads the registry, applies one change and saves it. A registry that
/// cannot be read is never overwritten.
pub fn edit<T>(
    app_files: &Path,
    change: impl FnOnce(&mut Registry) -> Result<T, WorkspaceError>,
) -> anyhow::Result<(Registry, T)> {
    let _guard = EDIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut registry = Registry::read(app_files)?;
    let value = change(&mut registry)?;
    registry.save(app_files)?;
    Ok((registry, value))
}

/// Records `id` as the running workspace and points this process at it.
/// The guest must already be stopped.
pub fn switch_to(
    app_files: &Path,
    id: &str,
    now_unix: u64,
    system_ready: SystemReady,
) -> anyhow::Result<Workspace> {
    let (_, target) = edit(app_files, |registry| {
        let target = registry.switch_target(app_files, id, system_ready)?;
        // "Used 5 minutes ago" means when it was last on screen.
        let left = registry.active.clone();
        registry.get_mut(&left)?.last_used_unix = now_unix;
        registry.set_active(id, now_unix)?;
        Ok(target)
    })?;
    set_active_dir(workspace_dir(app_files, id));
    Ok(target)
}

/// Picks the workspace to open at launch and points this process at it.
///
/// A one-shot `open_next` wins once and is cleared. A launch target whose
/// directory has gone (deleted by hand, or a half-made workspace) is skipped
/// for `main` if it is still listed, else for the last one used that still
/// has its directory, so Portal never starts into nothing. With none left,
/// the target's folder is made again and its installer starts there. The
/// registry is written only when something changed.
pub fn activate(app_files: &Path, now_unix: u64) -> Workspace {
    let mut registry = Registry::load(app_files);
    let once = registry.open_next.take();
    let mut target = once
        .as_deref()
        .and_then(|id| registry.get(id))
        .unwrap_or_else(|| registry.launch_target())
        .clone();
    if !workspace_dir(app_files, &target.id).is_dir() {
        let fallback = registry
            .workspaces
            .iter()
            .filter(|workspace| workspace_dir(app_files, &workspace.id).is_dir())
            .max_by_key(|workspace| (workspace.id == MAIN_ID, workspace.last_used_unix));
        match fallback {
            Some(fallback) => {
                log::warn!(
                    "Workspace {} has no directory; opening {}",
                    target.id,
                    fallback.id
                );
                target = fallback.clone();
            }
            None => {
                if let Err(error) = fs::create_dir_all(workspace_dir(app_files, &target.id)) {
                    log::warn!("Could not remake workspace {}'s folder: {error}", target.id);
                }
            }
        }
    }
    if registry.active != target.id || once.is_some() {
        if registry.active != target.id {
            let _ = registry.set_active(&target.id, now_unix);
        }
        if let Err(error) = registry.save(app_files) {
            log::warn!("Could not record the active workspace: {error}");
        }
    }
    set_active_dir(workspace_dir(app_files, &target.id));
    target
}

/// Registers a new workspace with an empty folder and asks the next launch
/// to open it, where the first-run installer sets it up.
pub fn create(app_files: &Path, name: &str, now_unix: u64) -> anyhow::Result<Workspace> {
    let (_, workspace) = edit(app_files, |registry| {
        registry.create(app_files, name, now_unix)
    })?;
    Ok(workspace)
}

/// Asks the next launch to open `id` so its installer starts or resumes.
pub fn request_setup(
    app_files: &Path,
    id: &str,
    now_unix: u64,
    system_ready: SystemReady,
) -> anyhow::Result<Workspace> {
    let (_, workspace) = edit(app_files, |registry| {
        registry.request_setup(app_files, id, now_unix, system_ready)
    })?;
    Ok(workspace)
}

/// Gives up the running workspace's unfinished setup and deletes its files.
/// Returns the workspace the next launch opens. The registry is saved
/// before anything is deleted, so an interrupted delete leaves only an
/// unlisted folder, which no new workspace reuses.
pub fn abandon_setup(app_files: &Path, system_ready: SystemReady) -> anyhow::Result<Workspace> {
    let (_, (removed, back)) = edit(app_files, |registry| {
        registry.abandon_setup(app_files, system_ready)
    })?;
    if let Err(error) = delete_files(app_files, &removed.id) {
        log::warn!(
            "Workspace {} was given up but its files remain: {error:#}",
            removed.id
        );
    }
    Ok(back)
}

/// Deletes a workspace that isn't running, files and all; Main too, whose
/// install files leave the app files directory while Portal's own stay.
/// The registry is saved first, so an interrupted delete leaves only
/// unlisted files.
pub fn delete(app_files: &Path, id: &str) -> anyhow::Result<Workspace> {
    let (_, removed) = edit(app_files, |registry| registry.remove(id))?;
    delete_files(app_files, &removed.id)?;
    Ok(removed)
}

/// Deletes the running workspace's files and leaves it listed for a fresh
/// install ([`Registry::reset_active`]); Portal must restart into it. The
/// guest must already be stopped. `id` is the workspace the user chose, so
/// a switch that raced the delete never empties another one.
pub fn reset_running(app_files: &Path, id: &str, now_unix: u64) -> anyhow::Result<Workspace> {
    let (_, workspace) = edit(app_files, |registry| registry.reset_active(id, now_unix))?;
    delete_files(app_files, &workspace.id)?;
    // The next launch opens it only if its folder is there.
    fs::create_dir_all(workspace_dir(app_files, &workspace.id))?;
    Ok(workspace)
}

/// Deletes one workspace's files: its folder, or for Main (the app files
/// directory itself) only [`MAIN_INSTALL_ENTRIES`]. Directories the guest
/// left without write permission are opened up first, since a rootfs may
/// hold read-only trees.
pub fn delete_files(app_files: &Path, id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(is_valid_id(id), "refusing to delete workspace {id:?}");
    if id == MAIN_ID {
        return delete_main_files(app_files);
    }
    remove_tree(&workspace_dir(app_files, id))
}

/// Moves Main's install files into one trash folder, each in a single
/// rename, then deletes that. An interrupted delete leaves only the trash,
/// never a partial rootfs that could read as an install; the next delete
/// finishes it.
fn delete_main_files(app_files: &Path) -> anyhow::Result<()> {
    let trash = app_files.join(WORKSPACES_DIR).join(MAIN_TRASH_DIR);
    remove_tree(&trash)?;
    fs::create_dir_all(&trash)?;
    for entry in fs::read_dir(app_files)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str().is_some_and(is_main_install_entry) {
            fs::rename(entry.path(), trash.join(&name))?;
        }
    }
    remove_tree(&trash)
}

fn remove_tree(dir: &Path) -> anyhow::Result<()> {
    if fs::symlink_metadata(dir).is_err() || fs::remove_dir_all(dir).is_ok() {
        return Ok(());
    }
    open_up_directories(dir);
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[cfg(unix)]
fn open_up_directories(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        // symlink_metadata: a link is removed, never followed.
        let Ok(metadata) = fs::symlink_metadata(&dir) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let mode = metadata.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(mode | 0o700));
        }
        if let Ok(entries) = fs::read_dir(&dir) {
            pending.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
}

#[cfg(not(unix))]
fn open_up_directories(_dir: &Path) {}
