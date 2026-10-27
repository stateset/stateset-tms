"""Counter fixtures and negative controls for the resource-envelope observer."""

import importlib.util
import hashlib
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    "api_envelope", Path(__file__).with_name("qualify-api-envelope.py")
)
envelope = importlib.util.module_from_spec(spec)
spec.loader.exec_module(envelope)

V1 = """@memory.events
unavailable
@memory/memory.usage_in_bytes
73400320
@memory/memory.max_usage_in_bytes
157286400
@memory/memory.oom_control
oom_kill_disable 0
under_oom 0
oom_kill 0
@memory/memory.failcnt
0
@memory/memory.limit_in_bytes
536870912
@memory/memory.memsw.limit_in_bytes
536870912
@cpu/cpu.stat
nr_periods 12
nr_throttled 3
throttled_time 42000000
@cpu/cpu.cfs_quota_us
200000
@cpu/cpu.cfs_period_us
100000
@cpuacct/cpuacct.usage
123456789
"""
V2 = """@memory.current
73400320
@memory.peak
157286400
@memory.events
low 0
high 0
max 0
oom 0
oom_kill 0
oom_group_kill 0
@memory.max
536870912
@memory.swap.max
0
@cpu.max
200000 100000
@cpu.stat
usage_usec 123456
user_usec 100000
system_usec 23456
nr_periods 12
nr_throttled 3
throttled_usec 42000
"""


class ResourceCounterTests(unittest.TestCase):
    def test_codec_permission_or_byte_drift_refuses_before_resource_start(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'owned-codec'
            data = b'#!/bin/sh\nexit 0\n'
            path.write_bytes(data)
            expected = hashlib.sha256(data).hexdigest()
            path.chmod(0o700)
            envelope.verify_codec_binary(path, expected)
            path.chmod(0o600)
            with self.assertRaisesRegex(AssertionError, 'executable regular file'):
                envelope.verify_codec_binary(path, expected)
            path.chmod(0o700)
            path.write_bytes(b'#!/bin/sh\nexit 1\n')
            with self.assertRaisesRegex(AssertionError, 'binding mismatch'):
                envelope.verify_codec_binary(path, expected)
            with self.assertRaisesRegex(AssertionError, 'executable regular file'):
                envelope.verify_codec_binary(Path(directory), expected)

    def test_allocator_profile_is_explicit_and_uses_only_declared_arenas(self):
        self.assertEqual(envelope.allocator_environment(None), {})
        for value in [1, 2, 4]:
            self.assertEqual(
                envelope.allocator_environment(value), {"MALLOC_ARENA_MAX": str(value)}
            )

    def test_invalid_allocator_profile_cannot_silently_change_runtime(self):
        for value in [True, False, 0, -1, 3, 8, "1", 1.0, {}, []]:
            with self.subTest(value=value), self.assertRaises(AssertionError):
                envelope.allocator_environment(value)

    def test_v1_actual_units_limits_and_high_water_are_retained(self):
        sample = envelope.parse_cgroup(V1, 512)
        self.assertEqual(sample["cgroup_version"], 1)
        self.assertEqual(sample["memory_peak"], 150 * 2**20)
        self.assertEqual(sample["cpu_stat"]["usage_ns"], 123456789)
        self.assertEqual(sample["cpu_stat"]["throttled_time"], 42000000)
        self.assertNotIn("oom", sample["memory_events"])
        envelope.require_no_memory_faults(sample)

    def test_v2_actual_units_limits_and_high_water_are_retained(self):
        sample = envelope.parse_cgroup(V2, 512)
        self.assertEqual(sample["cgroup_version"], 2)
        self.assertEqual(sample["memory_current"], 70 * 2**20)
        self.assertEqual(sample["cpu_stat"]["usage_usec"], 123456)
        self.assertEqual(sample["cpu_stat"]["throttled_usec"], 42000)
        envelope.require_no_memory_faults(sample)

    def test_missing_oom_counter_cannot_silently_become_zero(self):
        for text in [V1.replace("oom_kill 0\n", ""), V2.replace("oom_kill 0\n", "")]:
            with self.subTest(text=text[:20]), self.assertRaises(AssertionError):
                envelope.parse_cgroup(text, 512)

    def test_oom_kill_is_retained_and_refused_in_either_version(self):
        for text in [V1, V2]:
            sample = envelope.parse_cgroup(
                text.replace("oom_kill 0", "oom_kill 1"), 512
            )
            self.assertEqual(sample["memory_events"]["oom_kill"], 1)
            with self.assertRaises(AssertionError):
                envelope.require_no_memory_faults(sample)

    def test_limit_pressure_without_a_kill_is_not_a_success(self):
        for text in [
            V1.replace("@memory/memory.failcnt\n0", "@memory/memory.failcnt\n4"),
            V2.replace("max 0\n", "max 4\n"),
        ]:
            with self.subTest(text=text[:20]), self.assertRaises(AssertionError):
                envelope.require_no_memory_faults(envelope.parse_cgroup(text, 512))

    def test_under_oom_and_oom_pressure_are_refused(self):
        for text in [
            V1.replace("under_oom 0", "under_oom 1"),
            V2.replace("oom 0\n", "oom 1\n"),
        ]:
            with self.subTest(text=text[:20]), self.assertRaises(AssertionError):
                envelope.require_no_memory_faults(envelope.parse_cgroup(text, 512))

    def test_wrong_memory_limit_is_not_the_requested_envelope(self):
        for text in [V1, V2]:
            with self.assertRaises(AssertionError):
                envelope.parse_cgroup(text, 768)

    def test_swap_allowance_is_not_disabled_swap(self):
        for text in [
            V1.replace(
                "@memory/memory.memsw.limit_in_bytes\n536870912",
                "@memory/memory.memsw.limit_in_bytes\n1073741824",
            ),
            V2.replace("@memory.swap.max\n0", "@memory.swap.max\n536870912"),
        ]:
            with self.subTest(text=text[:20]), self.assertRaises(AssertionError):
                envelope.parse_cgroup(text, 512)

    def test_unlimited_cpu_does_not_count_as_two_cpus(self):
        for text in [
            V1.replace("@cpu/cpu.cfs_quota_us\n200000", "@cpu/cpu.cfs_quota_us\n-1"),
            V2.replace("@cpu.max\n200000", "@cpu.max\nmax"),
        ]:
            with self.subTest(text=text[:20]), self.assertRaises(
                (AssertionError, ValueError)
            ):
                envelope.parse_cgroup(text, 512)

    def test_duplicate_or_unavailable_counter_streams_refuse(self):
        for text in [
            V1 + "@memory.events\nunavailable\n",
            "@memory.events\nunavailable\n",
        ]:
            with self.assertRaises(AssertionError):
                envelope.parse_cgroup(text, 512)


if __name__ == "__main__":
    unittest.main()
