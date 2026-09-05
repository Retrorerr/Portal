//! Narrowly scoped high-refresh-rate hint for Portal's `ANativeWindow`.
//!
//! Physical profiling on the OnePlus Pad 3 shows Portal is subject to
//! OxygenOS's unrecognized-app ceiling (60 Hz touch-active, 50 Hz VRR idle)
//! while the panel supports up to 144 Hz. Portal previously made no frame-rate
//! request at all.
//!
//! This module issues exactly one supported hint:
//! `ANativeWindow_setFrameRate[WithChangeStrategy](<preferred> Hz, DEFAULT,
//! ONLY_IF_SEAMLESS)`, where `<preferred>` is resolved from
//! `Display.getSupportedModes()` (144 Hz on the OnePlus Pad 3, otherwise the
//! device's own maximum). Semantics per the NDK:
//! - `frameRate` is a hint; the system may stay on a lower refresh when idle
//!   or when no better match exists. Unsupported modes are never forced.
//! - `COMPATIBILITY_DEFAULT (0)` lets the system use VRR/power-saving and
//!   lets Portal adapt via pull-down. Never uses fixed-source video semantics.
//! - `ONLY_IF_SEAMLESS (0)` preserves UX: no mode switch with visual
//!   interruption (black screen). Power-saving behaviour is preserved.
//!
//! No global settings are modified, no root is required, no hidden OEM APIs
//! are used, and no device-name checks exist. Symbols are resolved at runtime
//! with `dlopen`/`dlsym` so devices with API < 30/31 (where the symbols do
//! not exist) degrade gracefully to a logged no-op instead of failing to load.

use winit::platform::android::activity::AndroidApp;

/// Fallback interactive desktop hint in Hz, used only when the supported-mode
/// list is unavailable. The live path resolves the preferred rate from
/// `Display.getSupportedModes()`; this stays as the sane default.
pub const DESIRED_FRAME_RATE_HZ: f32 = 120.0;

/// Fallback interactive desktop hint in millihertz (see [`DESIRED_FRAME_RATE_HZ`]).
pub const DESIRED_FRAME_RATE_MILLIHZ: i32 = 120_000;

/// Resolve the preferred frame-rate hint in Hz from the display modes Android
/// reports (144.0 on the OnePlus Pad 3, otherwise the device maximum).
/// Always finite and positive; falls back to [`DESIRED_FRAME_RATE_HZ`].
pub fn preferred_frame_rate_hz(android_app: &AndroidApp) -> f32 {
    let millihz = crate::android::utils::ndk::preferred_high_refresh_millihz(android_app);
    let hz = millihz as f32 / 1000.0;
    if hz.is_finite() && hz > 0.0 {
        hz
    } else {
        DESIRED_FRAME_RATE_HZ
    }
}

/// `ANATIVEWINDOW_FRAME_RATE_COMPATIBILITY_DEFAULT` — no inherent restriction.
/// Correct for games/UIs; preserves VRR and power-saving.
const COMPATIBILITY_DEFAULT: i8 = 0;
/// `ANATIVEWINDOW_CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS` — never cause a
/// non-seamless (visually interrupting) mode switch.
const CHANGE_ONLY_IF_SEAMLESS: i8 = 0;

/// Request the preferred rate for Portal's current `ANativeWindow` via the
/// supported NDK path. Idempotency is owned by the caller (invoke on
/// resume/window creation, not per-frame): the hint is sticky per window until
/// cleared or destroyed.
///
/// Logs the numeric `status` return (0 = success, `-EINVAL` = invalid window /
/// rate / compatibility) plus which entry point was used, so OxygenOS policy
/// decisions can be correlated with `dumpsys display` / SurfaceFlinger
/// evidence. Never panics, never modifies global state.
pub fn ensure_high_refresh_rate(android_app: &AndroidApp) {
    ensure_high_refresh_rate_hz(android_app, preferred_frame_rate_hz(android_app));
}

/// Request an explicit rate (Hz) for Portal's current `ANativeWindow`.
///
/// Prefer [`ensure_high_refresh_rate`] (which resolves the preferred rate
/// from the supported modes) unless the caller already holds the nominal
/// target and needs the hint to match it exactly. Invalid/non-positive rates
/// fall back to [`DESIRED_FRAME_RATE_HZ`].
pub fn ensure_high_refresh_rate_hz(android_app: &AndroidApp, rate_hz: f32) {
    let rate_hz = if rate_hz.is_finite() && rate_hz > 0.0 {
        rate_hz
    } else {
        DESIRED_FRAME_RATE_HZ
    };
    let Some(native_window) = android_app.native_window() else {
        log::debug!("frame-rate: no ANativeWindow yet; skipping {rate_hz} Hz hint");
        return;
    };
    let raw_window = native_window.ptr().as_ptr().cast::<std::ffi::c_void>();

    // SAFETY: `dlopen`/`dlsym`/`dlclose` with NUL-terminated literals; the
    // resolved function pointers are only called with a live ANativeWindow
    // owned by `native_window` (kept alive for the whole call).
    unsafe {
        let lib_name = std::ffi::CString::new("libnativewindow.so").expect("static str");
        let handle = libc::dlopen(lib_name.as_ptr(), libc::RTLD_NOW);
        if handle.is_null() {
            log::warn!(
                "frame-rate: dlopen(libnativewindow.so) failed; cannot request {rate_hz} Hz"
            );
            return;
        }

        // Prefer the API-31 entry point so the seamless strategy is explicit.
        let with_strategy_name =
            std::ffi::CString::new("ANativeWindow_setFrameRateWithChangeStrategy")
                .expect("static str");
        let with_strategy_addr = libc::dlsym(handle, with_strategy_name.as_ptr());
        if !with_strategy_addr.is_null() {
            type WithStrategyFn =
                unsafe extern "system" fn(*mut std::ffi::c_void, f32, i8, i8) -> i32;
            let func: WithStrategyFn = std::mem::transmute(with_strategy_addr);
            let status = func(
                raw_window,
                rate_hz,
                COMPATIBILITY_DEFAULT,
                CHANGE_ONLY_IF_SEAMLESS,
            );
            log::info!(
                "frame-rate: ANativeWindow_setFrameRateWithChangeStrategy({rate_hz}Hz, compat=DEFAULT, seamless) status={status}"
            );
            crate::android::diagnostics::host_event(
                "frame-rate",
                &format!("api=setFrameRateWithChangeStrategy rate_hz={rate_hz} compat=default strategy=seamless status={status}"),
            );
            libc::dlclose(handle);
            return;
        }

        // Fallback to the API-30 entry point, which is defined as the same
        // call with ONLY_IF_SEAMLESS.
        let legacy_name = std::ffi::CString::new("ANativeWindow_setFrameRate").expect("static str");
        let legacy_addr = libc::dlsym(handle, legacy_name.as_ptr());
        if !legacy_addr.is_null() {
            type SetFrameRateFn = unsafe extern "system" fn(*mut std::ffi::c_void, f32, i8) -> i32;
            let func: SetFrameRateFn = std::mem::transmute(legacy_addr);
            let status = func(raw_window, rate_hz, COMPATIBILITY_DEFAULT);
            log::info!(
                "frame-rate: ANativeWindow_setFrameRate({rate_hz}Hz, compat=DEFAULT) status={status}"
            );
            crate::android::diagnostics::host_event(
                "frame-rate",
                &format!("api=setFrameRate rate_hz={rate_hz} compat=default status={status}"),
            );
        } else {
            log::warn!(
                "frame-rate: neither ANativeWindow_setFrameRate symbol found (requires API 30+); {rate_hz} Hz hint unavailable"
            );
            crate::android::diagnostics::host_event(
                "frame-rate",
                "api=unavailable requires_api=30 status=missing-symbol",
            );
        }
        libc::dlclose(handle);
    }
}
