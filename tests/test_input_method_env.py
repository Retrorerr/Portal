"""Exercise the session policy in a real shell, including user overrides."""
import os
from pathlib import Path
import subprocess
import unittest

POLICY = Path(__file__).resolve().parents[1] / "assets/portal-input-method-env.sh"


class InputMethodEnvironmentTests(unittest.TestCase):
    def environment(self, **overrides):
        env = {key: value for key, value in os.environ.items()
               if key not in ("QT_IM_MODULE", "QT_IM_MODULES", "GTK_IM_MODULE")}
        env.update(overrides)
        result = subprocess.check_output(
            ["bash", "-c", '. "$1"; . "$1"; env -0', "policy-test", str(POLICY)],
            env=env,
        )
        return dict(item.decode().split("=", 1) for item in result.split(b"\0") if item)

    def test_default_covers_qt5_and_keeps_qt6_wayland_first(self):
        env = self.environment()
        self.assertEqual(env["QT_IM_MODULE"], "ibus")
        self.assertEqual(env["QT_IM_MODULES"], "wayland;ibus")
        self.assertNotIn("GTK_IM_MODULE", env)

    def test_single_module_user_choice_is_not_overridden_by_a_fallback_list(self):
        for choice in ("fcitx", "none", ""):
            with self.subTest(choice=choice):
                env = self.environment(QT_IM_MODULE=choice)
                self.assertEqual(env["QT_IM_MODULE"], choice)
                self.assertNotIn("QT_IM_MODULES", env)

    def test_module_list_user_choice_does_not_force_qt5_ibus(self):
        for choice in ("wayland;fcitx", ""):
            with self.subTest(choice=choice):
                env = self.environment(QT_IM_MODULES=choice, GTK_IM_MODULE="custom")
                self.assertEqual(env["QT_IM_MODULES"], choice)
                self.assertNotIn("QT_IM_MODULE", env)
                self.assertEqual(env["GTK_IM_MODULE"], "custom")


if __name__ == "__main__":
    unittest.main()
