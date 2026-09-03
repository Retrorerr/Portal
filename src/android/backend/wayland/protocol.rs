//! Server bindings for Android Wayland protocols:
//!   * `android_wlegl` - used by Android clients / libhybris to pass gralloc native handles.

pub mod android_wlegl {
    pub mod server {
        use smithay::reexports::wayland_server;
        use smithay::reexports::wayland_server::protocol::*;
        pub mod __interfaces {
            use smithay::reexports::wayland_server::backend as wayland_backend;
            use smithay::reexports::wayland_server::protocol::__interfaces::*;
            wayland_scanner::generate_interfaces!("protocols/android_wlegl.xml");
        }
        use self::__interfaces::*;

        wayland_scanner::generate_server_code!("protocols/android_wlegl.xml");
    }
}
