#!/usr/bin/python3
"""Apply one immutable first-run desktop plan during provisional setup.

This helper is launched by Plasma autostart. It is intentionally inert unless
native setup has staged a plan, and it never reapplies a plan after the
matching attempt has a verified proof.
"""

import fcntl
import json
import math
import os
import re
import stat
import subprocess
import sys
import time
from pathlib import Path


PLAN_PATH = Path("/var/lib/localdesktop/initial-setup-plan-v2")
PROOF_RELATIVE_PATH = Path(".local/state/localdesktop/initial-setup-proof-v2")
PANEL_RESULT_RELATIVE_PATH = Path(".local/state/localdesktop/panel-launchers-result-v2")
STATE_RELATIVE_DIR = Path(".local/state/localdesktop")
KWAIN_OUTPUT_CONFIG = Path(".config/kwinoutputconfig.json")
KDEGLOBALS = Path(".config/kdeglobals")
RUNTIME_COMPLETE_MARKER = Path("/.portal-runtime-complete")
INSTALLATION_MARKER_KIND = "portal-installation-v1"
PANEL_RESULT_GROUP = "PortalInitialSetup"
EXPECTED_PLAN_KEYS = {
    "version",
    "fingerprint",
    "attempt_id",
    "appearance",
    "scale",
    "app_ids",
}
EXPECTED_PROOF_KEYS = EXPECTED_PLAN_KEYS | {
    "launcher_urls",
    "widget_ids",
    "output_id",
    "observed_scale",
}
SCALE_RE = re.compile(r"(?:0|[1-9][0-9]*)(?:\.[0-9]+)?\Z")
FINGERPRINT_RE = re.compile(r"[0-9a-f]{64}\Z")
ATTEMPT_RE = re.compile(r"[A-Za-z0-9._-]{1,128}\Z")
MAX_WAIT_SECONDS = 60
MAX_PANEL_WAIT_SECONDS = 20

# Native OptionalApp IDs map to the desktop-entry IDs shipped by Debian
# Trixie. The Compose labels, package names, and desktop-file IDs never become
# shell fragments; all three mappings are fixed allowlists.
APP_DESKTOP_FILES = {
    "chatgpt": "chatgpt.desktop",
    "gimp": "gimp.desktop",
    "inkscape": "org.inkscape.Inkscape.desktop",
    "krita": "org.kde.krita.desktop",
    "libreoffice": "libreoffice-startcenter.desktop",
    "thunderbird": "thunderbird.desktop",
    "vlc": "vlc.desktop",
}
TASK_MANAGER_TYPES = ("org.kde.plasma.taskmanager", "org.kde.plasma.icontasks")
# When a first-run applet has never saved its StringList, Plasma scripting
# returns the supplied fallback rather than the KConfig schema default. These
# are Plasma 6.3.6's shipped defaults without the unavailable Discover
# shortcut. preferred://filemanager already resolves to Dolphin, so adding
# Dolphin explicitly would create two identical panel icons. Once saved, the
# user-owned list is read unchanged and becomes fully authoritative.
TASK_MANAGER_DEFAULT_LAUNCHERS = (
    "applications:systemsettings.desktop",
    "applications:org.kde.konsole.desktop",
    "preferred://filemanager",
    "preferred://browser",
)
KONSOLE_LAUNCHER = "applications:org.kde.konsole.desktop"


class ApplyError(RuntimeError):
    pass


def log(message):
    try:
        log_path = Path.home() / STATE_RELATIVE_DIR / "initial-appearance.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)
        with log_path.open("a", encoding="utf-8") as stream:
            stream.write(f"{int(time.time())} {message}\n")
    except OSError:
        pass


def parse_key_values(path, expected_keys):
    try:
        lines = path.read_text(encoding="ascii").splitlines()
    except OSError as error:
        raise ApplyError(f"cannot read {path}: {error}") from error

    values = {}
    for line in lines:
        key, separator, value = line.partition("=")
        if not separator or not key or key in values or key not in expected_keys:
            raise ApplyError(f"invalid or duplicate field in {path}")
        values[key] = value
    if values.keys() != expected_keys:
        raise ApplyError(f"incomplete fields in {path}")
    return values


def parse_plan():
    values = parse_key_values(PLAN_PATH, EXPECTED_PLAN_KEYS)
    if values["version"] != "2":
        raise ApplyError("unsupported initial setup plan version")
    if not FINGERPRINT_RE.fullmatch(values["fingerprint"]):
        raise ApplyError("invalid plan fingerprint")
    if not ATTEMPT_RE.fullmatch(values["attempt_id"]):
        raise ApplyError("invalid setup attempt id")
    if values["appearance"] not in ("dark", "light"):
        raise ApplyError("invalid requested appearance")
    if not SCALE_RE.fullmatch(values["scale"]):
        raise ApplyError("invalid requested scale")
    scale = float(values["scale"])
    if not math.isfinite(scale) or not 0 < scale <= 5:
        raise ApplyError("requested scale is outside KWin's supported range")
    app_ids = parse_app_ids(values["app_ids"])
    if app_ids != sorted(app_ids) or len(app_ids) != len(set(app_ids)):
        raise ApplyError("selected app IDs must be strictly sorted and unique")
    return values, scale


def parse_app_ids(encoded):
    if not encoded:
        return []
    app_ids = encoded.split(",")
    if any(app_id not in APP_DESKTOP_FILES for app_id in app_ids):
        raise ApplyError("initial setup plan contains an unknown app ID")
    return app_ids


def launcher_urls(app_ids):
    try:
        return [f"applications:{APP_DESKTOP_FILES[app_id]}" for app_id in app_ids]
    except KeyError as error:
        raise ApplyError("initial setup plan contains an unknown app ID") from error


def merge_launcher_urls(existing, additions):
    """Preserve unrelated entries while keeping each selected launcher once."""
    if isinstance(existing, str):
        result = [] if not existing else existing.split(",")
    elif isinstance(existing, (list, tuple)):
        result = list(existing)
        if any(not isinstance(value, str) for value in result):
            raise ApplyError("Plasma task manager returned a non-string launcher")
    else:
        raise ApplyError("Plasma task manager returned an invalid launcher list")
    selected = set(additions)
    merged = []
    seen_selected = set()
    for launcher in result:
        if launcher in selected:
            if launcher in seen_selected:
                continue
            seen_selected.add(launcher)
        merged.append(launcher)
    for launcher in additions:
        if launcher not in seen_selected:
            merged.append(launcher)
            seen_selected.add(launcher)
    return merged


def build_panel_launcher_script(plan, app_ids, result_path):
    """Pin selections once in every existing Task Manager applet."""
    expected_ids = json.dumps(app_ids, separators=(",", ":"))
    expected_urls = json.dumps(launcher_urls(app_ids), separators=(",", ":"))
    result_file = json.dumps(str(result_path), ensure_ascii=True)
    fingerprint = json.dumps(plan["fingerprint"], ensure_ascii=True)
    attempt_id = json.dumps(plan["attempt_id"], ensure_ascii=True)
    app_ids_value = json.dumps(plan["app_ids"], ensure_ascii=True)
    return f"""(function () {{
    var expectedAppIds = {expected_ids};
    var expectedLaunchers = {expected_urls};
    var konsoleLauncher = {json.dumps(KONSOLE_LAUNCHER)};
    var widgetIds = [];

    function launcherList(value) {{
        if (Array.isArray(value)) return value.slice();
        if (typeof value === "string") return value.length ? value.split(",") : [];
        if (value === undefined || value === null) return [];
        // Plasma 6's scripting API exposes QStringList as a V4Sequence.
        if (Object.prototype.toString.call(value) === "[object V4Sequence]" &&
            typeof value.length === "number") {{
            var launchers = [];
            for (var index = 0; index < value.length; index++) {{
                if (typeof value[index] !== "string")
                    throw new Error("Task Manager launcher is not a string");
                launchers.push(value[index]);
            }}
            return launchers;
        }}
        throw new Error("Task Manager launchers have an unsupported type");
    }}

    var allPanels = panels();
    for (var panelIndex = 0; panelIndex < allPanels.length; panelIndex++) {{
        var panel = allPanels[panelIndex];
        if (!panel) continue;
        var appletIds = panel.widgetIds;
        for (var appletIndex = 0; appletIndex < appletIds.length; appletIndex++) {{
            var widget = panel.widgetById(appletIds[appletIndex]);
            if (!widget || {json.dumps(list(TASK_MANAGER_TYPES))}.indexOf(widget.type) === -1) continue;

            widget.currentConfigGroup = ["General"];
            var stored = launcherList(widget.readConfig("launchers", []));
            var current = launcherList(widget.readConfig(
                "launchers", {json.dumps(TASK_MANAGER_DEFAULT_LAUNCHERS)}
            ));
            var merged = [];
            var seenSelected = [];
            for (var currentIndex = 0; currentIndex < current.length; currentIndex++) {{
                var currentLauncher = current[currentIndex];
                if (currentLauncher === konsoleLauncher || currentLauncher === "preferred://terminal")
                    continue;
                if (expectedLaunchers.indexOf(currentLauncher) !== -1) {{
                    if (seenSelected.indexOf(currentLauncher) !== -1) continue;
                    seenSelected.push(currentLauncher);
                }}
                merged.push(currentLauncher);
            }}
            var settingsIndex = merged.indexOf("applications:systemsettings.desktop");
            merged.splice(settingsIndex < 0 ? 0 : settingsIndex + 1, 0, konsoleLauncher);
            for (var selectedIndex = 0; selectedIndex < expectedLaunchers.length; selectedIndex++) {{
                if (seenSelected.indexOf(expectedLaunchers[selectedIndex]) === -1) {{
                    merged.push(expectedLaunchers[selectedIndex]);
                    seenSelected.push(expectedLaunchers[selectedIndex]);
                }}
            }}
            // readConfig returns the fallback when this applet has never
            // saved launchers. An identical fallback is not a persisted key.
            var changed = stored.length === 0 || merged.length !== current.length;
            if (!changed) {{
                for (var compareIndex = 0; compareIndex < merged.length; compareIndex++) {{
                    if (merged[compareIndex] !== current[compareIndex]) {{
                        changed = true;
                        break;
                    }}
                }}
            }}
            if (changed) {{
                widget.writeConfig("launchers", merged);
                widget.reloadConfig();
            }}

            var actual = launcherList(widget.readConfig("launchers", []));
            if (actual.filter(function (launcher) {{ return launcher === konsoleLauncher; }}).length !== 1)
                throw new Error("Task Manager launcher readback did not contain exactly one Konsole");
            for (var verifyIndex = 0; verifyIndex < expectedLaunchers.length; verifyIndex++) {{
                var occurrences = 0;
                for (var actualIndex = 0; actualIndex < actual.length; actualIndex++) {{
                    if (actual[actualIndex] === expectedLaunchers[verifyIndex]) occurrences++;
                }}
                if (occurrences !== 1) {{
                    throw new Error("Task Manager launcher readback did not contain exactly one selected app");
                }}
            }}
            widgetIds.push(String(widget.id));
        }}
    }}

    if (widgetIds.length === 0) {{
        throw new Error("No Task Manager applet exists in a Plasma panel");
    }}

    var result = new ConfigFile({result_file}, {json.dumps(PANEL_RESULT_GROUP)});
    result.writeEntry("version", "2");
    result.writeEntry("fingerprint", {fingerprint});
    result.writeEntry("attempt_id", {attempt_id});
    result.writeEntry("app_ids", {app_ids_value});
    result.writeEntry("launcher_urls", expectedLaunchers.join(","));
    result.writeEntry("widget_ids", widgetIds.join(","));
}})();"""


def runtime_is_committed():
    """Recognize Portal's completion marker, not the image-extraction marker."""
    try:
        lines = [line.strip() for line in RUNTIME_COMPLETE_MARKER.read_text(encoding="ascii").splitlines()]
    except OSError:
        return False
    if len(lines) == 3 and lines[2] == INSTALLATION_MARKER_KIND:
        return True
    # Older Portal installs have a valid two-line completion marker. They are
    # already user-owned desktops too, so a stale pending plan must not reset
    # their appearance after upgrade.
    return (
        len(lines) == 2
        and bool(lines[0])
        and len(lines[1]) == 64
        and all(character in "0123456789abcdefABCDEF" for character in lines[1])
    )


def run_checked(arguments, timeout=10):
    try:
        result = subprocess.run(
            arguments,
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ApplyError(f"could not run {arguments[0]}: {error}") from error
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip().replace("\n", " ")
        raise ApplyError(f"{arguments[0]} exited {result.returncode}: {detail[:300]}")
    return result.stdout


def read_live_output():
    output = run_checked(["kscreen-doctor", "--json"])
    try:
        document = json.loads(output)
    except json.JSONDecodeError as error:
        raise ApplyError("kscreen-doctor returned invalid JSON") from error
    outputs = document.get("outputs") if isinstance(document, dict) else None
    if not isinstance(outputs, list):
        raise ApplyError("kscreen-doctor JSON has no outputs array")

    active = [
        item
        for item in outputs
        if isinstance(item, dict)
        and item.get("connected") is True
        and item.get("enabled", True) is not False
    ]
    if len(active) != 1:
        raise ApplyError(f"expected exactly one connected/enabled output, got {len(active)}")
    selected = active[0]
    output_id = selected.get("id")
    name = selected.get("name")
    scale = selected.get("scale")
    if isinstance(output_id, bool) or not isinstance(output_id, int) or output_id <= 0:
        raise ApplyError("KScreen output id is not a positive integer")
    if not isinstance(name, str) or not name:
        raise ApplyError("KScreen output name is missing")
    if isinstance(scale, bool) or not isinstance(scale, (int, float)):
        raise ApplyError("KScreen output scale is not numeric")
    return output_id, name, float(scale)


def quantized_scale(requested):
    # KWin 6.3 Wayland OutputManagement rounds scale to increments of 1/120.
    return math.floor(requested * 120.0 + 0.5) / 120.0


def scale_matches(value, expected):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and abs(value - expected) <= 0.001


def read_persisted_scale(output_name):
    try:
        document = json.loads((Path.home() / KWAIN_OUTPUT_CONFIG).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(document, list):
        return None
    matches = []
    for group in document:
        if not isinstance(group, dict) or group.get("name") != "outputs":
            continue
        outputs = group.get("data")
        if not isinstance(outputs, list):
            continue
        matches.extend(
            item
            for item in outputs
            if isinstance(item, dict) and item.get("connectorName") == output_name
        )
    if len(matches) != 1:
        return None
    scale = matches[0].get("scale")
    if isinstance(scale, bool) or not isinstance(scale, (int, float)) or not math.isfinite(scale):
        return None
    return float(scale)


def read_color_scheme():
    try:
        return run_checked(
            [
                "kreadconfig6",
                "--file",
                str(Path.home() / KDEGLOBALS),
                "--group",
                "General",
                "--key",
                "ColorScheme",
            ],
            timeout=5,
        ).strip()
    except ApplyError:
        return None


def read_lookandfeel_package():
    try:
        return run_checked(
            [
                "kreadconfig6",
                "--file",
                str(Path.home() / KDEGLOBALS),
                "--group",
                "KDE",
                "--key",
                "LookAndFeelPackage",
            ],
            timeout=5,
        ).strip()
    except ApplyError:
        return None


def read_panel_result_value(path, key):
    try:
        return run_checked(
            [
                "kreadconfig6",
                "--file",
                str(path),
                "--group",
                PANEL_RESULT_GROUP,
                "--key",
                key,
            ],
            timeout=5,
        ).strip()
    except ApplyError:
        return None


def parse_widget_ids(encoded, require_nonempty):
    if not encoded:
        if require_nonempty:
            return None
        return []
    values = encoded.split(",")
    if any(not value.isascii() or not value.isdecimal() for value in values):
        return None
    try:
        parsed = [int(value, 10) for value in values]
    except ValueError:
        return None
    if any(value <= 0 for value in parsed) or len(parsed) != len(set(parsed)):
        return None
    return parsed


def read_panel_launcher_result(plan, app_ids, result_path):
    expected_urls = ",".join(launcher_urls(app_ids))
    values = {
        key: read_panel_result_value(result_path, key)
        for key in ("version", "fingerprint", "attempt_id", "app_ids", "launcher_urls", "widget_ids")
    }
    widget_ids = parse_widget_ids(values["widget_ids"] or "", require_nonempty=True)
    if (
        values["version"] == "2"
        and values["fingerprint"] == plan["fingerprint"]
        and values["attempt_id"] == plan["attempt_id"]
        and values["app_ids"] == plan["app_ids"]
        and values["launcher_urls"] == expected_urls
        and widget_ids
    ):
        return ",".join(str(value) for value in widget_ids)
    return None


def prepare_panel_result_path(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    if not stat.S_ISREG(metadata.st_mode) or path.is_symlink():
        raise ApplyError("refusing to replace an unsafe Plasma panel result path")
    path.unlink()


def wait_for_appearance(expected_lookandfeel, expected_scheme):
    deadline = time.monotonic() + MAX_WAIT_SECONDS
    while time.monotonic() < deadline:
        if (
            read_lookandfeel_package() == expected_lookandfeel
            and read_color_scheme() == expected_scheme
        ):
            return
        time.sleep(0.5)
    raise ApplyError("KDE look-and-feel and application color scheme did not persist")


def wait_for_scale(output_name, expected):
    deadline = time.monotonic() + MAX_WAIT_SECONDS
    while time.monotonic() < deadline:
        try:
            output_id, current_name, live_scale = read_live_output()
        except ApplyError:
            time.sleep(0.5)
            continue
        persisted_scale = read_persisted_scale(output_name)
        if (
            current_name == output_name
            and scale_matches(live_scale, expected)
            and scale_matches(persisted_scale, expected)
        ):
            return output_id, live_scale
        time.sleep(0.5)
    raise ApplyError("KScreen live output and persisted KWin scale did not converge")


def proof_matches(path, plan):
    try:
        proof = parse_key_values(path, EXPECTED_PROOF_KEYS)
    except ApplyError:
        return False
    if not all(proof.get(key) == value for key, value in plan.items()):
        return False
    if not proof["output_id"].isdecimal() or int(proof["output_id"]) <= 0:
        return False
    try:
        observed_scale = float(proof["observed_scale"])
    except ValueError:
        return False
    requested_scale = float(plan["scale"])
    if not math.isfinite(observed_scale) or not scale_matches(
        observed_scale, quantized_scale(requested_scale)
    ):
        return False
    app_ids = parse_app_ids(plan["app_ids"])
    expected_urls = ",".join(launcher_urls(app_ids))
    if proof["launcher_urls"] != expected_urls:
        return False
    widget_ids = parse_widget_ids(proof["widget_ids"], require_nonempty=True)
    return widget_ids is not None


def write_proof(path, plan, output_id, observed_scale, widget_ids):
    path.parent.mkdir(parents=True, exist_ok=True)
    fields = dict(plan)
    fields["launcher_urls"] = ",".join(launcher_urls(parse_app_ids(plan["app_ids"])))
    fields["widget_ids"] = widget_ids
    fields["output_id"] = str(output_id)
    fields["observed_scale"] = format(observed_scale, ".12g")
    content = "".join(f"{key}={fields[key]}\n" for key in (
        "version", "fingerprint", "attempt_id", "appearance", "scale", "app_ids",
        "launcher_urls", "widget_ids", "output_id", "observed_scale"
    ))
    temporary = path.with_name(path.name + ".tmp")
    with temporary.open("w", encoding="ascii") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    directory_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)


def apply():
    if runtime_is_committed() or not PLAN_PATH.is_file():
        return
    plan, requested_scale = parse_plan()
    state_dir = Path.home() / STATE_RELATIVE_DIR
    state_dir.mkdir(parents=True, exist_ok=True)
    proof_path = Path.home() / PROOF_RELATIVE_PATH
    panel_result_path = Path.home() / PANEL_RESULT_RELATIVE_PATH

    lock_path = state_dir / "initial-appearance.lock"
    with lock_path.open("a", encoding="ascii") as lock:
        fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        if proof_matches(proof_path, plan):
            return

        app_ids = parse_app_ids(plan["app_ids"])
        for app_id in app_ids:
            desktop_path = Path("/usr/share/applications") / APP_DESKTOP_FILES[app_id]
            if not desktop_path.is_file():
                raise ApplyError(f"selected app desktop entry is missing: {app_id}")
        if not (Path("/usr/share/applications") / "org.kde.konsole.desktop").is_file():
            raise ApplyError("Konsole desktop entry is missing")
        prepare_panel_result_path(panel_result_path)
        script = build_panel_launcher_script(plan, app_ids, panel_result_path)
        dbus_arguments = [
            "qdbus6",
            "org.kde.plasmashell",
            "/PlasmaShell",
            "org.kde.PlasmaShell.evaluateScript",
            script,
        ]
        panel_deadline = time.monotonic() + MAX_PANEL_WAIT_SECONDS
        last_panel_error = None
        widget_ids = None
        while time.monotonic() < panel_deadline:
            try:
                run_checked(
                    dbus_arguments,
                    timeout=max(1, panel_deadline - time.monotonic()),
                )
            except ApplyError as error:
                # XDG autostart may run before the default panel/applets
                # are ready. Retry the idempotent Plasma API operation for
                # this bounded window; retain the real last error if it
                # never becomes available.
                last_panel_error = error
            widget_ids = read_panel_launcher_result(plan, app_ids, panel_result_path)
            if widget_ids is not None:
                break
            time.sleep(0.5)
        if widget_ids is None:
            if last_panel_error is not None:
                raise ApplyError(
                    "Plasma did not validate the selected taskbar launchers: "
                    f"{last_panel_error}"
                ) from last_panel_error
            raise ApplyError("Plasma did not validate the selected taskbar launchers")

        if plan["appearance"] == "dark":
            lookandfeel = "org.kde.breezedark.desktop"
            scheme = "BreezeDark"
        else:
            lookandfeel = "org.kde.breeze.desktop"
            scheme = "BreezeLight"
        run_checked(["plasma-apply-lookandfeel", "--apply", lookandfeel], timeout=30)
        run_checked(["plasma-apply-colorscheme", scheme], timeout=20)
        wait_for_appearance(lookandfeel, scheme)

        output_id, output_name, _ = read_live_output()
        run_checked(
            ["kscreen-doctor", f"output.{output_id}.scale.{plan['scale']}"],
            timeout=20,
        )
        expected_scale = quantized_scale(requested_scale)
        output_id, observed_scale = wait_for_scale(output_name, expected_scale)

        # Re-read both states immediately before publishing the attempt-bound
        # proof, so a transient or stale live snapshot cannot satisfy setup.
        current_id, current_name, current_scale = read_live_output()
        persisted_scale = read_persisted_scale(output_name)
        if (
            current_id != output_id
            or current_name != output_name
            or not scale_matches(current_scale, expected_scale)
            or not scale_matches(persisted_scale, expected_scale)
            or read_lookandfeel_package() != lookandfeel
            or read_color_scheme() != scheme
        ):
            raise ApplyError("appearance/output readback changed before proof commit")
        write_proof(proof_path, plan, output_id, observed_scale, widget_ids)
        log(
            f"proof-written attempt={plan['attempt_id']} output={output_id} "
            f"requested={plan['scale']} observed={format(observed_scale, '.12g')} "
            f"lookandfeel={lookandfeel} scheme={scheme} "
            f"apps={plan['app_ids']} widgets={widget_ids}"
        )


def main():
    try:
        apply()
    except (ApplyError, BlockingIOError, OSError) as error:
        log(f"apply-failed error={str(error)[:300]}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
