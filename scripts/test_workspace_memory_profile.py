"""Refuse incomplete/misattributed process-memory observations."""

import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "memory_profile_test", Path(__file__).with_name("profile-api-workspace-memory.py")
)
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)

SOURCE = """@@exe
/usr/local/bin/stateset-freight
@@status
Pid: 1
Threads: 3
VmRSS: 30 kB
VmHWM: 40 kB
RssAnon: 20 kB
RssFile: 10 kB
RssShmem: 0 kB
VmSize: 200 kB
@@smaps
1000-3000 rw-p 00000000 00:00 0
Rss: 8 kB
Anonymous: 8 kB
Private_Dirty: 8 kB
Private_Clean: 0 kB
Swap: 0 kB
4000-6000 rw-p 00000000 00:00 0 [heap]
Rss: 4 kB
Anonymous: 4 kB
Private_Dirty: 4 kB
Private_Clean: 0 kB
Swap: 0 kB
@@tasks
1
2
3
"""


class ProcessMemoryControls(unittest.TestCase):
    def test_mapping_residence_is_separate_from_virtual_reservation(self):
        value = profile.parse_process(SOURCE)
        self.assertEqual(value["status"]["VmRSS"], 30 * 1024)
        self.assertEqual(value["anonymous_rw_mapping_count"], 1)
        self.assertEqual(value["anonymous_rw_rss_bytes"], 8 * 1024)
        self.assertEqual(value["heap_rss_bytes"], 4 * 1024)
        self.assertEqual(value["mapping_anonymous_bytes"], 12 * 1024)
        self.assertEqual(
            value["anonymous_rw_mappings"][0]["end"]
            - value["anonymous_rw_mappings"][0]["start"],
            8192,
        )

    def test_wrong_pid_or_executable_refuses(self):
        for value in [
            SOURCE.replace("Pid: 1", "Pid: 2"),
            SOURCE.replace("/usr/local/bin/stateset-freight", "/bin/sh"),
        ]:
            with self.assertRaises(AssertionError):
                profile.parse_process(value)

    def test_absent_or_ambiguous_counters_do_not_become_zero(self):
        for value in [
            SOURCE.replace("RssAnon: 20 kB\n", ""),
            SOURCE.replace("Anonymous: 4 kB\n", ""),
            SOURCE.replace("VmRSS: 30 kB", "VmRSS: 30 MB"),
            SOURCE.replace("VmHWM: 40 kB", "VmHWM: 40 kB\nVmHWM: 50 kB"),
        ]:
            with self.assertRaises(AssertionError):
                profile.parse_process(value)

    def test_source_framing_and_duplicate_tasks_refuse(self):
        for value in [
            SOURCE + "@@exe\n/usr/local/bin/stateset-freight",
            SOURCE.replace("1\n2\n3\n", "1\n2\n2\n"),
            SOURCE.replace("@@smaps", "@@unknown"),
        ]:
            with self.assertRaises(AssertionError):
                profile.parse_process(value)

    def test_task_churn_is_recorded_without_inventing_atomic_snapshot(self):
        value = profile.parse_process(SOURCE.replace("Threads: 3", "Threads: 4"))
        self.assertEqual(value["status"]["Threads"], 4)
        self.assertEqual(len(value["observed_tids"]), 3)


if __name__ == "__main__":
    unittest.main()
