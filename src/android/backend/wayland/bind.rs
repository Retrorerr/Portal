use crate::core::runtime::LinuxRuntime;
use smithay::reexports::wayland_server::ListeningSocket;
use std::{error::Error, fs};

pub fn bind_socket() -> Result<ListeningSocket, Box<dyn Error>> {
    let runtime = crate::android::runtime::proot::PRootRuntime::active();
    let tmp_dir = runtime.rootfs_path().join("tmp");
    let _ = fs::create_dir_all(&tmp_dir);
    let socket_path = tmp_dir.join(crate::core::config::WAYLAND_SOCKET_NAME);
    let _ = fs::remove_file(&socket_path);
    let listener = ListeningSocket::bind_absolute(socket_path)?;
    Ok(listener)
}
