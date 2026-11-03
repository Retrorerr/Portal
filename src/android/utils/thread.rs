//! Named worker threads. Unnamed ones inherit `android_main` (or become
//! `Thread-N` once attached to the JVM), which hides them in `top -H`,
//! simpleperf and tombstones.

use std::thread::{self, JoinHandle};

/// `thread::spawn` with a name (at most 15 bytes are kept by the kernel).
pub fn spawn_named<F, T>(name: &str, body: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .unwrap_or_else(|error| panic!("failed to spawn thread {name}: {error}"))
}

/// Put back the current thread's name after a JNI attach. jni-rs attaches
/// without a name, so ART renames the native thread `Thread-N` (on every
/// attach, for threads that attach per call).
pub fn restore_name() {
    let current = thread::current();
    let Some(name) = current.name() else {
        return;
    };
    if let Ok(name) = std::ffi::CString::new(name) {
        // The kernel keeps the first 15 bytes.
        unsafe { libc::prctl(libc::PR_SET_NAME, name.as_ptr()) };
    }
}
