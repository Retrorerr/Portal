use jni::objects::{JObject, JValue};
use jni::sys::{_jobject, JNIInvokeInterface_};
use jni::{JNIEnv, JavaVM};
use winit::platform::android::activity::AndroidApp;

use crate::core::android_integration::density_scale_factor;

/// A higher-order function to run a provided JNI function within the JVM context.
pub fn run_in_jvm<F, T>(jni_function: F, android_app: AndroidApp) -> T
where
    F: FnOnce(&mut JNIEnv, &AndroidApp) -> T,
{
    // Set up JNI and gather the JavaVM
    let vm =
        unsafe { JavaVM::from_raw(android_app.vm_as_ptr() as *mut *const JNIInvokeInterface_) }
            .expect("Failed to get JavaVM");

    let mut env = vm.attach_current_thread().expect("Failed to attach thread");

    // Call the provided JNI function
    let res = jni_function(&mut env, &android_app);

    // Detach the current thread from the JVM
    unsafe { vm.detach_current_thread() };

    res
}

/// Recreate the NativeActivity after first-run provisioning so the same launch continues into
/// the Wayland backend without asking the user to close or restart the app.
pub fn recreate_activity(env: &mut JNIEnv, android_app: &AndroidApp) {
    let activity = unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _jobject) };
    if let Err(error) = env.call_method(activity, "recreate", "()V", &[]) {
        log::error!("Failed to recreate activity after setup: {error}");
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

/// Current Android display refresh rate expressed in Wayland mode units (millihertz).
pub fn refresh_rate_millihz(android_app: &AndroidApp) -> i32 {
    run_in_jvm(
        |env, app| {
            let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _jobject) };
            let window_manager = env
                .call_method(
                    activity,
                    "getWindowManager",
                    "()Landroid/view/WindowManager;",
                    &[],
                )
                .and_then(|value| value.l())
                .ok()?;
            #[allow(deprecated)]
            let display = env
                .call_method(
                    window_manager,
                    "getDefaultDisplay",
                    "()Landroid/view/Display;",
                    &[],
                )
                .and_then(|value| value.l())
                .ok()?;
            env.call_method(display, "getRefreshRate", "()F", &[])
                .and_then(|value| value.f())
                .ok()
        },
        android_app.clone(),
    )
    .map(|hz| (hz * 1000.0).round() as i32)
    .filter(|millihz| *millihz > 0)
    .unwrap_or(60_000)
}

/// Screen density in dpi, read from `Resources.getDisplayMetrics()`.
///
/// Prefer this over winit's `scale_factor()`: that one comes from `AConfiguration`, which the
/// native-activity glue builds from the asset manager at `onCreate` while density is still unset,
/// so it reports the 160 dpi default until the first configuration change.
pub fn density_dpi(android_app: &AndroidApp) -> i32 {
    run_in_jvm(
        |env, app| {
            let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _jobject) };
            let resources = env
                .call_method(
                    activity,
                    "getResources",
                    "()Landroid/content/res/Resources;",
                    &[],
                )
                .and_then(|it| it.l())
                .ok()?;
            let metrics = env
                .call_method(
                    resources,
                    "getDisplayMetrics",
                    "()Landroid/util/DisplayMetrics;",
                    &[],
                )
                .and_then(|it| it.l())
                .ok()?;
            env.get_field(&metrics, "densityDpi", "I")
                .and_then(|it| it.i())
                .ok()
        },
        android_app.clone(),
    )
    .unwrap_or(crate::core::android_integration::BASELINE_DPI as i32)
}

/// Guest UI scale factor derived from the device density, never below 1x.
pub fn scale_factor(android_app: &AndroidApp) -> f64 {
    density_scale_factor(density_dpi(android_app))
}

/// How far a finger may travel before the gesture counts as a scroll rather than a tap
/// (`ViewConfiguration.getScaledTouchSlop()`, already in physical pixels).
pub fn touch_slop_px(android_app: &AndroidApp) -> f64 {
    run_in_jvm(
        |env, app| {
            let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _jobject) };
            let config = env
                .call_static_method(
                    "android/view/ViewConfiguration",
                    "get",
                    "(Landroid/content/Context;)Landroid/view/ViewConfiguration;",
                    &[JValue::Object(&activity)],
                )
                .and_then(|it| it.l())
                .ok()?;
            env.call_method(config, "getScaledTouchSlop", "()I", &[])
                .and_then(|it| it.i())
                .ok()
        },
        android_app.clone(),
    )
    .map(|slop| slop as f64)
    .unwrap_or(24.0)
}

/// How long a finger must stay put to count as a long press
/// (`ViewConfiguration.getLongPressTimeout()`, 500 ms by default, tunable in accessibility
/// settings).
pub fn long_press_timeout_ms(android_app: &AndroidApp) -> u64 {
    run_in_jvm(
        |env, _| {
            env.call_static_method(
                "android/view/ViewConfiguration",
                "getLongPressTimeout",
                "()I",
                &[],
            )
            .and_then(|it| it.i())
            .ok()
        },
        android_app.clone(),
    )
    .map(|timeout| timeout.max(0) as u64)
    .unwrap_or(500)
}
