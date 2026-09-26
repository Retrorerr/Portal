"""Host-side tests for the first-run Plasma handoff helper.

The helper runs in Debian/Plasma and uses ``fcntl`` for its process lock. On
Windows, replace only that unavailable lock module while importing so its
pure validation/merge logic can still be exercised; lock behavior itself is
covered by the Linux runtime.
"""

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import types
import unittest
from unittest import mock
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HELPER_PATH = ROOT / "assets" / "localdesktop-apply-initial-appearance.py"

try:
    import fcntl  # noqa: F401
except ModuleNotFoundError:
    fcntl_stub = types.ModuleType("fcntl")
    fcntl_stub.LOCK_EX = 2
    fcntl_stub.LOCK_NB = 4
    fcntl_stub.flock = lambda *_args: None
    sys.modules["fcntl"] = fcntl_stub

spec = importlib.util.spec_from_file_location("initial_setup_helper", HELPER_PATH)
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class LauncherPlanTests(unittest.TestCase):
    def test_native_app_ids_map_to_allowlisted_debian_desktop_entries(self):
        app_ids = ["chatgpt", "gimp", "inkscape", "krita", "libreoffice", "thunderbird", "vlc"]
        self.assertEqual(
            helper.launcher_urls(app_ids),
            [
                "applications:chatgpt.desktop",
                "applications:gimp.desktop",
                "applications:org.inkscape.Inkscape.desktop",
                "applications:org.kde.krita.desktop",
                "applications:libreoffice-startcenter.desktop",
                "applications:thunderbird.desktop",
                "applications:vlc.desktop",
            ],
        )
        with self.assertRaises(helper.ApplyError):
            helper.launcher_urls(["plasma-shell"])

    def test_merge_preserves_unrelated_order_and_is_idempotent(self):
        old = [
            "preferred://browser",
            "applications:vlc.desktop",
            "applications:vlc.desktop",
            "applications:other.desktop",
        ]
        selected = ["applications:gimp.desktop", "applications:vlc.desktop"]
        expected = [
            "preferred://browser",
            "applications:vlc.desktop",
            "applications:other.desktop",
            "applications:gimp.desktop",
        ]

        merged = helper.merge_launcher_urls(old, selected)
        self.assertEqual(merged, expected)
        self.assertEqual(helper.merge_launcher_urls(merged, selected), expected)
        self.assertEqual(helper.merge_launcher_urls(old, []), old)
        self.assertIsNone(helper.parse_widget_ids("+3", require_nonempty=True))
        self.assertIsNone(helper.parse_widget_ids("3, 4", require_nonempty=True))

    def test_proof_is_bound_to_plan_and_exact_selected_launcher_set(self):
        plan = {
            "version": "2",
            "fingerprint": "a" * 64,
            "attempt_id": "123-456",
            "appearance": "dark",
            "scale": "1.25",
            "app_ids": "gimp,vlc",
        }
        proof = dict(plan)
        proof.update(
            launcher_urls="applications:gimp.desktop,applications:vlc.desktop",
            widget_ids="2,3",
            output_id="1",
            observed_scale="1.25",
        )
        proof_path = Path("unused-proof-path")
        with mock.patch.object(
            helper,
            "parse_key_values",
            return_value=proof,
        ):
            self.assertTrue(helper.proof_matches(proof_path, plan))

            wrong_launcher_proof = dict(proof, launcher_urls="applications:gimp.desktop")
            with mock.patch.object(
                helper,
                "parse_key_values",
                return_value=wrong_launcher_proof,
            ):
                self.assertFalse(helper.proof_matches(proof_path, plan))

            duplicate_widget_proof = dict(proof, widget_ids="2,2")
            with mock.patch.object(
                helper,
                "parse_key_values",
                return_value=duplicate_widget_proof,
            ):
                self.assertFalse(helper.proof_matches(proof_path, plan))

            minimal_plan = dict(plan, app_ids="")
            minimal_proof = dict(proof, app_ids="", launcher_urls="", widget_ids="")
            with mock.patch.object(helper, "parse_key_values", return_value=minimal_proof):
                self.assertFalse(helper.proof_matches(proof_path, minimal_plan))

    def test_panel_readback_rejects_stale_attempt_result(self):
        plan = {
            "version": "2",
            "fingerprint": "c" * 64,
            "attempt_id": "77-88",
            "app_ids": "gimp",
        }
        values = {
            "version": "2",
            "fingerprint": plan["fingerprint"],
            "attempt_id": "old-attempt",
            "app_ids": plan["app_ids"],
            "launcher_urls": "applications:gimp.desktop",
            "widget_ids": "5",
        }
        path = Path("unused-panel-result")
        with mock.patch.object(
            helper,
            "read_panel_result_value",
            side_effect=lambda _path, key: values.get(key),
        ):
            self.assertIsNone(helper.read_panel_launcher_result(plan, ["gimp"], path))
        values["attempt_id"] = plan["attempt_id"]
        with mock.patch.object(
            helper,
            "read_panel_result_value",
            side_effect=lambda _path, key: values.get(key),
        ):
            self.assertEqual(helper.read_panel_launcher_result(plan, ["gimp"], path), "5")

    def test_completed_modern_and_legacy_install_markers_make_helper_inert(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            marker = Path(temp_dir) / "runtime-complete"
            plan_path = Path(temp_dir) / "plan"
            plan_path.write_text("not parsed after commit", encoding="ascii")
            with mock.patch.object(helper, "PLAN_PATH", plan_path), mock.patch.object(
                helper, "RUNTIME_COMPLETE_MARKER", marker
            ), mock.patch.object(helper, "parse_plan", side_effect=AssertionError("must stay inert")):
                marker.write_text(
                    "runtime-version\n" + "a" * 64 + "\n" + helper.INSTALLATION_MARKER_KIND + "\n",
                    encoding="ascii",
                )
                self.assertTrue(helper.runtime_is_committed())
                self.assertEqual(helper.apply(), None)

                marker.write_text("runtime-version\n" + "b" * 64 + "\n", encoding="ascii")
                self.assertTrue(helper.runtime_is_committed())
                self.assertEqual(helper.apply(), None)

    @unittest.skipUnless(shutil.which("node"), "Node.js is needed to execute the Plasma script mock")
    def test_generated_plasma_script_updates_only_task_managers_and_retries_cleanly(self):
        plan = {"fingerprint": "b" * 64, "attempt_id": "1-2", "app_ids": "gimp,vlc"}
        app_ids = ["gimp", "vlc"]
        result_path = "/home/portal/.local/state/localdesktop/panel-result"
        script = helper.build_panel_launcher_script(plan, app_ids, result_path)
        minimal_script = helper.build_panel_launcher_script(
            dict(plan, app_ids=""), [], result_path
        )
        node_source = f"""
const vm = require('vm');
const assert = require('assert');
const script = {json.dumps(script)};
const minimalScript = {json.dumps(minimal_script)};
function widget(id, type, launchers) {{
  return {{
    id, type, currentConfigGroup: [], config: {{launchers}}, writes: 0, reloads: 0,
    readConfig(key, fallback) {{
      const value = this.config[key] === undefined ? fallback : this.config[key];
      if (key !== 'launchers' || !Array.isArray(value)) return value;
      const sequence = {{length: value.length, [Symbol.toStringTag]: 'V4Sequence'}};
      value.forEach((entry, index) => {{ sequence[index] = entry; }});
      return sequence;
    }},
    writeConfig(key, value) {{ this.config[key] = value.slice(); this.writes++; }},
    reloadConfig() {{ this.reloads++; }}
  }};
}}
const taskManager = widget(10, 'org.kde.plasma.taskmanager', [
  'preferred://browser', 'preferred://terminal', 'applications:vlc.desktop',
  'applications:vlc.desktop', 'applications:other.desktop',
  'applications:org.kde.konsole.desktop', 'applications:org.kde.konsole.desktop'
]);
const iconTasks = widget(20, 'org.kde.plasma.icontasks', []);
const launcher = widget(30, 'org.kde.plasma.kickoff', ['applications:old.desktop']);
const defaultTaskManager = widget(40, 'org.kde.plasma.taskmanager', undefined);
const all = [taskManager, iconTasks, launcher, defaultTaskManager];
const containment = {{widgetIds: all.map((entry) => entry.id), widgetById(id) {{ return all.find((entry) => entry.id === id); }}}};
let resultEntries = {{}};
function ConfigFile(path, group) {{
  assert.strictEqual(path, {json.dumps(result_path)});
  assert.strictEqual(group, 'PortalInitialSetup');
  this.writeEntry = (key, value) => {{ resultEntries[key] = value; }};
}}
const context = {{panels: () => [containment], ConfigFile}};
vm.runInNewContext(script, context);
assert.deepStrictEqual(Array.from(taskManager.config.launchers), [
  'applications:org.kde.konsole.desktop', 'preferred://browser', 'applications:vlc.desktop',
  'applications:other.desktop', 'applications:gimp.desktop'
]);
assert.deepStrictEqual(Array.from(iconTasks.config.launchers), [
  'applications:org.kde.konsole.desktop', 'applications:gimp.desktop', 'applications:vlc.desktop'
]);
assert.deepStrictEqual(Array.from(launcher.config.launchers), ['applications:old.desktop']);
assert.strictEqual(resultEntries.widget_ids, '10,20,40');
assert.strictEqual(resultEntries.launcher_urls, 'applications:gimp.desktop,applications:vlc.desktop');
assert.deepStrictEqual(Array.from(defaultTaskManager.config.launchers), [
  'applications:systemsettings.desktop', 'applications:org.kde.konsole.desktop', 'preferred://filemanager',
  'preferred://browser',
  'applications:gimp.desktop', 'applications:vlc.desktop'
]);
assert.strictEqual(defaultTaskManager.writes, 1);
const firstWriteCounts = all.map((entry) => entry.writes);
vm.runInNewContext(script, context);
assert.deepStrictEqual(all.map((entry) => entry.writes), firstWriteCounts);
const minimalTaskManager = widget(50, 'org.kde.plasma.taskmanager', []);
vm.runInNewContext(minimalScript, {{
  panels: () => [{{widgetIds: [50], widgetById: () => minimalTaskManager}}], ConfigFile
}});
assert.deepStrictEqual(Array.from(minimalTaskManager.config.launchers), [
  'applications:org.kde.konsole.desktop'
]);
assert.strictEqual(resultEntries.launcher_urls, '');
assert.strictEqual(resultEntries.widget_ids, '50');
let failedResultEntries = {{}};
function FailedConfigFile() {{ this.writeEntry = (key, value) => {{ failedResultEntries[key] = value; }}; }}
assert.throws(
  () => vm.runInNewContext(script, {{panels: () => [], ConfigFile: FailedConfigFile}}),
  /No Task Manager applet exists/
);
assert.deepStrictEqual(failedResultEntries, {{}});
"""
        result = subprocess.run(
            ["node", "-e", node_source],
            check=False,
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
