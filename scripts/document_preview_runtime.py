"""Owned, local-only API image fixture for document-preview fault qualification."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import urllib.error
import urllib.request
import uuid

VIEWER = 'owned-pdf-image-viewer-fixture-credential-32'
OPERATOR = 'owned-pdf-image-operator-fixture-credential-32'
MEMORY_BYTES = 512 * 1024 * 1024
RENDERER_MEMORY_BYTES = 384 * 1024 * 1024
RESPONSE_BYTES = 12 * 1024 * 1024
OWNER_LABEL = 'com.stateset.freight.preview-qualification'

# One exec per sample: the observer must not create a Docker process for every
# individual counter while API traffic is being measured. Missing files still
# fail the command; the framing/parser never substitutes a zero.
RESOURCE_PROBE = r'''set -eu
if [ -f /sys/fs/cgroup/memory.max ]; then
    printf '@cgroup_version\n2\n'
    names='memory.max memory.events memory.current'
else
    printf '@cgroup_version\n1\n'
    names='memory/memory.limit_in_bytes memory/memory.oom_control memory/memory.failcnt memory/memory.max_usage_in_bytes'
fi
for name in $names; do
    printf '@%s\n' "$name"
    cat "/sys/fs/cgroup/$name"
    printf '\n'
done
'''


def parse_resources(text):
    result = {}
    require(text.startswith('@'), 'Missing resource counter framing')
    for section in text.split('@')[1:]:
        key, separator, value = section.partition('\n')
        require(separator and key not in result and value.strip(),
                'Missing or duplicate resource counter')
        result[key] = value.strip()
    version = result.get('cgroup_version')
    require(version in ('1', '2'), 'Unknown cgroup version')
    names = (['memory.max', 'memory.events', 'memory.current'] if version == '2' else
             ['memory/memory.limit_in_bytes', 'memory/memory.oom_control',
              'memory/memory.failcnt', 'memory/memory.max_usage_in_bytes'])
    require(set(result) == {'cgroup_version', *names}, 'Incomplete resource counters')
    for name in names:
        value = result[name]
        if name in ('memory.events', 'memory/memory.oom_control'):
            rows = [line.split() for line in value.splitlines()]
            require(all(len(row) == 2 and row[1].isdigit() for row in rows)
                    and len({row[0] for row in rows}) == len(rows),
                    'Invalid resource event counters')
        else:
            require(value.isdigit(), 'Invalid numeric resource counter')
    result['cgroup_version'] = int(version)
    return result


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def validate_image(config):
    """Never start an image with baked service settings or persistent volumes."""
    require(config.get('User') == '10001:10001', 'Expected the unprivileged Freight API image')
    require(config.get('Entrypoint') == ['stateset-freight'], 'Unexpected API entrypoint')
    require(not config.get('Cmd'), 'Unexpected API command arguments')
    require(not config.get('Volumes'), 'Qualification must not use image volumes')
    permitted_environment = {'PATH', 'FREIGHT_BIND_ADDR', 'LANG', 'LC_ALL', 'TZ',
                             'RUST_LOG', 'HOME', 'SSL_CERT_FILE', 'SSL_CERT_DIR'}
    for item in config.get('Env') or []:
        name, _, value = item.partition('=')
        require(not value or name in permitted_environment,
                'Image embeds service settings; use a clean release image')


def validate_policy(policy):
    require(policy.get('result') == 'pass' and policy.get('socket_denials') == 6
            and policy.get('spawn_denied') is True and policy.get('no_new_privileges') is True
            and policy.get('threads') == 1 and policy.get('filesystem_isolation') is False,
            'Worker syscall policy probe failed or overstates its isolation')
    require(policy.get('readonly_file_opens') is True and type(policy.get('write_open_denials')) is int
            and policy.get('write_open_denials') == 9,
            'Worker file-open restrictions are missing')
    before, after = policy.get('filter_count_before'), policy.get('filter_count_after')
    require(type(before) is int and type(after) is int and before >= 1 and after == before + 1,
            'Expected the worker policy in addition to the container policy')


def validate_renderer(metadata, volume):
    config, limits = metadata['Config'], metadata['HostConfig']
    require(config.get('User') == '10002:10001'
            and config.get('Entrypoint') == ['stateset-freight']
            and config.get('Cmd') == ['--pdf-renderer-service'], 'Unexpected renderer identity or role')
    require(limits.get('NetworkMode') == 'none' and limits.get('PidMode') == ''
            and limits.get('IpcMode') == 'private' and not limits.get('Privileged')
            and limits.get('Memory') == RENDERER_MEMORY_BYTES
            and limits.get('MemorySwap') == RENDERER_MEMORY_BYTES
            and limits.get('NanoCpus') == 1000000000 and limits.get('PidsLimit') == 16
            and limits.get('ReadonlyRootfs') is True and limits.get('CapDrop') == ['ALL']
            and not limits.get('CapAdd') and limits.get('SecurityOpt') == ['no-new-privileges'],
            'Renderer namespace/resource/security restrictions differ')
    mounts = metadata.get('Mounts')
    require(type(mounts) is list and len(mounts) == 1 and mounts[0].get('Type') == 'volume'
            and mounts[0].get('Name') == volume and mounts[0].get('Destination') == '/run/freight-pdf'
            and mounts[0].get('RW') is True, 'Renderer must receive only its owned socket volume')
    permitted = {'PATH', 'HOSTNAME', 'HOME', 'LANG', 'LC_ALL', 'TZ', 'RUST_LOG', 'FREIGHT_BIND_ADDR'}
    for item in config.get('Env') or []:
        name, _, value = item.partition('=')
        require(not value or name in permitted, 'Renderer must not receive API credentials/settings')


class Runtime:
    def __init__(self, directory, image):
        self.directory = Path(directory)
        self.image = image
        self.cid = None
        self.renderer_cid = None
        self.socket_volume = None
        self.owner = str(uuid.uuid4())
        self.cli = self.directory / 'docker-client'
        self.cli.mkdir(mode=0o700)
        docker = shutil.which('docker')
        require(docker, 'Docker CLI is required')
        self.docker_prefix = [docker, '--host', 'unix:///var/run/docker.sock', '--config', str(self.cli)]
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def docker(self, *args):
        return subprocess.check_output(self.docker_prefix + list(args), text=True,
                                       env={'PATH': os.defpath, 'LC_ALL': 'C'}).strip()

    def owned_container(self, cid):
        require(cid, 'No owned container')
        metadata = json.loads(self.docker('inspect', cid))[0]
        require(metadata['Config']['Labels'].get(OWNER_LABEL) == self.owner,
                'Refusing an unowned container')
        return metadata

    def owned(self):
        return self.owned_container(self.cid)

    def renderer(self):
        return self.owned_container(self.renderer_cid)

    def start_renderer(self):
        self.socket_volume = self.docker('volume', 'create', '--label', OWNER_LABEL + '=' + self.owner,
            '--driver', 'local', '--opt', 'type=tmpfs', '--opt', 'device=tmpfs', '--opt',
            'o=size=1048576,uid=10002,gid=10001,mode=0750', 'freight-renderer-' + self.owner)
        self.renderer_cid = self.docker('create', '--pull', 'never',
            '--name', 'freight-renderer-' + self.owner, '--label', OWNER_LABEL + '=' + self.owner,
            '--user', '10002:10001', '--read-only', '--network', 'none', '--ipc', 'private',
            '--tmpfs', '/tmp:rw,nosuid,nodev,noexec,size=16777216,uid=10002,gid=10001,mode=0700',
            '--memory', str(RENDERER_MEMORY_BYTES), '--memory-swap', str(RENDERER_MEMORY_BYTES),
            '--cpus', '1', '--pids-limit', '16', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
            '--mount', 'type=volume,src=' + self.socket_volume + ',dst=/run/freight-pdf',
            '--health-cmd', '/usr/local/bin/stateset-freight --check-pdf-renderer',
            '--health-interval', '1s', '--health-timeout', '3s', '--health-start-period', '3s',
            self.image, '--pdf-renderer-service')
        self.docker('start', self.renderer_cid)
        metadata = self.renderer()
        limits = metadata['HostConfig']
        validate_renderer(metadata, self.socket_volume)
        deadline = time.monotonic() + 30
        while self.renderer()['State'].get('Health', {}).get('Status') != 'healthy':
            require(self.renderer()['State']['Running'], 'Renderer exited during isolated startup')
            require(time.monotonic() < deadline, 'Isolated renderer did not become ready')
            time.sleep(.1)
        lines = self.docker('logs', self.renderer_cid).splitlines()
        policies = [json.loads(line[len('PDF renderer policy '):]) for line in lines
                    if line.startswith('PDF renderer policy ')]
        require(len(policies) == 1, 'Missing renderer kernel policy observation')
        policy = policies[0]
        validate_policy(policy)
        (self.directory / 'runtime-policy.json').write_text(json.dumps(policy, indent=2) + '\n')
        self.renderer_binding = dict(container_id=self.renderer_cid, image=self.image,
            user=metadata['Config']['User'], memory_bytes=limits['Memory'], nano_cpus=limits['NanoCpus'],
            pids=limits['PidsLimit'], readonly_root=True, network='none', cap_drop=limits['CapDrop'],
            security_options=limits['SecurityOpt'], mounts=metadata['Mounts'],
            filesystem_boundary='Dedicated PID/filesystem namespace; no API environment/data mounts; '
                                'nondumpable broker, cleared child environment and inherited descriptors.')
        (self.directory / 'renderer-binding.json').write_text(json.dumps(self.renderer_binding, indent=2) + '\n')
        return policy

    def __enter__(self):
        metadata = json.loads(self.docker('image', 'inspect', self.image))[0]
        validate_image(metadata['Config'])
        self.image = metadata['Id']
        try:
            policy = self.start_renderer()
            keys = {
                VIEWER: dict(tenant_id='demo', subject='fixture:viewer', role='viewer'),
                OPERATOR: dict(tenant_id='demo', subject='fixture:operator', role='operations'),
            }
            args = ['create', '--pull', 'never', '--name', 'freight-preview-' + self.owner,
                    '--label', OWNER_LABEL + '=' + self.owner, '--read-only',
                    '--tmpfs', '/tmp:rw,nosuid,noexec,size=16777216', '--memory', str(MEMORY_BYTES),
                    '--memory-swap', str(MEMORY_BYTES), '--cpus', '2', '--pids-limit', '64',
                    '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
                    '--mount', 'type=volume,src=' + self.socket_volume + ',dst=/run/freight-pdf,readonly',
                    '--publish', '127.0.0.1::8090']
            environment = dict(FREIGHT_DEMO='true', FREIGHT_BIND_ADDR='0.0.0.0:8090',
                               FREIGHT_API_KEYS_JSON=json.dumps(keys), FREIGHT_INTEGRATIONS_JSON='{}',
                               TOKIO_WORKER_THREADS='2', RUST_LOG='warn',
                               FREIGHT_PDF_RENDERER_SOCKET='/run/freight-pdf/renderer.sock',
                               FREIGHT_PDF_RENDERER_UID='10002')
            for name, value in environment.items():
                args += ['--env', name + '=' + value]
            self.cid = self.docker(*args, self.image)
            self.docker('start', self.cid)
            metadata = self.owned()
            port = int(metadata['NetworkSettings']['Ports']['8090/tcp'][0]['HostPort'])
            self.origin = 'http://127.0.0.1:' + str(port)
            limits = metadata['HostConfig']
            require(limits['Memory'] == MEMORY_BYTES and limits['NanoCpus'] == 2000000000
                    and limits['PidsLimit'] == 64 and limits['ReadonlyRootfs']
                    and limits['CapDrop'] == ['ALL'], 'Container resource restrictions differ')
            self.binding = dict(image=self.image, container_id=self.cid,
                                owner_label=OWNER_LABEL, owner_id=self.owner,
                                user=metadata['Config']['User'], port=port,
                                memory_bytes=limits['Memory'], nano_cpus=limits['NanoCpus'],
                                pids=limits['PidsLimit'], readonly_root=limits['ReadonlyRootfs'],
                                cap_drop=limits['CapDrop'], security_options=limits['SecurityOpt'],
                                renderer_policy=policy, renderer=self.renderer_binding)
            (self.directory / 'runtime-binding.json').write_text(json.dumps(self.binding, indent=2) + '\n')
            deadline = time.monotonic() + 30
            while True:
                try:
                    status, _, body, _ = self.request('/v1/loads?limit=50')
                    if status == 200:
                        self.load = json.loads(body)['loads'][0]['id']
                        return self
                except OSError:
                    pass
                require(self.owned()['State']['Running'], 'API exited during startup')
                require(time.monotonic() < deadline, 'API did not become ready')
                time.sleep(.1)
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def request(self, path, credential=VIEWER, body=None, media=None, key=None):
        headers = {}
        if credential:
            headers['Authorization'] = 'Bearer ' + credential
        if media:
            headers['Content-Type'] = media
        if key:
            headers['Idempotency-Key'] = key
        started = time.monotonic()
        try:
            response = self.opener.open(urllib.request.Request(
                self.origin + path, data=body, headers=headers), timeout=15)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            raw = response.read(RESPONSE_BYTES + 1)
            require(len(raw) <= RESPONSE_BYTES, 'API response exceeds qualification byte limit')
            return response.code, dict(response.headers), raw, (time.monotonic() - started) * 1000

    def upload(self, name, body, key):
        status, _, raw, _ = self.request('/v1/loads/' + self.load + '/documents?kind=pod&filename=' + name,
                                         OPERATOR, body, 'application/pdf', key)
        require(status in [200, 201], 'Synthetic fixture upload failed: ' + str(status))
        return json.loads(raw)['data']['id']

    def renderers(self):
        self.owned()
        self.renderer()
        rows = self.docker('top', self.renderer_cid, '-eo', 'pid,ppid,comm').splitlines()[1:]
        result = []
        for row in rows:
            fields = row.split()
            if len(fields) != 3 or fields[2] != 'pdftoppm':
                continue
            host = int(fields[0])
            status = Path('/proc/%d/status' % host).read_text()
            ns = next(line for line in status.splitlines() if line.startswith('NSpid:')).split()
            pid = int(ns[-1])
            require(pid > 1, 'Invalid owned renderer PID')
            fields = dict(line.split(':', 1) for line in status.splitlines() if ':' in line)
            policy = {name: int(fields[name].strip()) for name in
                      ['NoNewPrivs', 'Seccomp', 'Seccomp_filters', 'Threads']}
            require(policy['NoNewPrivs'] == 1 and policy['Seccomp'] == 2
                    and policy['Seccomp_filters'] == self.binding['renderer_policy']['filter_count_after']
                    and policy['Threads'] == 1, 'Actual PDF renderer did not inherit its worker policy')
            result.append(dict(host=host, namespace=pid, policy=policy))
        return result

    def wait_renderer(self):
        deadline = time.monotonic() + 4
        while True:
            processes = self.renderers()
            if processes:
                require(len(processes) == 1, 'Overlapping PDF renderers')
                return processes[0]
            require(time.monotonic() < deadline, 'Renderer did not start')
            time.sleep(.02)

    def signal(self, renderer, signal):
        require(signal in ['STOP', 'KILL'], 'Unsupported qualification signal')
        require(renderer in self.renderers(), 'Renderer no longer belongs to the owned container')
        self.docker('exec', '--user', '10002:10001', self.renderer_cid, '/bin/sh', '-c',
                    'kill -s "$1" "$2"', 'owned-preview-fault', signal, str(renderer['namespace']))

    def resources(self, renderer=False):
        cid = self.renderer_cid if renderer else self.cid
        self.owned_container(cid)
        return parse_resources(self.docker('exec', cid, '/bin/sh', '-c', RESOURCE_PROBE))

    @staticmethod
    def validate_resources(result, expected_memory=MEMORY_BYTES):
        version = str(result['cgroup_version'])
        limit_key = 'memory.max' if version == '2' else 'memory/memory.limit_in_bytes'
        require(int(result[limit_key]) == expected_memory, 'Actual memory limit differs')
        if version == '2':
            events = dict((key, int(value)) for key, value in
                          (line.split() for line in result['memory.events'].splitlines()))
            require('oom_kill' in events and events['oom_kill'] == 0, 'A process was killed by OOM')
        else:
            events = dict((key, int(value)) for key, value in
                          (line.split() for line in result['memory/memory.oom_control'].splitlines()))
            require('oom_kill' in events and events['oom_kill'] == 0, 'A process was killed by OOM')
            require(int(result['memory/memory.max_usage_in_bytes']) <= expected_memory,
                    'Observed memory peak exceeds its limit')

    def __exit__(self, *_):
        errors = []
        for attribute, role, memory, wait in [('cid', 'api', MEMORY_BYTES, '5'),
                                            ('renderer_cid', 'renderer', RENDERER_MEMORY_BYTES, '15')]:
            cid = getattr(self, attribute, None)
            if cid is None:
                continue
            role_errors, state, gone = [], None, False
            try:
                data = self.resources(renderer=role == 'renderer')
                name = 'resource-final.json' if role == 'api' else 'renderer-resource-final.json'
                (self.directory / name).write_text(json.dumps(data, indent=2) + '\n')
                self.validate_resources(data, memory)
            except Exception as error:
                role_errors.append('resources: ' + str(error))
            try:
                self.owned_container(cid)
                with (self.directory / (role + '.log')).open('xb') as stream:
                    subprocess.run(self.docker_prefix + ['logs', cid], stdout=stream,
                                   stderr=subprocess.STDOUT, check=True)
            except Exception as error:
                role_errors.append('logs: ' + str(error))
            try:
                # Never act on an unowned ID. Failure of one cleanup must not
                # prevent attempts to clean the other container and volume.
                self.owned_container(cid)
                self.docker('stop', '--time', wait, cid)
                state = self.owned_container(cid)['State']
                self.docker('rm', cid)
                gone = subprocess.run(self.docker_prefix + ['inspect', cid],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0
                require(gone, 'Owned container remains')
                require(not state['OOMKilled'], 'Qualification container was killed by OOM')
            except Exception as error:
                role_errors.append('cleanup: ' + str(error))
            if gone:
                setattr(self, attribute, None)
            errors += [role + ': ' + error for error in role_errors]
            try:
                name = 'cleanup.json' if role == 'api' else 'renderer-cleanup.json'
                (self.directory / name).write_text(json.dumps(dict(
                    container_removed=gone, state=state, diagnostic_errors=role_errors), indent=2) + '\n')
            except Exception as error:
                errors.append(role + ' cleanup report: ' + str(error))
        if self.socket_volume is not None:
            try:
                metadata = json.loads(self.docker('volume', 'inspect', self.socket_volume))[0]
                require(metadata['Labels'].get(OWNER_LABEL) == self.owner, 'Refuse an unowned socket volume')
                self.docker('volume', 'rm', self.socket_volume)
                self.socket_volume = None
            except Exception as error:
                errors.append('socket volume cleanup: ' + str(error))
        require(not errors, 'Resource diagnostics/cleanup failed: ' + '; '.join(errors))
