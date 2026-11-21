use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[cfg(any(target_os = "android", test))]
#[path = "runtime/output.rs"]
pub(crate) mod output;

/// Callback for streaming lines of process stdout/stderr.
pub type LogCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Represents a bind mount mapping from host filesystem to guest rootfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindMount {
    pub host_path: PathBuf,
    pub guest_path: PathBuf,
}

impl BindMount {
    pub fn new(host_path: impl Into<PathBuf>, guest_path: impl Into<PathBuf>) -> Self {
        Self {
            host_path: host_path.into(),
            guest_path: guest_path.into(),
        }
    }
}

/// Specification for launching a guest process inside a Linux runtime.
#[derive(Debug, Clone)]
pub struct ProcessSpec {
    /// Command string to execute (typically executed via guest shell).
    pub command: String,
    /// User to execute as (None defaults to "root").
    pub user: Option<String>,
    /// Explicit environment variables to append or override.
    pub env: HashMap<String, String>,
    /// Additional bind mounts specific to this process invocation.
    pub extra_binds: Vec<BindMount>,
    /// Start the guest's system services (the D-Bus system bus, and polkit
    /// and PackageKit through it) as root before switching to `user`, and
    /// stop them when the command exits. For the desktop session only.
    pub system_services: bool,
}

impl ProcessSpec {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            user: None,
            env: HashMap::new(),
            extra_binds: Vec::new(),
            system_services: false,
        }
    }

    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

/// Represents an A/B or versioned runtime slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSlot {
    pub id: String,
    pub rootfs_path: PathBuf,
    pub distro_name: String,
    pub is_active: bool,
}

/// A Linux guest that runs processes: PRoot or tawcroot (`PRootRuntime`).
pub trait LinuxRuntime: Send + Sync {
    /// Path to the guest root filesystem on host storage.
    fn rootfs_path(&self) -> &Path;

    /// Execute a guest process with bounded output capture, streaming log callbacks,
    /// and graceful cancellation support via process-group signaling.
    fn execute(
        &self,
        spec: ProcessSpec,
        log: Option<LogCallback>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Output;
}

/// Layout of versioned runtime slots and shared storage.
#[derive(Debug, Clone)]
pub struct RuntimeLayout {
    pub base_dir: PathBuf,
    pub platform_state: PathBuf,
}

impl RuntimeLayout {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let base = base_dir.into();
        Self {
            platform_state: base.join("platform-state"),
            base_dir: base,
        }
    }

    pub fn slot_b(&self) -> RuntimeSlot {
        RuntimeSlot {
            id: "slot-b".to_string(),
            rootfs_path: self.base_dir.join("runtime-B"),
            distro_name: "Debian 13 (Trixie) ARM64".to_string(),
            is_active: false,
        }
    }

    /// Debian is the production runtime even on fresh app data or old Arch installs.
    pub fn active_slot(&self) -> RuntimeSlot {
        let mut slot = self.slot_b();
        slot.is_active = true;
        slot
    }

    pub fn set_active_slot(&self, slot_id: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.platform_state)?;
        let state_file = self.platform_state.join("active-slot");
        std::fs::write(state_file, slot_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_spec_builder_sets_expected_fields() {
        let spec = ProcessSpec::new("ls -la")
            .with_user("desktop")
            .with_env("DISPLAY", ":0");

        assert_eq!(spec.command, "ls -la");
        assert_eq!(spec.user.as_deref(), Some("desktop"));
        assert_eq!(spec.env.get("DISPLAY").map(|s| s.as_str()), Some(":0"));
        assert!(spec.extra_binds.is_empty());
    }

    #[test]
    fn runtime_layout_defaults_to_debian() {
        let temp_dir = std::env::temp_dir().join(format!("portal-test-{}", std::process::id()));
        let layout = RuntimeLayout::new(&temp_dir);
        let active = layout.active_slot();
        assert_eq!(active.id, "slot-b");
        assert_eq!(active.distro_name, "Debian 13 (Trixie) ARM64");
        assert!(active.is_active);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
