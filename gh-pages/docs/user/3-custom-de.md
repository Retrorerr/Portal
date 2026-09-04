---
title: Custom desktop commands
---

:::warning
This is an advanced topic. Keep a copy of the working Plasma commands before experimenting.
:::

## The `[command]` configuration

Portal uses three commands from `/etc/localdesktop/localdesktop.toml`:

- `check` verifies that the complete desktop environment is installed.
- `install` provisions any missing packages during the guided first run.
- `launch` starts the session after the native Android Wayland compositor is ready.

The built-in defaults install KDE Plasma 6 and launch
`/usr/local/bin/startplasma-localdesktop` directly on `WAYLAND_DISPLAY=wayland-0`.
Xwayland remains available only for individual legacy applications inside Plasma; it is not
the app's display backend or session launcher.

The default launcher also connects Plasma to Portal's PipeWire/Pulse bridge, applies
Android-aware scaling, enables KWin's QPainter renderer, and records startup failures under
`/var/lib/localdesktop`. If Plasma cannot become ready within the startup window, the app
enters a small native-Wayland labwc recovery session where Plasma can be retried.

## Trying a different Wayland session

Use the optional `try_check`, `try_install`, and `try_launch` keys first. They override the
normal values for one validation cycle and are commented out automatically after a successful
run.

```toml title="/etc/localdesktop/localdesktop.toml"
[command]
try_check = "pacman -Q your-wayland-session"
try_install = "stdbuf -oL pacman -Syu --needed --noconfirm --noprogressbar your-wayland-session"
try_launch = "XDG_RUNTIME_DIR=/tmp WAYLAND_DISPLAY=wayland-0 XDG_SESSION_TYPE=wayland your-session-command 2>&1"
```

The session must be able to run as a nested Wayland client on the existing
`/tmp/wayland-0` socket. Replacing the native Android Wayland backend with VNC, a Termux
display server, or an X11 session is outside this fork's supported configuration.

When diagnosing a custom session, inspect Android logcat together with
`/var/lib/localdesktop/plasma.log` and `/var/lib/localdesktop/recovery.log`.
