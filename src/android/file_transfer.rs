//! Binary Android descriptors are handed to the existing guest supervisor;
//! filenames and paths travel only in the structured helper request.
use std::collections::HashMap;
use std::fs::File;
use std::os::fd::FromRawFd;
use std::sync::{atomic::AtomicBool, Arc, Mutex, OnceLock};

use jni::{
    objects::{JClass, JString, JValue},
    sys::{jint, jlong},
    JNIEnv,
};
use serde_json::{json, Value};

use super::runtime::proot::PRootRuntime;
use crate::core::runtime::{LogCallback, ProcessSpec};

fn jobs() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn duplicate(fd: jint) -> std::io::Result<Option<File>> {
    if fd < 0 {
        return Ok(None);
    }
    let owned = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if owned < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Some(unsafe { File::from_raw_fd(owned) }))
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_FileTransfers_nativeRun(
    mut env: JNIEnv,
    class: JClass,
    request: JString,
    input_fd: jint,
    output_fd: jint,
    job: JString,
) -> jni::sys::jstring {
    let response = (|| -> Result<Value, String> {
        let request: String = env.get_string(&request).map_err(|e| e.to_string())?.into();
        let job: String = env.get_string(&job).map_err(|e| e.to_string())?.into();
        let value: Value = serde_json::from_str(&request).map_err(|e| e.to_string())?;
        if !matches!(
            value["action"].as_str(),
            Some("defaults" | "list" | "import" | "commit" | "export" | "cleanup")
        ) {
            return Err("Unknown file operation".into());
        }
        if value["action"] == "import" && input_fd < 0
            || value["action"] == "export" && output_fd < 0
        {
            return Err("Missing file stream".into());
        }
        let input = duplicate(input_fd).map_err(|e| e.to_string())?;
        let output = duplicate(output_fd).map_err(|e| e.to_string())?;
        let binary = output.is_some();
        let cancel = jobs()
            .lock()
            .map_err(|e| e.to_string())?
            .entry(job.clone())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone();
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            if let Ok(mut jobs) = jobs().lock() {
                jobs.remove(&job);
            }
            return Err("Transfer canceled".into());
        }
        let progress = (|| {
            let vm = env.get_java_vm().ok()?;
            let class = env.new_global_ref(&class).ok()?;
            let job = job.clone();
            Some(Arc::new(move |line: String| {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    return;
                };
                let Some(bytes) = value["bytes"].as_i64() else {
                    return;
                };
                let Ok(mut env) = vm.attach_current_thread() else {
                    return;
                };
                crate::android::utils::thread::restore_name();
                let Ok(job) = env.new_string(&job) else {
                    return;
                };
                let class: &JClass = class.as_obj().into();
                let _ = env.call_static_method(
                    class,
                    "onNativeProgress",
                    "(Ljava/lang/String;JJ)V",
                    &[
                        JValue::Object(&job),
                        JValue::Long(bytes as jlong),
                        JValue::Long(value["total"].as_i64().unwrap_or(-1) as jlong),
                    ],
                );
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_clear();
                }
            }) as LogCallback)
        })();
        let result = PRootRuntime::active().execute_with_streams(
            ProcessSpec::new("exec /usr/bin/python3 /usr/local/lib/portal/portal-file-transfer.py")
                .with_user("desktop")
                .with_env("PORTAL_FILE_REQUEST", request),
            input,
            output,
            None,
            progress.clone(),
            Some(cancel),
        );
        if let Ok(mut jobs) = jobs().lock() {
            jobs.remove(&job);
        }
        if result.status.success() {
            if binary {
                Ok(String::from_utf8_lossy(&result.stderr)
                    .lines()
                    .rev()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .find(|value| value["ok"] == true)
                    .unwrap_or_else(|| json!({"ok": true})))
            } else {
                serde_json::from_slice(&result.stdout)
                    .map_err(|e| format!("Invalid file helper response: {e}"))
            }
        } else {
            let error = String::from_utf8_lossy(&result.stderr)
                .lines()
                .rev()
                .find_map(|line| {
                    serde_json::from_str::<Value>(line)
                        .ok()
                        .and_then(|v| v["error"].as_str().map(str::to_owned))
                })
                .unwrap_or_else(|| {
                    "Could not transfer the file. Open Portal and try again.".into()
                });
            Err(error)
        }
    })()
    .unwrap_or_else(|error| json!({"ok": false, "error": error}));
    env.new_string(response.to_string())
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_FileTransfers_nativeCancel(
    mut env: JNIEnv,
    _class: JClass,
    job: JString,
) {
    if let Ok(job) = env.get_string(&job) {
        if let Ok(mut jobs) = jobs().lock() {
            jobs.entry(job.to_string_lossy().into_owned())
                .or_insert_with(|| Arc::new(AtomicBool::new(false)))
                .store(true, std::sync::atomic::Ordering::Release);
        }
    };
}

#[no_mangle]
pub extern "system" fn Java_app_polarbear_FileTransfers_nativeForget(
    mut env: JNIEnv,
    _class: JClass,
    job: JString,
) {
    if let Ok(job) = env.get_string(&job) {
        if let Ok(mut jobs) = jobs().lock() {
            jobs.remove(job.to_str().unwrap_or_default());
        }
    };
}
