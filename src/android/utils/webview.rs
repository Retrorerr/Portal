//! Android WebView host used for setup and actionable error/recovery screens.

use super::webview_handoff;
use jni::{
    objects::{JObject, JValue},
    sys::_jobject,
    JNIEnv,
};
use winit::platform::android::activity::AndroidApp;

const ANDROID_CONTENT_ID: i32 = 0x0102_0002; // android.R.id.content

/// Percent-encode a query value without pulling a URL parser into the tiny Android host.
pub fn encode_query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

/// Build the local setup page URL, including the per-instance action token.
pub fn setup_page_url(port: u16, token: &str) -> String {
    format!(
        "file:///android_asset/setup-progress-v2.html?port={port}&token={}",
        encode_query_component(token)
    )
}

/// Build the local runtime error page URL. The reason is query-escaped because setup and
/// renderer errors often contain spaces, punctuation, or a newline from an underlying JNI call.
pub fn runtime_error_page_url(port: u16, token: &str, reason: &str) -> String {
    format!(
        "file:///android_asset/runtime-error.html?port={port}&token={}&reason={}",
        encode_query_component(token),
        encode_query_component(reason)
    )
}

pub fn unsupported_page_url() -> &'static str {
    "file:///android_asset/unsupported.html"
}

fn activity(android_app: &AndroidApp) -> JObject<'static> {
    unsafe { JObject::from_raw(android_app.activity_as_ptr() as *mut _jobject) }
}

fn clear_exception(env: &mut JNIEnv<'_>, context: &str) {
    if env.exception_check().unwrap_or(false) {
        log::error!("Android WebView {context} raised a Java exception");
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

/// Run a WebView popup on its own Android Looper.
///
/// A NativeActivity does not supply a Java `onCreate` that we can extend, so this host creates a
/// dedicated Looper and keeps all WebView/PopupWindow calls on that thread. Setup completion asks
/// the Looper to quit through [`webview_handoff::request_close`]; after `Looper.loop()` returns we
/// dismiss the popup and release the global references. There is no fixed sleep or activity
/// recreation in this handoff path.
pub fn show_webview_popup(env: &mut JNIEnv<'_>, android_app: &AndroidApp, url: &str) {
    if env
        .call_static_method("android/os/Looper", "prepare", "()V", &[])
        .is_err()
    {
        clear_exception(env, "Looper.prepare");
        return;
    }

    let activity_obj = activity(android_app);
    let root = match env
        .call_method(
            &activity_obj,
            "findViewById",
            "(I)Landroid/view/View;",
            &[JValue::Int(ANDROID_CONTENT_ID)],
        )
        .and_then(|value| value.l())
    {
        Ok(root) if !root.is_null() => root,
        Ok(_) => {
            log::error!("Android WebView content root is null");
            return;
        }
        Err(error) => {
            log::error!("Failed to find Android WebView content root: {error}");
            clear_exception(env, "findViewById");
            return;
        }
    };

    let webview_class = match env.find_class("android/webkit/WebView") {
        Ok(class) => class,
        Err(error) => {
            log::error!("Android WebView class is unavailable: {error}");
            clear_exception(env, "find WebView class");
            return;
        }
    };
    let webview = match env.new_object(
        webview_class,
        "(Landroid/content/Context;)V",
        &[JValue::Object(&activity_obj)],
    ) {
        Ok(webview) => webview,
        Err(error) => {
            log::error!("Failed to create WebView object: {error}");
            clear_exception(env, "WebView constructor");
            return;
        }
    };

    let settings = match env
        .call_method(
            &webview,
            "getSettings",
            "()Landroid/webkit/WebSettings;",
            &[],
        )
        .and_then(|value| value.l())
    {
        Ok(settings) => settings,
        Err(error) => {
            log::error!("Failed to access WebView settings: {error}");
            clear_exception(env, "getSettings");
            return;
        }
    };
    if let Err(error) = env.call_method(
        &settings,
        "setJavaScriptEnabled",
        "(Z)V",
        &[JValue::Bool(1)],
    ) {
        log::warn!("Failed to enable WebView JavaScript: {error}");
        clear_exception(env, "setJavaScriptEnabled");
    }
    let _ = env.call_method(
        &webview,
        "setBackgroundColor",
        "(I)V",
        &[JValue::Int(0xFF10_131A_u32 as i32)],
    );

    // Keep navigation inside the local page. A WebViewClient created here is sufficient for the
    // setup/error pages and avoids opening an external browser for diagnostic links.
    let webview_client = match env
        .find_class("android/webkit/WebViewClient")
        .and_then(|class| env.new_object(class, "()V", &[]))
    {
        Ok(client) => client,
        Err(error) => {
            log::warn!("Failed to create WebViewClient; using default client: {error}");
            clear_exception(env, "WebViewClient");
            JObject::null()
        }
    };
    if !webview_client.is_null() {
        let _ = env.call_method(
            &webview,
            "setWebViewClient",
            "(Landroid/webkit/WebViewClient;)V",
            &[JValue::Object(&webview_client)],
        );
    }

    let Ok(jurl) = env.new_string(url) else {
        log::error!("Failed to allocate WebView URL string");
        return;
    };
    if let Err(error) = env.call_method(
        &webview,
        "loadUrl",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&jurl)],
    ) {
        log::error!("Failed to load Local Desktop WebView page: {error}");
        clear_exception(env, "loadUrl");
        return;
    }

    let popup = match env.new_object(
        "android/widget/PopupWindow",
        "(Landroid/view/View;II)V",
        &[
            JValue::Object(&webview),
            JValue::Int(-1), // MATCH_PARENT width
            JValue::Int(-1), // MATCH_PARENT height
        ],
    ) {
        Ok(popup) => popup,
        Err(error) => {
            log::error!("Failed to create WebView PopupWindow: {error}");
            clear_exception(env, "PopupWindow constructor");
            return;
        }
    };
    let _ = env.call_method(
        &popup,
        "setFocusable",
        "(Z)V",
        &[JValue::Bool(1)],
    );
    let _ = env.call_method(
        &popup,
        "setOutsideTouchable",
        "(Z)V",
        &[JValue::Bool(0)],
    );

    let looper = looper(env);
    if looper.is_null() || !webview_handoff::install(env, &looper, &popup) {
        log::error!("Failed to retain WebView handoff references");
        return;
    }

    // Use the activity's content root as the PopupWindow parent. Passing the WebView itself can
    // produce a blank popup on Android builds which require a valid window token.
    if let Err(error) = env.call_method(
        &popup,
        "showAtLocation",
        "(Landroid/view/View;III)V",
        &[
            JValue::Object(&root),
            JValue::Int(17), // Gravity.CENTER
            JValue::Int(0),
            JValue::Int(0),
        ],
    ) {
        log::error!("Failed to show Local Desktop WebView: {error}");
        clear_exception(env, "showAtLocation");
        webview_handoff::clear();
        return;
    }

    // This call blocks only the dedicated WebView thread. The setup/runtime owner quits it when
    // the page is no longer needed; it is deliberately not guarded by an arbitrary timeout.
    if let Err(error) = env.call_static_method("android/os/Looper", "loop", "()V", &[]) {
        log::warn!("Local Desktop WebView Looper exited with an error: {error}");
        clear_exception(env, "Looper.loop");
    }

    let _ = env.call_method(&popup, "dismiss", "()V", &[]);
    let _ = env.call_method(&webview, "destroy", "()V", &[]);
    webview_handoff::clear();
}

fn looper<'local>(env: &mut JNIEnv<'local>) -> JObject<'local> {
    env.call_static_method(
        "android/os/Looper",
        "myLooper",
        "()Landroid/os/Looper;",
        &[],
    )
    .and_then(|value| value.l())
    .unwrap_or_else(|_| JObject::null())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode_query_component("KWin failed: 50%\n"), "KWin%20failed%3A%2050%25%0A");
        assert_eq!(setup_page_url(1234, "a/b"), "file:///android_asset/setup-progress-v2.html?port=1234&token=a%2Fb");
        assert!(runtime_error_page_url(1234, "token", "bad reason").contains("reason=bad%20reason"));
    }
}
