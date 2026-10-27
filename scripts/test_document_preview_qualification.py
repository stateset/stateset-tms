"""Negative controls for the image fixture that deliberately stops child processes."""
import hashlib
import json
import copy
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from document_preview_runtime import Runtime, OWNER_LABEL, validate_image, validate_policy, validate_renderer, RENDERER_MEMORY_BYTES

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('preview_qualification', ROOT / 'scripts/qualify-document-previews.py')
QUALIFICATION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(QUALIFICATION)


class PreviewQualification(unittest.TestCase):
    def test_failed_api_cleanup_still_cleans_renderer_and_attempts_owned_volume(self):
        with tempfile.TemporaryDirectory() as directory:
            runtime = Runtime.__new__(Runtime)
            runtime.directory = Path(directory)
            runtime.cid, runtime.renderer_cid = 'owned-api', 'owned-renderer'
            runtime.socket_volume, runtime.owner = 'owned-volume', 'fixture-owner'
            runtime.docker_prefix = ['docker-fixture']
            calls = []
            runtime.owned_container = lambda cid: dict(State=dict(OOMKilled=False))
            runtime.resources = lambda renderer=False: {
                'cgroup_version': 2, 'memory.max': str(RENDERER_MEMORY_BYTES if renderer else 536870912),
                'memory.events': 'oom_kill 0\n'}

            def docker(*args):
                calls.append(args)
                if args[0] == 'stop' and args[-1] == 'owned-api':
                    raise RuntimeError('injected API stop failure')
                if args[:2] == ('volume', 'inspect'):
                    return '[{"Labels":{"' + OWNER_LABEL + '":"fixture-owner"}}]'
                if args[:2] == ('volume', 'rm'):
                    raise RuntimeError('volume remains attached to API')
                return ''

            runtime.docker = docker
            with patch('document_preview_runtime.subprocess.run',
                       return_value=subprocess.CompletedProcess([], 1)):
                with self.assertRaisesRegex(RuntimeError, 'injected API stop failure'):
                    runtime.__exit__(None, None, None)
            self.assertIn(('stop', '--time', '15', 'owned-renderer'), calls)
            self.assertIn(('rm', 'owned-renderer'), calls)
            self.assertIn(('volume', 'rm', 'owned-volume'), calls)
            self.assertEqual(runtime.cid, 'owned-api')
            self.assertIsNone(runtime.renderer_cid)
            self.assertEqual(runtime.socket_volume, 'owned-volume')
            api = json.loads((runtime.directory / 'cleanup.json').read_text())
            renderer = json.loads((runtime.directory / 'renderer-cleanup.json').read_text())
            self.assertFalse(api['container_removed'])
            self.assertIn('injected API stop failure', api['diagnostic_errors'][0])
            self.assertTrue(renderer['container_removed'])

    def test_renderer_rejects_shared_namespaces_credentials_data_mounts_and_privilege_expansion(self):
        base = dict(Config=dict(User='10002:10001', Entrypoint=['stateset-freight'],
                    Cmd=['--pdf-renderer-service'], Env=['PATH=/usr/bin']),
            HostConfig=dict(NetworkMode='none', PidMode='', IpcMode='private', Privileged=False,
                    Memory=RENDERER_MEMORY_BYTES, MemorySwap=RENDERER_MEMORY_BYTES,
                    NanoCpus=1000000000, PidsLimit=16, ReadonlyRootfs=True,
                    CapDrop=['ALL'], CapAdd=None, SecurityOpt=['no-new-privileges']),
            Mounts=[dict(Type='volume', Name='owned-socket', Destination='/run/freight-pdf', RW=True)])
        validate_renderer(base, 'owned-socket')
        mutations = [('Config', 'User', '10001:10001'), ('Config', 'Cmd', ['--serve']),
            ('Config', 'Entrypoint', ['/bin/sh']), ('Config', 'Env', ['DATABASE_URL=must-not-forward']),
            ('HostConfig', 'NetworkMode', 'host'), ('HostConfig', 'PidMode', 'host'),
            ('HostConfig', 'IpcMode', 'host'), ('HostConfig', 'Privileged', True),
            ('HostConfig', 'Memory', RENDERER_MEMORY_BYTES * 2), ('HostConfig', 'MemorySwap', -1),
            ('HostConfig', 'NanoCpus', 2000000000), ('HostConfig', 'PidsLimit', 0),
            ('HostConfig', 'ReadonlyRootfs', False), ('HostConfig', 'CapDrop', []),
            ('HostConfig', 'CapAdd', ['SYS_ADMIN']),
            ('HostConfig', 'SecurityOpt', ['no-new-privileges', 'seccomp=unconfined'])]
        for section, key, value in mutations:
            mutated = copy.deepcopy(base)
            mutated[section][key] = value
            with self.subTest(section=section, key=key), self.assertRaises(RuntimeError):
                validate_renderer(mutated, 'owned-socket')
        for mounts in [[], [dict(Type='bind', Source='/api-data', Destination='/run/freight-pdf', RW=True)],
                       base['Mounts'] + [dict(Type='volume', Name='api-data', Destination='/data', RW=False)]]:
            with self.subTest(mounts=mounts), self.assertRaises(RuntimeError):
                validate_renderer(dict(base, Mounts=mounts), 'owned-socket')
        with self.assertRaises(RuntimeError):
            validate_renderer(base, 'another-socket')

    def test_worker_policy_must_add_a_filter_and_not_claim_filesystem_isolation(self):
        policy = dict(result='pass', socket_denials=6, spawn_denied=True,
                      no_new_privileges=True, threads=1, filter_count_before=1,
                      filter_count_after=2, filesystem_isolation=False,
                      readonly_file_opens=True, write_open_denials=9)
        validate_policy(policy)
        for mutation in [dict(result='failure'), dict(socket_denials=5), dict(spawn_denied=False),
                         dict(no_new_privileges=False), dict(threads=2), dict(filter_count_before=0),
                         dict(filter_count_after=1), dict(filter_count_before=True),
                         dict(filesystem_isolation=True), dict(readonly_file_opens=False),
                         dict(readonly_file_opens=None), dict(write_open_denials=8),
                         dict(write_open_denials=True), dict(write_open_denials=None)]:
            with self.subTest(mutation=mutation), self.assertRaises(RuntimeError):
                validate_policy(dict(policy, **mutation))

    def test_build_gate_help_and_unknown_options_start_no_build_or_artifacts(self):
        with tempfile.TemporaryDirectory() as directory:
            for option, status in [('--help', 0), ('--unknown-build-profile', 2)]:
                result = subprocess.run([sys.executable, str(ROOT / 'scripts/check-document-preview-image.py'), option],
                    env=dict(os.environ, TMPDIR=directory), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.assertEqual(result.returncode, status)
                self.assertEqual(list(Path(directory).iterdir()), [])
                if status == 0:
                    self.assertIn(b'Build the current production Dockerfile', result.stdout)

    def test_fast_success_must_match_every_expected_pixel_even_with_a_matching_hash(self):
        rgb = bytes([0, 0, 255]) * (720 * 960)
        headers = {'content-type': 'application/vnd.stateset.rgb8', 'x-preview-width': '720',
                   'x-preview-height': '960', 'x-preview-page': '1', 'x-preview-pages': '1',
                   'x-document-id': 'owned-document', 'x-load-id': 'owned-load',
                   'x-source-sha256': 'a' * 64, 'x-content-sha256': hashlib.sha256(rgb).hexdigest()}
        wave = [(200, headers, rgb, 1000)] + [(503, {}, b'busy', 20)] * 11
        QUALIFICATION.validate_wave(wave, 'owned-document', 'owned-load', 'a' * 64)
        wrong = bytes([255]) + rgb[1:]
        forged = dict(headers, **{'x-content-sha256': hashlib.sha256(wrong).hexdigest()})
        with self.assertRaises(RuntimeError):
            QUALIFICATION.validate_wave([(200, forged, wrong, 1000)] + wave[1:],
                                        'owned-document', 'owned-load', 'a' * 64)

    def test_extra_admissions_server_errors_slow_busy_and_late_results_fail_the_gate(self):
        wave = [(422, {}, b'bounded refusal', 4000)] + [(503, {}, b'busy', 20)] * 11
        QUALIFICATION.validate_wave(wave, 'document', 'load', 'a' * 64)
        for change in [[(500, {}, b'error', 10)] + wave[1:],
                       wave[:1] + [(422, {}, b'error', 20)] + wave[2:],
                       wave[:1] + [(503, {}, b'busy', 501)] + wave[2:],
                       [(422, {}, b'late refusal', 7001)] + wave[1:]]:
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                QUALIFICATION.validate_wave(change, 'document', 'load', 'a' * 64)

    def test_clean_image_requires_expected_user_entrypoint_and_no_persistent_volumes(self):
        base = dict(User='10001:10001', Entrypoint=['stateset-freight'], Cmd=None,
                    Env=['PATH=/usr/bin', 'FREIGHT_BIND_ADDR=0.0.0.0:8090'], Volumes=None)
        validate_image(base)
        for mutation in [dict(User='root'), dict(Entrypoint=['/bin/sh']),
                         dict(Cmd=['--migrate']), dict(Volumes={'/data': {}}),
                         dict(Env=['DATABASE_URL=postgres://must-not-connect.invalid/service']),
                         dict(Env=['FREIGHT_IDENTITY_JSON={}']),
                         dict(Env=['AWS_ACCESS_KEY_ID=must-not-use-this-image']),
                         dict(Env=['FREIGHT_INTEGRATIONS_JSON={}'])]:
            with self.subTest(mutation=mutation), self.assertRaises(RuntimeError):
                validate_image(dict(base, **mutation))

    def test_cgroup_v1_requires_real_limit_peak_and_no_child_oom(self):
        base = {'cgroup_version': 1, 'memory/memory.limit_in_bytes': '536870912',
                'memory/memory.oom_control': 'under_oom 0\noom_kill 0\n',
                'memory/memory.max_usage_in_bytes': '49000000'}
        Runtime.validate_resources(base)
        for mutation in [{'memory/memory.limit_in_bytes': '1073741824'},
                         {'memory/memory.max_usage_in_bytes': '536870913'},
                         {'memory/memory.oom_control': 'under_oom 0\noom_kill 1'},
                         {'memory/memory.oom_control': 'under_oom 0'}]:
            with self.subTest(mutation=mutation), self.assertRaises(RuntimeError):
                Runtime.validate_resources(dict(base, **mutation))

    def test_cgroup_v2_does_not_confuse_api_survival_with_child_survival(self):
        base = {'cgroup_version': 2, 'memory.max': '536870912',
                'memory.events': 'oom 0\noom_kill 0\noom_group_kill 0'}
        Runtime.validate_resources(base)
        for events in ['oom 1\noom_kill 1', 'oom 0']:
            with self.subTest(events=events), self.assertRaises(RuntimeError):
                Runtime.validate_resources(dict(base, **{'memory.events': events}))

    def test_existing_report_is_refused_before_docker_or_other_resources_start(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'original.json'
            content = b'{"original":"retain exact bytes"}\n'
            output.write_bytes(content)
            result = subprocess.run([sys.executable, str(ROOT / 'scripts/qualify-document-previews.py'),
                                     '--image', 'must-not-run', '--output', str(output)],
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b'Preserve reports', result.stderr)
            self.assertEqual(output.read_bytes(), content)
            self.assertEqual(sorted(p.name for p in Path(directory).iterdir()), ['original.json'])

    def test_process_signal_requires_both_owned_identity_and_live_renderer_membership(self):
        runtime = Runtime.__new__(Runtime)
        renderer = dict(host=44, namespace=4)
        runtime.renderers = lambda: []
        runtime.docker = lambda *_: self.fail('must not send a signal')
        with self.assertRaises(RuntimeError):
            runtime.signal(renderer, 'KILL')
        with self.assertRaises(RuntimeError):
            runtime.signal(renderer, 'TERM')

    def test_qualification_cannot_stop_or_remove_another_container(self):
        runtime = Runtime.__new__(Runtime)
        runtime.cid = 'existing-but-not-owned'
        runtime.owner = 'expected-owner'
        runtime.docker = lambda *args: '[{"Config":{"Labels":{}}}]'
        with self.assertRaises(RuntimeError):
            runtime.owned()


if __name__ == '__main__':
    unittest.main()
