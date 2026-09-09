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
        let runtime = PRootRuntime::active();
        let mut spec = ProcessSpec::new(self.command);
        if let Some(user) = self.user {
            spec = spec.with_user(user);
        }
        runtime.execute(spec, self.log, None)
    }

    /// Run a guest process that can be cancelled by its lifecycle owner.
    pub fn run_with_cancel(self, cancel: Arc<AtomicBool>) -> Output {
        self.run_with_cancel_and_env(cancel, std::iter::empty::<(String, String)>())
    }

    /// Run a guest process with lifecycle cancellation and explicit environment
    /// variables supplied by the Android host.
    pub fn run_with_cancel_and_env<I>(self, cancel: Arc<AtomicBool>, environment: I) -> Output
    where
        I: IntoIterator<Item = (String, String)>,
    {
        self.run_with_cancel_env_and_binds(
            cancel,
            environment,
            std::iter::empty::<crate::core::runtime::BindMount>(),
        )
    }

    /// Run a guest process with lifecycle cancellation, explicit environment
    /// variables and additional bind mounts (used by the Anland GPU session
    /// for its broker-socket dir and Mesa overlay; empty for QPainter).
    pub fn run_with_cancel_env_and_binds<I, B>(
        self,
        cancel: Arc<AtomicBool>,
        environment: I,
        binds: B,
    ) -> Output
    where
        I: IntoIterator<Item = (String, String)>,
        B: IntoIterator<Item = crate::core::runtime::BindMount>,
    {
        let runtime = PRootRuntime::active();
        let mut spec = ProcessSpec::new(self.command);
        if let Some(user) = self.user {
            spec = spec.with_user(user);
        }
        for (key, value) in environment {
            spec = spec.with_env(key, value);
        }
        for bind in binds {
            spec.extra_binds.push(bind);
        }
        runtime.execute(spec, self.log, Some(cancel))
    }
}
