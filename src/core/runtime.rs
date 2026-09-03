use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Callback for streaming lines of process stdout/stderr.
pub type LogCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Represents a bind mount mapping from host filesystem to guest rootfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindMount {
    pub host_path: PathBuf,
    pub guest_path: PathBuf,
    pub readonly: bool,
}

impl BindMount {
    pub fn new(host_path: impl Into<PathBuf>, guest_path: impl Into<PathBuf>) -> Self {
        Self {
            host_path: host_path.into(),
            guest_path: guest_path.into(),
            readonly: false,
        }
    }

    pub fn readonly(host_path: impl Into<PathBuf>, guest_path: impl Into<PathBuf>) -> Self {
        Self {
            host_path: host_path.into(),
            guest_path: guest_path.into(),
            readonly: true,
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
    /// Working directory inside guest.
    pub working_dir: Option<PathBuf>,
    /// Explicit environment variables to append or override.
    pub env: HashMap<String, String>,
    /// Additional bind mounts specific to this process invocation.
    pub extra_binds: Vec<BindMount>,
}

impl ProcessSpec {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            user: None,
            working_dir: None,
            env: HashMap::new(),
            extra_binds: Vec::new(),
        }
    }

    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    pub fn with_working_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(path.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn with_bind(mut self, host_path: impl Into<PathBuf>, guest_path: impl Into<PathBuf>) -> Self {
        self.extra_binds.push(BindMount::new(host_path, guest_path));
        self
    }
}

/// Status of runtime health check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeHealth {
    Healthy,
    Degraded(String),
    Unsupported(String),
    MissingRootfs(PathBuf),
}

/// Represents an A/B or versioned runtime slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSlot {
    pub id: String,
    pub rootfs_path: PathBuf,
    pub distro_name: String,
    pub is_active: bool,
}

/// Abstract Linux runtime interface.
///
/// Implementations encapsulate the container or emulation mechanism (PRoot, future Tawcroot, etc.)
/// and provide uniform lifecycle, process execution, environment, bind mounts, and signaling.
pub trait LinuxRuntime: Send + Sync {
    /// Human-readable name of the runtime engine (e.g. "proot", "tawcroot").
    fn engine_name(&self) -> &'static str;

    /// Path to the guest root filesystem on host storage.
    fn rootfs_path(&self) -> &Path;

    /// Inspect runtime health and whether the rootfs is ready for execution.
    fn check_health(&self) -> RuntimeHealth;

    /// Execute a guest process with bounded output capture, streaming log callbacks,
    /// and graceful cancellation support via process-group signaling.
    fn execute(
        &self,
        spec: ProcessSpec,
        log: Option<LogCallback>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Output;

    /// Request graceful termination of running guest processes in this runtime.
    fn terminate(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_spec_builder_sets_expected_fields() {
        let spec = ProcessSpec::new("ls -la")
            .with_user("desktop")
            .with_working_dir("/tmp")
            .with_env("DISPLAY", ":0")
            .with_bind("/sdcard", "/android");

        assert_eq!(spec.command, "ls -la");
        assert_eq!(spec.user.as_deref(), Some("desktop"));
        assert_eq!(spec.working_dir, Some(PathBuf::from("/tmp")));
        assert_eq!(spec.env.get("DISPLAY").map(|s| s.as_str()), Some(":0"));
        assert_eq!(spec.extra_binds.len(), 1);
        assert_eq!(spec.extra_binds[0].host_path, PathBuf::from("/sdcard"));
        assert_eq!(spec.extra_binds[0].guest_path, PathBuf::from("/android"));
        assert!(!spec.extra_binds[0].readonly);
    }

    #[test]
    fn bind_mount_readonly_flag() {
        let b = BindMount::readonly("/sys", "/sys");
        assert!(b.readonly);
    }
}
