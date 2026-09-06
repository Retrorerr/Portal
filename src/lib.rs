pub mod core {
    pub mod android_input;
    pub mod android_integration;
    pub mod clipboard_broker;
    pub mod clipboard_policy;
    pub mod clipboard_sync;
    pub mod config;
    pub mod coordinate_transform;
    pub mod ime_policy;
    pub mod pointer_buttons;
    pub mod presentation;
    pub mod runtime;
    pub mod provisioning;
    pub mod shm_damage;
    pub mod startup;
    pub mod tablet_mode;
    pub mod wayland_protocol;
}

#[cfg(target_os = "android")]
pub mod android {
    pub mod accessibility;
    pub mod clipboard;
    pub mod clipboard_broker;
    pub mod diagnostics;
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
        pub mod process;
        pub mod setup;
    }
    pub mod runtime {
        pub mod proot;
    }
    pub mod utils {
        pub mod application_context;
        pub mod frame_rate;
        pub mod fullscreen_immersive;
        pub mod ndk;
        pub mod webview;
        pub mod webview_handoff;
    }
}
