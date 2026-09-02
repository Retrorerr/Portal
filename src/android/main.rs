use crate::{
    android::{
        accessibility::{register_event_loop_proxy, AppUserEvent},
        app::build::PolarBearApp,
        diagnostics,
        ime,
        utils::{
            application_context::ApplicationContext,
            fullscreen_immersive::{enable_fullscreen_immersive_mode, keep_screen_on},
            ndk::run_in_jvm,
            webview_handoff,
        },
    },
    core::config,
};
use winit::{
    event_loop::{ControlFlow, EventLoop},
    platform::android::{activity::AndroidApp, EventLoopBuilderExtAndroid},
};

#[no_mangle]
fn android_main(android_app: AndroidApp) {
    std::env::set_var("RUST_BACKTRACE", "full");

    // Build the context before touching the persistent diagnostic paths.  The
    // context owns the app-private directory used by diagnostics, and this
    // call must happen before any setup/PRoot worker can emit guest events.
    ApplicationContext::build(&android_app);
    diagnostics::initialize();

    // Keep Android logcat and the user-exported host log bounded in debug builds too. The
    // per-frame Smithay/EGL records are DEBUG-level; they remain available only when a future
    // explicitly bounded diagnostic mode opts into them.
    let log_level = log::LevelFilter::Info;
    let logger = android_logger::AndroidLogger::default();
    // Keep a copy in diagnostics/host.log even when Android logcat is
    // unavailable (or a release build is running with logcat filtering).
    if log::set_boxed_logger(Box::new(diagnostics::HostLogTee::new(Box::new(logger)))).is_ok() {
        log::set_max_level(log_level);
    } else {
        android_logger::init_once(android_logger::Config::default().with_max_level(log_level));
    }

    run_in_jvm(enable_fullscreen_immersive_mode, android_app.clone());
    run_in_jvm(keep_screen_on, android_app.clone());

    // Resolve the bundled bridge once from the NativeActivity-attached thread.  Calling `hide`
    // is deliberately nonintrusive, but it exercises the same Activity class-loader path used
    // by later IME events and leaves a useful success/failure marker in logcat and host.log.
    match ime::hide(&android_app) {
        Ok(()) => log::info!("SoftKeyboardBridge JNI smoke check passed"),
        Err(error) => log::warn!("SoftKeyboardBridge JNI smoke check failed: {error}"),
    }

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
