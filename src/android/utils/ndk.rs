use jni::objects::{JObject, JObjectArray, JValue};
use jni::sys::{_jobject, JNIInvokeInterface_};
use jni::{JNIEnv, JavaVM};
use winit::platform::android::activity::AndroidApp;

use crate::core::android_integration::{
    density_scale_factor, select_preferred_refresh_millihz,
};

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

    // Call the provided JNI function. `AttachGuard` owns the attachment and
    // detaches it on drop when this thread was not already attached. Calling
    // `JavaVM::detach_current_thread` here is unsafe: for a nested attach it
    // clears the outer guard owned by the NativeActivity thread and leaves
    // subsequent callbacks with an invalid JNIEnv.
    let res = jni_function(&mut env, &android_app);

    // Do not let a failed lookup/call leak a pending Java exception into the
    // next callback on this worker. Individual helpers still return their
    // contextual error, while this boundary prevents an uncleared exception
    // from poisoning unrelated JNI calls.
    if env.exception_check().unwrap_or(false) {
        log::error!("JNI callback returned with a pending Java exception; clearing it");
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }

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
///
/// This is the legacy `Display.getRefreshRate()` reading. New code should prefer
/// [`active_refresh_millihz`], which consults the active `Display.Mode` first so
/// VRR switches (50/60/90/120/144 Hz on the OnePlus Pad 3) are observed instead
/// of a stale startup snapshot.
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

/// Active display refresh in millihertz, consulting `Display.getMode()` first.
///
/// `Display.getRefreshRate()` and `Display.Mode.getRefreshRate()` should agree,
/// but the `Mode` path names the exact active mode (id + physical size) and is
/// the documented way to observe refresh switches. Falls back to
/// `getRefreshRate()` on API < 23 or on any JNI failure. Never panics; returns
/// 60_000 on total failure so callers preserve last valid instead.
pub fn active_refresh_millihz(android_app: &AndroidApp) -> i32 {
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
            // Prefer the active Mode (API 23+): exact mode refresh, not a snapshot.
            if let Ok(mode) = env
                .call_method(&display, "getMode", "()Landroid/view/Display$Mode;", &[])
                .and_then(|value| value.l())
            {
                if !mode.is_null() {
                    if let Ok(hz) = env
                        .call_method(&mode, "getRefreshRate", "()F", &[])
                        .and_then(|value| value.f())
                    {
                        if hz.is_finite() && hz > 0.0 {
                            return Some(hz);
                        }
                    }
                    let _ = env.exception_clear();
                } else {
                    let _ = env.exception_clear();
                }
            } else {
                let _ = env.exception_clear();
            }
            env.call_method(&display, "getRefreshRate", "()F", &[])
                .and_then(|value| value.f())
                .ok()
        },
        android_app.clone(),
    )
    .map(|hz| (hz * 1000.0).round() as i32)
    .filter(|millihz| *millihz > 0)
    .unwrap_or(60_000)
}

/// Supported display refresh rates in millihertz, from `Display.getSupportedModes()`.
///
/// Portable enumeration (API 23+): each `Display.Mode` contributes its
/// `getRefreshRate()`. Best-effort; returns an empty vec on API < 23, JNI
/// failure, or a null/empty mode array so callers fall back to the active
/// rate and finally to the sane default. Never panics, never contains
/// non-positive entries (those are filtered here; full validity filtering
/// stays in [`select_preferred_refresh_millihz`]).
pub fn supported_refresh_rates_millihz(android_app: &AndroidApp) -> Vec<i32> {
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
            let modes = env
                .call_method(
                    &display,
                    "getSupportedModes",
                    "()[Landroid/view/Display$Mode;",
                    &[],
                )
                .and_then(|value| value.l())
                .ok()?;
            if modes.is_null() {
                let _ = env.exception_clear();
                return None;
            }
            // `getSupportedModes()` returns `Display.Mode[]`: rewrap the
            // object reference as a `JObjectArray` for length/element access
            // (jni 0.21 types array operations on `JObjectArray`, not `JObject`).
            let modes_array = JObjectArray::from(modes);
            let len = env.get_array_length(&modes_array).ok()?;
            let _ = env.exception_clear();
            if len <= 0 {
                return Some(Vec::new());
            }
            let mut rates = Vec::new();
            for i in 0..len {
                let mode_obj = env.get_object_array_element(&modes_array, i).ok()?;
                if mode_obj.is_null() {
                    let _ = env.exception_clear();
                    continue;
                }
                let hz: f32 = env
                    .call_method(&mode_obj, "getRefreshRate", "()F", &[])
                    .and_then(|v| v.f())
                    .unwrap_or(0.0);
                let _ = env.exception_clear();
                if hz.is_finite() && hz > 0.0 {
                    rates.push((hz * 1000.0).round() as i32);
                }
            }
            Some(rates)
        },
        android_app.clone(),
    )
    .unwrap_or_default()
    .into_iter()
    .filter(|millihz| *millihz > 0)
    .collect()
}

/// Preferred stable high-refresh target in millihertz.
///
/// Resolves [`select_preferred_refresh_millihz`] over
/// `Display.getSupportedModes()` so a 144 Hz panel yields 144000 while
/// 120/90/60 Hz devices yield their own maximum. Falls back to the active
/// rate when enumeration is empty/unusable, and finally to the sane default.
/// Never panics; always returns a valid millihertz value.
pub fn preferred_high_refresh_millihz(android_app: &AndroidApp) -> i32 {
    let supported = supported_refresh_rates_millihz(android_app);
    if supported
        .iter()
        .any(|rate| crate::core::android_integration::is_valid_refresh_millihz(*rate))
    {
        return select_preferred_refresh_millihz(&supported);
    }
    let active = active_refresh_millihz(android_app);
    if crate::core::android_integration::is_valid_refresh_millihz(active) {
        return active.max(60_000);
    }
    crate::core::android_integration::NOMINAL_OUTPUT_REFRESH_MILLIHZ
}

/// Log the active display mode plus the supported modes once per resume.
///
/// This is the on-device counterpart to `adb shell dumpsys display` /
/// SurfaceFlinger evidence: it shows which mode is active after the frame-rate
/// hint and which refresh rates the panel reports as supported. Best-effort;
/// never fails the caller.
pub fn log_display_modes(android_app: &AndroidApp) {
    run_in_jvm(
        |env, app| {
            let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _jobject) };
            let window_manager = match env
                .call_method(
                    activity,
                    "getWindowManager",
                    "()Landroid/view/WindowManager;",
                    &[],
                )
                .and_then(|value| value.l())
            {
                Ok(wm) => wm,
                Err(_) => {
                    let _ = env.exception_clear();
                    return;
                }
            };
            #[allow(deprecated)]
            let display = match env
                .call_method(
                    window_manager,
                    "getDefaultDisplay",
                    "()Landroid/view/Display;",
                    &[],
                )
                .and_then(|value| value.l())
            {
                Ok(d) => d,
                Err(_) => {
                    let _ = env.exception_clear();
                    return;
                }
            };
            let active_hz: f32 = env
                .call_method(&display, "getRefreshRate", "()F", &[])
                .and_then(|v| v.f())
                .unwrap_or(0.0);
            let _ = env.exception_clear();
            // Active mode details (API 23+): mode id + physical size + refresh.
            let mut mode_detail = String::from("unavailable");
            if let Ok(mode) = env
                .call_method(&display, "getMode", "()Landroid/view/Display$Mode;", &[])
                .and_then(|v| v.l())
            {
                if !mode.is_null() {
                    let hz = env
                        .call_method(&mode, "getRefreshRate", "()F", &[])
                        .and_then(|v| v.f())
                        .unwrap_or(0.0);
                    let w = env
                        .call_method(&mode, "getPhysicalWidth", "()I", &[])
                        .and_then(|v| v.i())
                        .unwrap_or(0);
                    let h = env
                        .call_method(&mode, "getPhysicalHeight", "()I", &[])
                        .and_then(|v| v.i())
                        .unwrap_or(0);
                    let id = env
                        .call_method(&mode, "getModeId", "()I", &[])
                        .and_then(|v| v.i())
                        .unwrap_or(-1);
                    let _ = env.exception_clear();
                    mode_detail = format!("id={id} {w}x{h}@{hz:.2}Hz");
                } else {
                    let _ = env.exception_clear();
                }
            } else {
                let _ = env.exception_clear();
            }
            // Supported modes (API 23+): compact `WxH@Hz` list for target
            // selection evidence. Failures leave the placeholder text.
            let mut supported_detail = String::from("unavailable");
            let mut supported_rates: Vec<i32> = Vec::new();
            if let Ok(modes_obj) = env
                .call_method(
                    &display,
                    "getSupportedModes",
                    "()[Landroid/view/Display$Mode;",
                    &[],
                )
                .and_then(|v| v.l())
            {
                if !modes_obj.is_null() {
                    let modes_array = JObjectArray::from(modes_obj);
                    if let Ok(len) = env.get_array_length(&modes_array) {
                        let _ = env.exception_clear();
                        let mut parts = Vec::new();
                        for i in 0..len {
                            let Ok(mode_obj) =
                                env.get_object_array_element(&modes_array, i)
                            else {
                                let _ = env.exception_clear();
                                continue;
                            };
                            if mode_obj.is_null() {
                                let _ = env.exception_clear();
                                continue;
                            }
                            let hz = env
                                .call_method(&mode_obj, "getRefreshRate", "()F", &[])
                                .and_then(|v| v.f())
                                .unwrap_or(0.0);
                            let w = env
                                .call_method(&mode_obj, "getPhysicalWidth", "()I", &[])
                                .and_then(|v| v.i())
                                .unwrap_or(0);
                            let h = env
                                .call_method(&mode_obj, "getPhysicalHeight", "()I", &[])
                                .and_then(|v| v.i())
                                .unwrap_or(0);
                            let _ = env.exception_clear();
                            if hz.is_finite() && hz > 0.0 && w > 0 && h > 0 {
                                parts.push(format!("{w}x{h}@{hz:.2}Hz"));
                                supported_rates.push((hz * 1000.0).round() as i32);
                            }
                        }
                        if !parts.is_empty() {
                            supported_detail = parts.join(",");
                        } else {
                            supported_detail = String::from("empty");
                        }
                    } else {
                        let _ = env.exception_clear();
                    }
                } else {
                    let _ = env.exception_clear();
                }
            } else {
                let _ = env.exception_clear();
            }
            // Preferred target evidence: what the nominal `wl_output` mode and
            // the frame-rate hint will use (highest valid supported rate).
            let preferred_millihz =
                crate::core::android_integration::select_preferred_refresh_millihz(
                    &supported_rates,
                );
            log::info!(
                "display.modes active={active_hz:.2}Hz mode=[{mode_detail}] supported=[{supported_detail}] preferred_millihz={preferred_millihz}"
            );
            crate::android::diagnostics::host_event(
                "display-modes",
                &format!(
                    "active_hz={active_hz:.2} mode=[{mode_detail}] supported=[{supported_detail}] preferred_millihz={preferred_millihz}"
                ),
            );
        },
        android_app.clone(),
    );
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

/// Control Android system cursor visibility.
/// Setting to false (TYPE_NULL) hides Android's native pointer icon so only KWin's
/// cursor is visible. Setting to true restores the default system pointer icon.
pub fn set_android_system_cursor_visible(android_app: &AndroidApp, visible: bool) {
    run_in_jvm(
        |env, app| {
            let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as *mut _jobject) };
            let window =
                match env.call_method(&activity, "getWindow", "()Landroid/view/Window;", &[]) {
                    Ok(v) => match v.l() {
                        Ok(w) => w,
                        Err(e) => {
                            log::warn!("Failed to get window: {e}");
                            return;
                        }
                    },
                    Err(e) => {
                        log::warn!("Failed to call getWindow: {e}");
                        return;
                    }
                };
            let decor_view =
                match env.call_method(&window, "getDecorView", "()Landroid/view/View;", &[]) {
                    Ok(v) => match v.l() {
                        Ok(dv) => dv,
                        Err(e) => {
                            log::warn!("Failed to get decorView: {e}");
                            return;
                        }
                    },
                    Err(e) => {
                        log::warn!("Failed to call getDecorView: {e}");
                        return;
                    }
                };
            let icon_type: i32 = if visible { 1000 } else { 0 }; // 1000 = TYPE_DEFAULT, 0 = TYPE_NULL
            let pointer_icon = match env.call_static_method(
                "android/view/PointerIcon",
                "getSystemIcon",
                "(Landroid/content/Context;I)Landroid/view/PointerIcon;",
                &[JValue::Object(&activity), JValue::Int(icon_type)],
            ) {
                Ok(v) => match v.l() {
                    Ok(pi) => pi,
                    Err(e) => {
                        log::warn!("Failed to get system PointerIcon: {e}");
                        return;
                    }
                },
                Err(e) => {
                    log::warn!("Failed to call PointerIcon.getSystemIcon: {e}");
                    return;
                }
            };
            if let Err(e) = env.call_method(
                &decor_view,
                "setPointerIcon",
                "(Landroid/view/PointerIcon;)V",
                &[JValue::Object(&pointer_icon)],
            ) {
                log::warn!("Failed to call setPointerIcon: {e}");
            }
        },
        android_app.clone(),
    );
}
