import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('portal_wallet_unlock',
    Path(__file__).resolve().parents[1] / 'assets/portal-kwallet-unlock.py')
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class WalletUnlockTests(unittest.TestCase):
    def test_login_uses_base_dbus_client_without_python_desktop_bindings(self):
        with patch.object(helper, 'first_use_defaults'), patch.object(helper, 'wallet_salt', return_value=b's' * 56), \
             patch.object(helper.subprocess, 'run', side_effect=[
                 subprocess.CompletedProcess([], 0, '', ''),
                 subprocess.CompletedProcess([], 0, 'b true\n', '')]) as run:
            self.assertEqual(helper.main(), 0)
            opening, check = [call.args[0] for call in run.call_args_list]
            self.assertEqual(opening[:4], ['busctl', '--user', '--timeout=60', 'call'])
            self.assertEqual(opening[7:11], ['pamOpen', 'sayi', 'kdewallet', '56'])
            self.assertEqual(len(opening[11:-1]), 56)
            self.assertEqual(opening[-1], '0')
            self.assertEqual(check[-3:], ['isOpen', 's', 'kdewallet'])

    def test_an_existing_wallet_without_pam_salt_is_left_to_its_password_prompt(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(helper, 'WALLET_DIR', Path(tmp)), \
             patch.object(helper, 'first_use_defaults'), patch.object(helper.subprocess, 'run') as run:
            wallet = Path(tmp) / 'kdewallet.kwl'
            wallet.write_bytes(b'Existing user wallet')
            self.assertEqual(helper.main(), 0)
            run.assert_not_called()
            self.assertEqual(wallet.read_bytes(), b'Existing user wallet')
            self.assertFalse((Path(tmp) / 'kdewallet.salt').exists())

    def test_a_successful_dbus_call_does_not_hide_a_locked_wallet(self):
        with patch.object(helper, 'first_use_defaults'), patch.object(helper, 'wallet_salt', return_value=b's' * 56), \
             patch.object(helper.subprocess, 'run', side_effect=[
                 subprocess.CompletedProcess([], 0, '', ''),
                 subprocess.CompletedProcess([], 0, 'b false\n', '')]), \
             patch('sys.stderr') as stderr:
            helper.main()
            self.assertIn('wallet remains locked', ''.join(c.args[0] for c in stderr.write.call_args_list))


if __name__ == '__main__':
    unittest.main()
