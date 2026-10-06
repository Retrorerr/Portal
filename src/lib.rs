pub mod core {
    pub mod android_input;
    pub mod android_integration;
    pub mod clipboard_broker;
    pub mod clipboard_policy;
    pub mod clipboard_sync;
    pub mod config;
    pub mod coordinate_transform;
    pub mod drm_nodes;
    pub mod guest_browser;
    pub mod guest_locale;
    pub mod guest_sudo;
    #[cfg(unix)]
    pub mod guest_timezone;
    pub mod gamepad;
    pub mod ime_policy;
    pub mod install_plan;
    pub mod mesa_layer;
    pub mod optional_apps;
    pub mod pointer_buttons;
    pub mod presentation;
    pub mod provisioning;
    pub mod renderer_policy;
    pub mod runtime;
    pub mod shm_damage;
    pub mod startup;
    pub mod stylus;
    pub mod surface_geometry;
    pub mod system_updates;
    pub mod tablet_mode;
    pub mod wayland_protocol;
}

#[cfg(target_os = "android")]
pub mod android {
    pub mod accessibility;
    pub mod anland;
    pub mod clipboard;
    pub mod clipboard_broker;
    pub mod diagnostics;
    pub mod gamepad;
    pub mod ime;
    pub mod tablet_mode_manager;

    pub mod main;
    pub mod app {
        pub mod build;
        pub mod run;
    }
    pub mod backend {
        pub mod pipewire_standalone_aaudio;
        pub mod wayland;
        pub mod webview;
    }
    pub mod proot {
        pub mod launch;
        pub mod mesa_layer;
        pub mod optional_apps;
        pub mod process;
        pub mod setup;
        pub mod system_updates;
    }
    pub mod runtime {
        pub mod proot;
    }
    pub mod utils {
        pub mod application_context;
        pub mod compose_overlay;
        pub mod display_mode_request;
        pub mod frame_pacing;
        pub mod frame_rate;
        pub mod fullscreen_immersive;
        pub mod ndk;
        pub mod recovery_screen;
        pub mod webview_handoff;
    }
}
