import subprocess

autoconfig_js = """pref("general.config.filename", "localdesktop.cfg");
pref("general.config.obscure_value", 0);
pref("general.config.sandbox_enabled", false);
"""

localdesktop_cfg = """// Auto updated by Portal on each startup, do not edit manually
defaultPref("media.cubeb.sandbox", false);
defaultPref("security.sandbox.content.level", 0);
defaultPref("media.allow-audio-non-utility", true);
defaultPref("media.rdd-process.enabled", false);

try {
  var { SandboxUtils } = ChromeUtils.importESModule("resource://gre/modules/SandboxUtils.sys.mjs");
  SandboxUtils.maybeWarnAboutDisabledContentSandbox = () => {};
  SandboxUtils.observeContentSandboxPref = () => {};
} catch (_) {}
"""

with open("target/autoconfig.js", "w", newline="\n") as f:
    f.write(autoconfig_js)

with open("target/localdesktop.cfg", "w", newline="\n") as f:
    f.write(localdesktop_cfg)

subprocess.check_call(["adb", "-s", "f105b146", "push", "target/autoconfig.js", "/sdcard/autoconfig.js"])
subprocess.check_call(["adb", "-s", "f105b146", "push", "target/localdesktop.cfg", "/sdcard/localdesktop.cfg"])

cmd = """
cat /sdcard/autoconfig.js | run-as app.polarbear sh -c 'cat > /data/data/app.polarbear/files/runtime-B/usr/lib/firefox-esr/defaults/pref/autoconfig.js'
cat /sdcard/localdesktop.cfg | run-as app.polarbear sh -c 'cat > /data/data/app.polarbear/files/runtime-B/usr/lib/firefox-esr/localdesktop.cfg'
rm -f /sdcard/autoconfig.js /sdcard/localdesktop.cfg
"""
subprocess.check_call(["adb", "-s", "f105b146", "shell", cmd])
print("Firefox ESR autoconfig installed successfully!")
