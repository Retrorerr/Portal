//! Steam detection for the touch controls (see `core::steam_processes`).
//! The guest process sampler gathers it on its regular pass over `/proc`.

use jni::objects::JClass;
use jni::sys::jstring;
use jni::JNIEnv;

#[no_mangle]
pub extern "system" fn Java_app_polarbear_TouchControls_nativeScanSteam(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    match env.new_string(crate::android::guest_procfs::steam().encode()) {
        Ok(text) => text.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}
