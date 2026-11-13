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

Portal's return screen, and the veil you can recall over the running desktop, show what is waiting:

- **Updates** covers Portal itself, its graphics driver layer, ChatGPT and Debian packages. Tap the capsule to see the list, then **Update**. The first time Portal updates itself, Android asks you to let Portal install apps; after that, Android asks to install the new version and closes Portal to finish. Nothing appears when everything is current.
- **Apps** lists the optional apps from setup, such as Steam. Add or remove any of them at any time; installs carry on beside the running desktop.

Portal 2.4.x predates the updater, so install the current version once by hand from the [latest release](https://github.com/Retrorerr/Portal/releases/latest). Later versions arrive in the app.

## Transferring files

Tap the tab at the screen edge to open Portal's labelled controls. Its icon
suggests the keyboard when a text field is focused, game options while Steam
or a game is running, and Files during a transfer. The bubbles open Report,
Files, Apps, the desktop keyboard, or Game settings. Report, Files and Apps open
on the veil.
The controller toggle appears while a game is running.

The desktop keyboard sends real keys to Linux, including shortcuts. Esc, Tab,
Ctrl, Alt, Super and arrows are always available. Tap **#+=** for punctuation,
then **ABC** to return to letters. Tap **123** for the number pad and tap it
again to return. Tap **+** to unfold F1–F12, navigation, system keys and common
shortcuts beneath the typing rows; **−** folds them away. Tap a modifier
once to arm it for the next key, twice to lock it, and again to release it;
you can also hold modifiers while pressing another key. Holding a key uses
the desktop's normal key repeat. Drag the small strip at the keyboard's bottom
to move it; it coasts and settles within the screen. The surrounding desktop
remains interactive. The keyboard adapts to rotation and smaller windows;
extra rows scroll vertically when space is limited. **▾**, or a quick downward
flick of the bottom strip, minimises the keyboard to a small button that stays
out of the way; tap it to bring the keyboard back, or drag it elsewhere. **×**
closes the keyboard and the controls together, and Android **Back** returns to
Portal controls. All three release held keys. Key positions follow the
desktop's XKB layout (the printed letter and symbol legends use US QWERTY).

Drag the tab to move it. It coasts briefly, settles against the nearest edge,
and remembers its position. It becomes quieter while you use the desktop.
Its frosted surfaces follow the windows underneath, with the same glass
treatment as the veil.
**Ctrl+Alt+P** opens the veil directly; accessibility tools can use
**Open Portal controls**. **Escape**, **Back**, or a tap outside
closes the utility menu. With the desktop keyboard open, Escape reaches Linux
and Back returns to Portal controls.

Open **Portal settings** from the pill to change **Hide cursor** or **Use Android
screen timeout**. Both are on by default. Android handles the idle timeout, and
an app that requests to keep the display awake can hold it on. Turn the timeout
off in Portal settings to keep the display on whenever Portal is visible.

**Use Portal keyboard** (off by default) shows the desktop keyboard wherever
Android's keyboard would otherwise appear. It opens when you tap a text field,
leaves when the field loses focus, and stays away while a hardware keyboard is
connected. If you minimise it, it returns as the small button for later fields
until you tap that or close it. Android's keyboard is not shown while this is
on.

Swipe up or activate the bottom veil affordance to return to the desktop.
Tap outside an expanded Apps or Files card to collapse it. The veil's idle
motion pauses while either card is open.

A quiet **Your desktop is running** notification keeps the Linux session active
when Portal is in the background. Tap it to return. Logging out or closing
Portal ends the session and removes the notification.

Open **Files**, beside **Apps**:

- **Add files** opens Android's file picker. Choose one file or a batch to copy
  into Linux **Downloads**.
- **Save files** opens a compact desktop file chooser, starting in Downloads.
  Select files, then choose **Save to Android**. Android asks where to save
  them. A batch uses Android's destination-folder picker; Android restricts
  some destinations, including the top-level Downloads folder. Save a single
  file to choose Downloads through the normal save dialog.
- You can also drag files from a compatible Android app onto the open Files
  window. Bringing an external drag into Portal opens Files automatically.

Transfers continue when you lift the veil or switch apps. Open **Details** for
individual progress, cancellation, and retry. Copies never remove the originals
or replace an existing Linux file; duplicate names receive a number. An
interrupted transfer must be retried. If Android has withdrawn access to its
source, choose that file again.

Files and batches are supported; put a folder in a ZIP archive before copying
it. No All files access permission or permanent shared folder is needed.

## Installing software

**Discover**, KDE's app store, is the easiest way: it installs Debian packages and Flatpak apps from Flathub, and shows updates for both. Flatpak apps run with their normal sandbox, and Portal installs its GPU driver for them during setup, so they are hardware accelerated from their first start.

Under the hood Portal uses Debian's standard package manager. You are the `desktop` user and `sudo` works without a password (the account has none), so ordinary Debian instructions apply as written:

```bash
sudo apt update
sudo apt install wget
```

Search available packages with `apt search <name>`. Graphical applications appear in Plasma's launcher after installation. `wget`, `curl`, the `ffmpeg` command line and CJK fonts (`fonts-noto-cjk`) are part of a fresh install; existing installs can add them with `apt`.

Third-party repositories and `.deb` files work too, for example `sudo apt install ./package-arm64.deb` or a vendor's APT repository. Only arm64 packages run.

Portal keeps its own KWin and Xwayland builds pinned so a package upgrade cannot replace them with stock Debian ones; everything else upgrades normally.

### Portable applications

Use **Open With → Open with Portal** in Dolphin, or run
`portal-run /path/to/application` in a terminal. Arguments, including paths
with spaces, are passed through to the application.

- Type 2 AppImages extract into a reusable cache without requiring FUSE.
  Mark the downloaded file executable first. Each updated image gets its
  own cache; a failed extraction can be retried.
- Native Linux bundles keep their own libraries and Qt plugins. x86-64
  Linux programs use Box64 when installed; other x86 programs require FEX
  and its matching RootFS. The launcher explains which runtime is missing.
- Java archives use the installed Java runtime. `.NET` DLLs require the
  application's matching `dotnet` runtime.
- Windows `.exe` and `.msi` files use Wine when installed. Otherwise, an
  installed Proton for ARM64 can run them through Portal's Steam integration.
  Install the large runtime through Steam's Tools library first. Standalone
  applications share a separate prefix under
  `~/.local/share/portal/windows`; game prefixes are preserved.
  `STEAM_COMPAT_DATA_PATH` selects a different Proton prefix and
  `PORTAL_PROTON` selects a particular installed Proton. An explicit
  `WINEPREFIX` is used with Wine; Proton uses its own compatibility-data path.

App compatibility depends on the application's architecture, dependencies
and required kernel features. Extraction or a successful launch alone does
not verify graphics, input or every application feature. See the
[compatibility coverage](app-compatibility.md) for tested paths and limits.

### Firefox

Portal ships Mozilla's own Firefox from Mozilla's APT repository, so the launcher shows a single Firefox. Installs set up before 2.6 keep Debian's Firefox ESR installed, hidden from the menu, as a fallback; the `firefox-beta`, `firefox-devedition` and `firefox-nightly` channels install from the same repository. Portal applies the settings it needs for Android (GPU compositing on) to every Firefox under `/usr/lib`, at each launch and again after every `apt`/`dpkg` run, so a new install or upgrade needs nothing extra. **System Settings → Default Applications** chooses the default browser.

Android's kernel has no user or PID namespaces, so Portal's guest engine emulates the parts browser sandboxes rely on. Chrome, Chromium, Electron apps and Firefox run with their own sandboxes on, as on a desktop Linux install, without `--no-sandbox`.

## Display and input

Display size, refresh rate, rotation, and density come from Android. Plasma receives a logical desktop size and scale suitable for the attached screen. You can adjust the result under **System Settings → Display & Monitor**.

On the OnePlus Pad 3, OxygenOS drops the screen to 50 Hz after a few seconds without touch input, even while video plays. 60 fps video then stutters and drops frames, in Portal and in Android's own browsers alike. Portal cannot override this; touching the screen restores the full rate until the next idle period.

On Snapdragon (Adreno) devices, Linux apps decode video in hardware through Android's own decoders. Firefox, Chromium, mpv, Celluloid, VLC, Haruna, GStreamer apps such as Videos, and Flatpak apps with GPU access use it without any setup, for H.264, HEVC, VP9 and AV1 where the device supports them. Apps can also encode H.264, HEVC and VP9 in hardware through VA-API (for example `ffmpeg -c:v h264_vaapi`). Portal sets mpv's `hwdec=auto-safe` in `/etc/mpv/mpv.conf`; delete that file's first line to keep your own edits. Portal downloads the small driver during setup, or in the background on the first launch after an update; until it arrives, and on other GPUs, video decodes in software as before. To turn hardware decoding off, write `off` to Portal's `files/hw-video-decode` file and restart Portal.

Android may intercept desktop shortcuts before they reach Linux. If keys such as `Ctrl+C`, `Ctrl+V`, or `Alt+Tab` do not arrive in Plasma, enable Portal under **Android Settings → Accessibility → Downloaded apps**. The service forwards physical key events and does not inspect window content.

When no external keyboard, mouse, or touchpad is connected, Portal can present Plasma in tablet mode and allow Android's software keyboard. Attaching desktop input switches the session back to desktop behaviour.

## Games

Pick Steam during setup or add it later from **Apps** (see above). Windows games run through Proton; Portal prefers **Proton Experimental (ARM64)** when it is installed. If no ARM64 Proton is installed yet, starting a Windows game opens Steam's install dialog for **Proton 11.0 (ARM64)**.

Open **Game settings** from the Portal pill. The pill expands into the settings
screen; **Done** or **Back** returns to its menu. Settings are available before
launching a game, and the controller toggle appears in the menu while a game runs:

- **On-screen controller**: show it, set its opacity and size, and edit the layout. A layout can be shared by every game or kept for one game. The controller steps aside while a physical controller is connected unless you ask for it.
- **Game settings**: the video memory Windows games are told the GPU has and a **Frame-rate cap** for steadier pacing and less heat. These can also be kept per game, and apply the next time the game starts.

Games and browser pages that lock the mouse pointer get Android pointer capture, so mouse look works with a physical mouse. On OnePlus, OPPO and realme devices, add Portal to the **Games** app so the screen is not held at a lower refresh rate.

## Camera and microphone

Linux apps see the tablet's cameras as ordinary video devices and its microphone as the default PipeWire source, so Firefox video calls, `ffmpeg` or `pw-record` work as on a desktop. Android asks for camera or microphone permission the first time an app opens one, and the device is released when the app stops using it.

## Android storage

Grant **All files access** only if you want the Linux guest to work with shared Android files. Portal's own Debian runtime remains in the app's private storage.

Uninstalling Portal removes that private storage. Export important files before uninstalling, changing signing keys, or moving between incompatible builds.

## Reporting problems and compatibility

Open the floating pill and tap **Report**, or use **Report this on GitHub** on the recovery and failed-setup screens. Portal never uploads anything itself: it opens a GitHub issue form in your browser, and nothing is sent until you post it. Posting needs a free GitHub account. Without one, use **Copy details** and send them any way you like.

- **Report a bug** fills in the bug form. Add the shortest steps that reliably cause the problem, and whether the install is fresh or upgraded.
- **Share what works** fills in the compatibility form. Choose how setup, graphics, video, sound, input and games behaved. Reports from devices that work well are wanted too.

The device details are shown before you continue: Portal version, device model, chipset, Android and kernel versions, memory, GPU, renderer, guest engine, driver versions and the video codecs the chip has in hardware. They contain no serial numbers, account names or logs.

## Diagnostics and recovery

Use Portal's diagnostics export before reinstalling, through **Save logs** in **Report** or **Export diagnostics** on the recovery screen. The zip is saved to `Download/Portal` and holds the device details, Portal's host logs, setup state and guest state directory. Unrelated personal files are not included, and binary files are left out.

Reports are meant to be posted publicly, so Portal removes file paths, web addresses (other than its own download sources), email addresses, IP and MAC addresses from the logs where it can spot them. Apps can print anything, though, so look the logs over before attaching them.

## Android process limits

Android 12 and newer can stop large child-process trees. If the entire desktop disappears under load, first check whether the device exposes **Disable child process restrictions** in Developer options. System-wide ADB changes should be a last resort and are intentionally not prescribed here because availability and side effects vary by Android build.
