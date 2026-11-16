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
        Ok(())
    }

    fn name_taken(&self, name: &str, except: Option<&str>) -> bool {
        let name = name.to_lowercase();
        self.workspaces.iter().any(|workspace| {
            Some(workspace.id.as_str()) != except && workspace.name.to_lowercase() == name
        })
    }

    fn unique_id(&self, name: &str) -> String {
        let base = slug(name);
        let mut id = base.clone();
        let mut n = 2;
        // `main` is reserved for the app files directory itself.
        while id == MAIN_ID || self.get(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }

    /// Registers a new workspace. Its files are the caller's to create.
    pub fn add(&mut self, name: &str, now_unix: u64) -> Result<Workspace, WorkspaceError> {
        let name = normalise_name(name)?;
        if self.name_taken(&name, None) {
            return Err(WorkspaceError::NameTaken);
        }
        let workspace = Workspace {
            id: self.unique_id(&name),
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

    /// Unregisters a workspace that is not running. Deleting its files is the
    /// caller's job, after the registry without it has been saved.
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
        Ok(self.workspaces.remove(index))
    }

    pub fn set_active(&mut self, id: &str, now_unix: u64) -> Result<(), WorkspaceError> {
        self.get_mut(id)?.last_used_unix = now_unix;
        self.active = id.to_owned();
        Ok(())
    }

    /// The workspace a switch to `id` would open, if it can open.
    pub fn switch_target(&self, app_files: &Path, id: &str) -> Result<Workspace, WorkspaceError> {
        let target = self
            .get(id)
            .ok_or_else(|| WorkspaceError::Unknown(id.to_owned()))?;
        if self.active == id {
            return Err(WorkspaceError::AlreadyActive);
        }
        if !workspace_rootfs(app_files, id).is_dir() {
            return Err(WorkspaceError::FilesMissing);
        }
        Ok(target.clone())
    }

    /// What the veil's Workspaces card shows, in the order the workspaces
    /// were made, so no card moves when another one opens.
    pub fn listing_json(&self) -> String {
        let workspaces = &self.workspaces;
        serde_json::json!({
            "active": self.active,
            "openAtLaunch": self.open_at_launch,
            "workspaces": workspaces
                .iter()
                .map(|workspace| serde_json::json!({
                    "id": workspace.id,
                    "name": workspace.name,
                    "lastUsed": workspace.last_used_unix,
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
pub fn switch_to(app_files: &Path, id: &str, now_unix: u64) -> anyhow::Result<Workspace> {
    let (_, target) = edit(app_files, |registry| {
        let target = registry.switch_target(app_files, id)?;
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
/// A launch target other than `main` whose directory has gone (deleted by
/// hand, or a half-made workspace) is skipped for `main`, which always
/// exists, so Portal never starts into nothing. The registry is written only
/// when the active workspace changed.
pub fn activate(app_files: &Path, now_unix: u64) -> Workspace {
    let mut registry = Registry::load(app_files);
    let mut target = registry.launch_target().clone();
    if target.id != MAIN_ID && !workspace_dir(app_files, &target.id).is_dir() {
        log::warn!("Workspace {} has no directory; opening Main", target.id);
        if registry.get(MAIN_ID).is_some() {
            target = registry.get(MAIN_ID).cloned().expect("checked above");
        }
    }
    if registry.active != target.id {
        let _ = registry.set_active(&target.id, now_unix);
        if let Err(error) = registry.save(app_files) {
            log::warn!("Could not record the active workspace: {error}");
        }
    }
    set_active_dir(workspace_dir(app_files, &target.id));
    target
}
