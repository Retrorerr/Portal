use jni::objects::JObject;
use jni::sys::_jobject;
use jni::JNIEnv;
use winit::platform::android::activity::AndroidApp;

pub fn keep_screen_on(env: &mut JNIEnv, android_app: &AndroidApp) {
    let activity_obj = unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _jobject) };

    // Call getWindow method
    let window = env
        .call_method(activity_obj, "getWindow", "()Landroid/view/Window;", &[])
        .expect("Failed to call getWindow")
        .l()
        .expect("Expected a Window object");

    // Get the WindowManager.LayoutParams class
    let layout_params_class = env
        .find_class("android/view/WindowManager$LayoutParams")
        .expect("Failed to find WindowManager.LayoutParams class");

    // Get the FLAG_KEEP_SCREEN_ON constant
    let flag_keep_screen_on = env
        .get_static_field(&layout_params_class, "FLAG_KEEP_SCREEN_ON", "I")
        .expect("Failed to get FLAG_KEEP_SCREEN_ON")
        .i()
        .unwrap();

    // Call addFlags method to set FLAG_KEEP_SCREEN_ON
    env.call_method(
        window,
        "addFlags",
        "(I)V",
        &[jni::objects::JValue::from(flag_keep_screen_on)],
    )
    .expect("Failed to call addFlags");
}
