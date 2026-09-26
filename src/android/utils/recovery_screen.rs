//! Rust side of the Compose recovery screen (`app.polarbear.RecoveryScreen`).
//!
//! Shown when Plasma fails after installation, or when the device or Android user profile cannot
//! run the guest. It is its own sibling View above the native SurfaceView in the same window, as
//! the setup overlay is, and independent of that overlay's veil state machine. Its buttons call
//! the JNI entry points below directly; there is no WebView, localhost socket or token.

use crate::android::backend::webview::{request_action, WebviewAction};
use jni::{
    objects::{JClass, JObject, JValue},
    sys::_jobject,
    JNIEnv,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use winit::platform::android::activity::AndroidApp;

const RECOVERY_DOTTED_CLASS: &str = "app.polarbear.RecoveryScreen";

/// What the screen offers, matching `RecoveryScreen.KIND_*` on the Kotlin side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryKind {
    /// Plasma failed after installation: Retry Plasma and Export diagnostics.
    Runtime = 0,
    /// The device or profile cannot run the guest: Export diagnostics only.
    Unsupported = 1,
}

/// Activity used by the diagnostics export, which runs off the event loop.
static APP: OnceLock<Mutex<Option<AndroidApp>>> = OnceLock::new();
static EXPORT_RUNNING: AtomicBool = AtomicBool::new(false);

fn app_slot() -> &'static Mutex<Option<AndroidApp>> {
    APP.get_or_init(|| Mutex::new(None))
}

fn activity_object(android_app: &AndroidApp) -> JObject<'static> {
    unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _jobject) }
}

/// Resolve the screen class through the Activity's class loader: the attached event-loop
/// thread only sees the system loader through plain `FindClass`.
fn recovery_class<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject,
) -> jni::errors::Result<JClass<'local>> {
    let loader = env
        .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
        .l()?;
    let name = env.new_string(RECOVERY_DOTTED_CLASS)?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )?
        .l()?;
    Ok(JClass::from(class))
}

fn call_static(android_app: &AndroidApp, method: &'static str, build: CallArgs) {
    super::ndk::run_in_jvm(
        move |env, app| {
            let activity = activity_object(app);
            let result = recovery_class(env, &activity).and_then(|class| match &build {
                CallArgs::Show { kind, reason } => {
                    let reason = env.new_string(reason)?;
                    env.call_static_method(
                        class,
                        method,
                        "(Landroid/app/Activity;ILjava/lang/String;)V",
                        &[
                            JValue::Object(&activity),
                            JValue::Int(*kind as i32),
                            JValue::Object(&reason),
                        ],
                    )
                }
                CallArgs::Activity => env.call_static_method(
                    class,
                    method,
                    "(Landroid/app/Activity;)V",
                    &[JValue::Object(&activity)],
                ),
                CallArgs::ExportResult { ok, message } => {
                    let message = env.new_string(message)?;
                    env.call_static_method(
                        class,
                        method,
                        "(ZLjava/lang/String;)V",
                        &[JValue::Bool((*ok).into()), JValue::Object(&message)],
                    )
                }
            });
            if let Err(error) = result {
                log::error!("Recovery screen {method} failed: {error}");
                if env.exception_check().unwrap_or(false) {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                }
            }
        },
        android_app.clone(),
    );
}

enum CallArgs {
    Show { kind: RecoveryKind, reason: String },
    Activity,
    ExportResult { ok: bool, message: String },
}

/// Show the recovery screen, or update it in place when it is already up.
pub fn show(android_app: &AndroidApp, kind: RecoveryKind, reason: &str) {
    if let Ok(mut app) = app_slot().lock() {
        *app = Some(android_app.clone());
    }
    call_static(
        android_app,
        "show",
        CallArgs::Show {
            kind,
            reason: reason.to_string(),
        },
    );
}

/// Remove the recovery screen. Safe to call when it is not showing.
pub fn hide(android_app: &AndroidApp) {
    call_static(android_app, "hide", CallArgs::Activity);
}

/// JNI: the user tapped Retry Plasma. The event loop performs the retry.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_RecoveryScreen_nativeRetryPlasma(
    _env: JNIEnv,
    _class: JObject,
) {
    log::info!("Recovery screen: Retry Plasma requested");
    request_action(WebviewAction::RetryPlasma);
}

/// JNI: the user tapped Export diagnostics. Builds the archive on a worker thread, opens the
/// Android share sheet, and reports the outcome back to the screen.
#[no_mangle]
pub extern "system" fn Java_app_polarbear_RecoveryScreen_nativeExportDiagnostics(
    _env: JNIEnv,
    _class: JObject,
) {
    let Some(app) = app_slot().lock().ok().and_then(|app| app.clone()) else {
        log::warn!("Recovery screen: diagnostics export requested before an activity attached");
        return;
    };
    if EXPORT_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        let (ok, message) = match crate::android::diagnostics::export_and_share(&app) {
            // The share sheet is open; the archive was also published to Downloads.
            Ok(_) => (true, "Saved to Download/Portal".to_string()),
            Err(error) => {
                log::warn!("Failed to export diagnostics: {error}");
                (false, format!("Export failed: {error}"))
            }
        };
        EXPORT_RUNNING.store(false, Ordering::Release);
        call_static(&app, "onExportResult", CallArgs::ExportResult { ok, message });
    });
}
