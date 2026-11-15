use crate::{
    android::{
        accessibility::{register_event_loop_proxy, AppUserEvent},
        app::build::PolarBearApp,
        diagnostics, ime,
        utils::{application_context::ApplicationContext, webview_handoff},
    },
    core::config,
};
use winit::{
    event_loop::{ControlFlow, EventLoop},
    platform::android::{activity::AndroidApp, EventLoopBuilderExtAndroid},
};

#[no_mangle]
fn android_main(android_app: AndroidApp) {
    crate::android::proot::launch::note_process_start();
    // Every path into the guest goes through the workspace chosen here, so it
    // comes before anything that reads the rootfs.
    let workspace = crate::core::workspaces::activate(
        std::path::Path::new(config::APP_FILES_ROOT),
        crate::core::workspaces::now_unix(),
    );
    std::env::set_var(
        "RUST_BACKTRACE",
        if cfg!(debug_assertions) { "1" } else { "0" },
    );
    // The host libxkbcommon runs outside PRoot. Its historical compiled-in
    // directory points at Arch, so bind its data search to the production
    // runtime before any keyboard context (or background worker) is created.
    std::env::set_var(
        "XKB_CONFIG_ROOT",
        config::rootfs().join("usr/share/X11/xkb"),
    );
    std::env::set_var("XLOCALEDIR", config::rootfs().join("usr/share/X11/locale"));

    // Build the context before touching the persistent diagnostic paths.  The
    // context owns the app-private directory used by diagnostics, and this
    // call must happen before any setup/PRoot worker can emit guest events.
    ApplicationContext::build(&android_app);
    diagnostics::initialize();
    diagnostics::host_event("workspace", &format!("id={}", workspace.id));

    // Keep Android logcat and the user-exported host log bounded in debug builds too. The
    // per-frame Smithay/EGL records are DEBUG-level; they remain available only when a future
    // explicitly bounded diagnostic mode opts into them.
    let log_level = if cfg!(debug_assertions) {
        log::LevelFilter::Info
    } else {
        log::LevelFilter::Warn
    };
    let logger = android_logger::AndroidLogger::default();
    // Keep a copy in diagnostics/host.log even when Android logcat is
    // unavailable (or a release build is running with logcat filtering).
    if log::set_boxed_logger(Box::new(diagnostics::HostLogTee::new(Box::new(logger)))).is_ok() {
        log::set_max_level(log_level);
    } else {
        android_logger::init_once(android_logger::Config::default().with_max_level(log_level));
    }

    // Resolve the bundled bridge once from the NativeActivity-attached thread.  Calling `hide`
    // is deliberately nonintrusive, but it exercises the same Activity class-loader path used
    // by later IME events and leaves a useful success/failure marker in logcat and host.log.
    match ime::hide(&android_app) {
        Ok(()) => log::info!("SoftKeyboardBridge JNI smoke check passed"),
        Err(error) => log::warn!("SoftKeyboardBridge JNI smoke check failed: {error}"),
    }
    if let Err(error) = ime::start_hardware_keyboard_monitor(&android_app) {
        log::warn!("Physical keyboard monitor failed to start: {error}");
    }

    // Each new kind of pointer event is recorded once, so a report shows how
    // a device this app has never met presents its mouse or touchpad.
    winit::platform::android::set_pointer_shape_observer(|shape| {
        crate::android::diagnostics::host_event(
            "pointer-shape",
            &format!(
                "device={} source={:#x} tool={} action={} buttons={:#x} class={}",
                shape.device_id,
                shape.source,
                shape.tool_type,
                shape.action,
                shape.button_state,
                shape.class
            ),
        );
    });
    let event_loop = EventLoop::<AppUserEvent>::with_user_event()
        .with_android_app(android_app.clone())
        .build()
        .expect("Failed to create event loop");
    register_event_loop_proxy(event_loop.create_proxy());
    ime::register_event_loop_proxy(event_loop.create_proxy());
    webview_handoff::register_event_loop_proxy(event_loop.create_proxy());

    // ControlFlow::Poll continuously runs the event loop, even if the OS hasn't
    // dispatched any events. This is ideal for games and similar applications.
    // event_loop.set_control_flow(ControlFlow::Poll);

    // ControlFlow::Wait pauses the event loop if no events are available to process.
    // This is ideal for non-game applications that only update in response to user
    // input, and uses significantly less power/CPU time than ControlFlow::Poll.
    event_loop.set_control_flow(ControlFlow::Wait);

    // Phase 1: Setup
    let mut app = PolarBearApp::build(android_app);

    // Phase 2: Run
    event_loop.run_app(&mut app).expect("Failed to run app");
}
