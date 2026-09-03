use std::process::Output;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::android::runtime::proot::PRootRuntime;
use crate::core::runtime::{LinuxRuntime, LogCallback, ProcessSpec};
use winit::platform::android::activity::AndroidApp;

pub type Log = LogCallback;

/// Legacy wrapper for guest process execution in Arch Linux rootfs.
/// Backed by `PRootRuntime` via the `LinuxRuntime` abstraction.
pub struct ArchProcess {
    pub command: String,
    pub user: Option<String>,
    pub log: Option<Log>,
}

impl ArchProcess {
    pub fn is_supported(android_app: &AndroidApp) -> bool {
        PRootRuntime::is_supported(android_app)
    }

    /// Run a guest process and return its complete bounded output.
    pub fn run(self) -> Output {
        let runtime = PRootRuntime::default_arch();
        let mut spec = ProcessSpec::new(self.command);
        if let Some(user) = self.user {
            spec = spec.with_user(user);
        }
        runtime.execute(spec, self.log, None)
    }

    /// Run a guest process that can be cancelled by its lifecycle owner.
    pub fn run_with_cancel(self, cancel: Arc<AtomicBool>) -> Output {
        let runtime = PRootRuntime::default_arch();
        let mut spec = ProcessSpec::new(self.command);
        if let Some(user) = self.user {
            spec = spec.with_user(user);
        }
        runtime.execute(spec, self.log, Some(cancel))
    }
}
