# Portal diagnostic APK: build, install, and verify

This is the canonical way to run the diagnostic Portal app on an Android
device from this checkout. It matters because the two APKs have different
package ids:

| APK | Package id | Purpose |
| --- | --- | --- |
| Stable | `app.polarbear.portal` | Release/stable Portal |
| Diagnostic | `app.polarbear` | Debug features, pointer injector, and diagnostics |

Do not install a file merely because its filename is `target/Portal-Debug.apk`.
That file is generated and can be stale. Always rebuild it and verify the
package id before installing it.

## Canonical command

From the repository root, on the intended Portal branch:

```powershell
.\scripts\build_android_variants.ps1 -InstallDebug -DeviceId <adb-device-id>
```

For example, the OnePlus Pad 3 currently appears as:

```powershell
.\scripts\build_android_variants.ps1 -InstallDebug -DeviceId 'adb-f105b146-YB8BT6._adb-tls-connect._tcp'
```

The script builds the stable artifact, then explicitly invokes xbuild with
`--manifest manifest.debug.yaml` for the diagnostic artifact. It never
overwrites `manifest.yaml`. It checks that the generated APK declares
`app.polarbear`, copies it to `target/Portal-Debug.apk`, installs it with
`adb install -r -t`, compares the host APK SHA-256 with the installed APK,
and launches `app.polarbear`.

## Why `--manifest` is mandatory for debug builds

The xbuild tool defaults to `manifest.yaml`, which is the stable Portal
manifest. A command such as this can therefore produce a successful APK with
the wrong package:

```powershell
cargo run --manifest-path patches/xbuild/xbuild/Cargo.toml -- build --debug --features portal-debug --platform android --arch arm64 --format apk
```

For a direct xbuild invocation, use the explicit manifest and verify the
package before installing:

```powershell
cargo run --manifest-path patches/xbuild/xbuild/Cargo.toml -- build `
  --manifest manifest.debug.yaml `
  --debug --features portal-debug --platform android --arch arm64 --format apk
```

The `--manifest` option is implemented by the local xbuild source, so the
selection is explicit rather than relying on a temporary tracked-file swap.

## Runtime verification

After launch, verify that the debug package owns the session and that the
Anland payload came from the current APK. The runtime paths below are for the
debug package:

```powershell
adb -s <adb-device-id> shell ps -A -o PID,PPID,NAME,ARGS
adb -s <adb-device-id> shell run-as app.polarbear sha256sum `
  /data/data/app.polarbear/files/runtime-B/usr/local/lib/portal-anland/kwin_wayland `
  /data/data/app.polarbear/files/runtime-B/usr/local/lib/portal-anland/libkwin.so.6.7.4 `
  /data/data/app.polarbear/files/runtime-B/usr/local/lib/portal-anland/kwin/plugins/screencast.so
```

The APK and device APK hashes must match before treating any physical test as
a test of the current source tree. `runtime-B/usr/local/bin/kwin_wayland` is a
wrapper; the Anland executable, library, and patched screencast plugin used by
this branch are under `runtime-B/usr/local/lib/portal-anland/`. The wrapper
sets `QT_PLUGIN_PATH` so KWin loads that app-owned `kwin/plugins/screencast.so`
before Debian's package copy.

## Non-negotiable debug-install checklist

1. Check the branch first: `git branch --show-current` must print
   `upgrade/debian-forky-plasma`; do not build from `main`.
2. Run the canonical script from this checkout:
   `.\scripts\build_android_variants.ps1 -InstallDebug -DeviceId <adb-device-id>`.
3. Do not use a stale `target/Portal-Debug.apk`, a stable APK, or an APK from
   another Portal checkout. The script explicitly selects `manifest.debug.yaml`,
   verifies package id `app.polarbear`, installs with `adb install -r -t`,
   compares the installed APK SHA-256 with the freshly built APK, force-stops,
   and launches the debug package.
4. Verify the three Anland payload hashes above and verify that `kwin_wayland`,
   `plasmashell`, and the Portal app are running from the debug package before
   interpreting a physical result.
5. If the startup veil is present, dismiss it with the debug-only helper:
   `.\scripts\debug-dismiss-veil.ps1 -DeviceId <adb-device-id>`.
