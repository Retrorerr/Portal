# Portal user guide

Portal runs a Debian 13 desktop directly on a supported ARM64 Android device. It does not require root, a remote server, or a second computer after installation.

## Before installing

- Use an ARM64 Android tablet. Development and hardware validation currently focus on the OnePlus Pad 3.
- Keep several gigabytes of internal storage free for Debian, Plasma, applications, and updates.
- A physical keyboard and pointing device are recommended. Portal can switch between tablet and desktop input behaviour as devices are attached.
- Treat builds from `main` as development software. Back up anything important inside the Linux guest.

## First launch

Keep Portal open while it prepares the Debian runtime. The setup screen reports each stage and can export diagnostics if provisioning fails. Portal asks for notification permission so it can tell you when it has to stay open to finish. Closing Android's activity during extraction or package installation can delay completion, but setup stages are designed to resume safely.

Setup lets you pick a light or dark desktop theme and which optional apps to install.

When setup completes, Portal starts KDE Plasma 6 on its native Wayland compositor. If Plasma fails during early startup, Portal records the failure and opens a recovery session instead of silently switching the normal desktop to X11.

## Updates and optional apps

When you switch back to Portal from another Android app, the screen that restores your desktop also shows what is waiting:

- **Updates** covers Portal itself, its graphics driver layer, ChatGPT and Debian packages. Tap the capsule to see the list, then **Update**. The first time Portal updates itself, Android asks you to let Portal install apps; after that, Android asks to install the new version and closes Portal to finish. Nothing appears when everything is current.
- **Apps** lists the optional apps from setup, such as Steam. Add or remove any of them at any time; installs carry on beside the running desktop.

Portal 2.4.x predates the updater, so install 2.5.0 once by hand from the [latest release](https://github.com/Retrorerr/Portal/releases/latest). Later versions arrive in the app.

## Installing software

Portal uses Debian's standard package manager. You are the `desktop` user and `sudo` works without a password (the account has none), so ordinary Debian instructions apply as written:

```bash
sudo apt update
sudo apt install wget
```

Search available packages with `apt search <name>`. Graphical applications appear in Plasma's launcher after installation. `wget`, `curl`, the `ffmpeg` command line and CJK fonts (`fonts-noto-cjk`) are part of a fresh install; existing installs can add them with `apt`.

Third-party repositories and `.deb` files work too, for example `sudo apt install ./package-arm64.deb` or a vendor's APT repository. Only arm64 packages run.

Portal keeps its own KWin and Xwayland builds pinned so a package upgrade cannot replace them with stock Debian ones; everything else upgrades normally.

### Firefox

Setup installs Mozilla's own Firefox from Mozilla's APT repository, so the launcher shows a single Firefox. Debian's Firefox ESR stays installed, hidden from the menu, as a fallback; the `firefox-beta`, `firefox-devedition` and `firefox-nightly` channels install from the same repository. Portal applies the settings it needs for Android (GPU compositing on) to every Firefox under `/usr/lib`, at each launch and again after every `apt`/`dpkg` run, so a new install or upgrade needs nothing extra. **System Settings → Default Applications** chooses the default browser.

Android's kernel has no user or PID namespaces, so PRoot emulates the parts browser sandboxes rely on. Chrome, Chromium, Electron apps and Firefox run with their own sandboxes on, as on a desktop Linux install, without `--no-sandbox`.

## Display and input

Display size, refresh rate, rotation, and density come from Android. Plasma receives a logical desktop size and scale suitable for the attached screen. You can adjust the result under **System Settings → Display & Monitor**.

Android may intercept desktop shortcuts before they reach Linux. If keys such as `Ctrl+C`, `Ctrl+V`, or `Alt+Tab` do not arrive in Plasma, enable Portal under **Android Settings → Accessibility → Downloaded apps**. The service forwards physical key events and does not inspect window content.

When no external keyboard, mouse, or touchpad is connected, Portal can present Plasma in tablet mode and allow Android's software keyboard. Attaching desktop input switches the session back to desktop behaviour.

## Games

Pick Steam during setup or add it later from **Apps** (see above). Windows games run through Proton; Portal prefers **Proton Experimental (ARM64)** when it is installed.

While Steam runs, a small handle sits at the top of the screen. One side shows or hides the on-screen controller while a game is running; the other opens the game panel:

- **On-screen controller**: show it, set its opacity and size, and edit the layout. A layout can be shared by every game or kept for one game. The controller steps aside while a physical controller is connected unless you ask for it.
- **Game settings**: the video memory Windows games are told the GPU has and a **Frame-rate cap** for steadier pacing and less heat. These can also be kept per game, and apply the next time the game starts.

Games and browser pages that lock the mouse pointer get Android pointer capture, so mouse look works with a physical mouse. On OnePlus, OPPO and realme devices, add Portal to the **Games** app so the screen is not held at a lower refresh rate.

## Android storage

Grant **All files access** only if you want the Linux guest to work with shared Android files. Portal's own Debian runtime remains in the app's private storage.

Uninstalling Portal removes that private storage. Export important files before uninstalling, changing signing keys, or moving between incompatible builds.

## Diagnostics and recovery

Use Portal's diagnostics export before reinstalling. Reports are bounded to Portal's host logs, setup state, and guest state directory; unrelated personal files are excluded.

When reporting a problem, include:

- device model and Android version;
- Portal version and build source;
- whether the installation is fresh or upgraded;
- the shortest reliable reproduction sequence;
- the exported diagnostics archive, after reviewing it.

Open reports through the repository's [issue tracker](https://github.com/Retrorerr/Portal/issues).

## Android process limits

Android 12 and newer can stop large child-process trees. If the entire desktop disappears under load, first check whether the device exposes **Disable child process restrictions** in Developer options. System-wide ADB changes should be a last resort and are intentionally not prescribed here because availability and side effects vary by Android build.
