import hashlib
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import Mock

from document_preview_runtime import Runtime, parse_resources

spec = importlib.util.spec_from_file_location('financial_preview_tests',
    Path(__file__).with_name('qualify-financial-previews.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class PreviewCapacityTests(unittest.TestCase):
    def test_resource_sampler_keeps_every_counter_in_one_owned_exec(self):
        frames = [
            '@cgroup_version\n1\n@memory/memory.limit_in_bytes\n402653184\n'
            '@memory/memory.oom_control\noom_kill_disable 0\nunder_oom 0\noom_kill 0\n'
            '@memory/memory.failcnt\n0\n@memory/memory.max_usage_in_bytes\n23470080\n',
            '@cgroup_version\n2\n@memory.max\n402653184\n'
            '@memory.events\nlow 0\nhigh 0\nmax 0\noom 0\noom_kill 0\n'
            '@memory.current\n23470080\n',
        ]
        for frame in frames:
            runtime = Runtime.__new__(Runtime)
            runtime.renderer_cid = 'owned-renderer'
            runtime.owned_container = Mock()
            runtime.docker = Mock(return_value=frame)
            counters = runtime.resources(renderer=True)
            Runtime.validate_resources(counters, 384 * 1024**2)
            runtime.owned_container.assert_called_once_with('owned-renderer')
            self.assertEqual(runtime.docker.call_count, 1)
            self.assertEqual(runtime.docker.call_args.args[:4],
                             ('exec', 'owned-renderer', '/bin/sh', '-c'))
            for invalid in [frame + '@memory.current\n1\n',
                            frame.replace('402653184', 'unavailable'),
                            frame.replace('oom_kill 0', 'oom_kill nope'),
                            frame.replace('@cgroup_version\n', '@cgroup_version\n3'),
                            frame.split('@')[0], frame.rsplit('@', 1)[0]]:
                with self.assertRaises(RuntimeError):
                    parse_resources(invalid)

    def test_absolute_schedule_keeps_both_shapes_and_full_windows(self):
        for seconds, count in [(60, 12), (600, 120)]:
            rows = m.preview_schedule(seconds)
            self.assertEqual(len(rows), count)
            self.assertEqual(rows[:2], [(1.5, 'png', 0), (6.5, 'pdf', 0)])
            self.assertEqual(rows[-1], (seconds - 3.5, 'pdf', seconds // 10 - 1))
            self.assertEqual(len({(kind, n) for _, kind, n in rows}), count)
        for invalid in [True, 0, 9, 11, 601, 60.0]:
            with self.assertRaises(AssertionError):
                m.preview_schedule(invalid)

    def test_every_arrival_failure_and_busy_response_counts(self):
        rows = [{'kind': kind, 'number': n, 'status': 200, 'verified': True,
                 'wall_ms': 100, 'start_lateness_ms': 1}
                for _, kind, n in m.preview_schedule(60)]
        self.assertEqual(m.evaluate_previews(rows, 60)['result'], 'pass')
        for changed in [rows[:-1], rows + [rows[0]],
                        [dict(r, status=503) if i == 0 else r for i, r in enumerate(rows)],
                        [dict(r, verified=False) if i == 0 else r for i, r in enumerate(rows)],
                        [dict(r, wall_ms=5001) for r in rows],
                        [dict(r, start_lateness_ms=1001) for r in rows],
                        [dict(r, wall_ms=float('nan')) for r in rows],
                        [dict(r, wall_ms=float('inf')) for r in rows],
                        [dict(r, wall_ms=-1) for r in rows],
                        [dict(r, start_lateness_ms=True) for r in rows]]:
            self.assertEqual(m.evaluate_previews(changed, 60)['result'], 'fail')

    def test_pixels_and_identity_are_verified_independently_of_headers(self):
        for kind, page, width, height, color, pages in [
            ('png', 1, 960, 960, (255, 0, 0), 1),
            ('pdf', 1, 720, 960, (255, 0, 0), 2),
            ('pdf', 2, 960, 720, (0, 0, 255), 2),
        ]:
            raw = bytes(color) * (width * height)
            headers = {'Content-Type': 'application/vnd.stateset.rgb8',
                'Cache-Control': 'private, no-store', 'X-Content-Type-Options': 'nosniff',
                'X-Document-Id': 'document', 'X-Load-Id': 'load', 'X-Source-Sha256': 'a' * 64,
                'X-Content-Sha256': hashlib.sha256(raw).hexdigest(),
                'X-Preview-Width': str(width), 'X-Preview-Height': str(height),
                'X-Preview-Page': str(page), 'X-Preview-Pages': str(pages)}
            m.verify_preview(headers, raw, 'document', 'load', 'a' * 64, kind, page)
            for key, value in [('X-Document-Id', 'foreign'), ('X-Load-Id', 'other'),
                               ('X-Source-Sha256', 'b' * 64), ('X-Preview-Page', '3'),
                               ('X-Preview-Width', '1'), ('Cache-Control', 'public')]:
                changed = dict(headers, **{key: value})
                with self.assertRaises(AssertionError):
                    m.verify_preview(changed, raw, 'document', 'load', 'a' * 64, kind, page)
            changed_pixels = bytes([raw[0] ^ 1]) + raw[1:]
            changed = dict(headers, **{'X-Content-Sha256': hashlib.sha256(changed_pixels).hexdigest()})
            with self.assertRaises(AssertionError):
                m.verify_preview(changed, changed_pixels, 'document', 'load', 'a' * 64, kind, page)


if __name__ == '__main__':
    unittest.main()
