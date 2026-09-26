import unittest

from scripts.guest_exec import find_process_pid


class GuestExecProcessLookupTests(unittest.TestCase):
    def test_matches_exact_process_name_not_another_process_command_line(self):
        process_table = (
            "USER PID PPID VSZ RSS WCHAN ADDR S NAME\n"
            "u0_a509 20191 20084 1768716 186204 0 0 S kwin_wayland\n"
            "u0_a509 20381 20076 145000 72000 0 0 S plasmashell\n"
        )
        self.assertEqual(find_process_pid(process_table, "plasmashell"), "20381")

    def test_does_not_report_shell_when_only_kwin_remains(self):
        process_table = (
            "USER PID PPID VSZ RSS WCHAN ADDR S NAME\n"
            "u0_a509 20191 20084 1768716 186204 0 0 S kwin_wayland\n"
        )
        self.assertIsNone(find_process_pid(process_table, "plasmashell"))


if __name__ == "__main__":
    unittest.main()
