"""Document/renderer acceptance shared by durable SCRAM and TLS Compose profiles."""
import copy
import hashlib
import importlib.util
import json
import time
import urllib.error
import urllib.request

from document_preview_runtime import validate_policy, validate_renderer
from runtime_contracts import ROOT, NoRedirect, require

INPUTS = ['scripts/profile_document_previews.py', 'scripts/document_preview_runtime.py',
          'scripts/check-tui-graphics.py', 'scripts/tui_terminal.py',
          'scripts/runtime_contracts.py', 'scripts/native_postgres.py',
          'tests/fixtures/documents/preview-two-pages.pdf']
SOCKET = '/run/freight-pdf'


def verify_compose(config):
    api, renderer = (config['services'][name] for name in ['api', 'renderer'])
    require(renderer.get('image') == api.get('image') and api.get('image'),
            'Renderer must use the exact API image')
    require(renderer.get('command') == ['--pdf-renderer-service']
            and renderer.get('user') == '10002:10001'
            and renderer.get('read_only') is True
            and renderer.get('network_mode') == 'none'
            and renderer.get('ipc') == 'private' and not renderer.get('pid')
            and not renderer.get('privileged') and not renderer.get('cap_add')
            and renderer.get('cap_drop') == ['ALL']
            and renderer.get('security_opt') == ['no-new-privileges:true']
            and not renderer.get('ports') and not renderer.get('devices')
            and not renderer.get('environment') and not renderer.get('env_file'),
            'Renderer isolation or credential boundary differs')
    require(int(renderer.get('mem_limit', 0)) == int(renderer.get('memswap_limit', 0)) == 402653184
            and float(renderer.get('cpus', 0)) == 1 and renderer.get('pids_limit') == 16,
            'Renderer resource budget differs')
    mounts = renderer.get('volumes', [])
    require(len(mounts) == 1 and mounts[0].get('type') == 'volume'
            and mounts[0].get('target') == SOCKET and not mounts[0].get('read_only'),
            'Renderer must receive only its socket volume')
    volume = mounts[0]['source']
    definition = config['volumes'][volume]
    require(definition.get('driver') == 'local'
            and definition.get('driver_opts') == dict(type='tmpfs', device='tmpfs',
                o='size=1048576,uid=10002,gid=10001,mode=0750'),
            'Renderer socket storage must be private owned tmpfs')
    api_mounts = [m for m in api.get('volumes', []) if m.get('target') == SOCKET]
    require(len(api_mounts) == 1 and api_mounts[0].get('type') == 'volume'
            and api_mounts[0].get('source') == volume and api_mounts[0].get('read_only') is True,
            'API requires the same read-only renderer socket mount')
    environment = api['environment']
    require(environment.get('FREIGHT_PDF_RENDERER_SOCKET') == SOCKET + '/renderer.sock'
            and str(environment.get('FREIGHT_PDF_RENDERER_UID')) == '10002'
            and api['depends_on']['renderer']['condition'] == 'service_healthy',
            'API must select and wait for the isolated renderer')
    for name in ['migrate', 'grants']:
        require(not any(key.startswith('FREIGHT_PDF_RENDERER_')
                        for key in config['services'][name].get('environment', {})),
                'Schema/role jobs must not receive renderer configuration')


def verify_pixels(headers, pixels, document, load, source, page, width, height, color):
    fields = {key.lower(): value for key, value in headers.items()}
    require(fields.get('content-type') == 'application/vnd.stateset.rgb8'
            and fields.get('cache-control') == 'private, no-store'
            and fields.get('x-document-id') == document and fields.get('x-load-id') == load
            and fields.get('x-source-sha256') == hashlib.sha256(source).hexdigest()
            and fields.get('x-content-sha256') == hashlib.sha256(pixels).hexdigest()
            and fields.get('x-preview-page') == str(page) and fields.get('x-preview-pages') == '2'
            and fields.get('x-preview-width') == str(width)
            and fields.get('x-preview-height') == str(height), 'Preview identity/integrity differs')
    require(pixels == bytes(color) * (width * height), 'Preview pixels differ from independent fixture')


def request(owner, path, *, body=None, key=None, credential=True, expected=200):
    headers = {}
    if credential:
        headers['Authorization'] = 'Bearer ' + (owner.token if credential is True else credential)
    if body is not None:
        headers['Content-Type'] = 'application/pdf'
    if key:
        headers['Idempotency-Key'] = key
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    try:
        response = client.open(urllib.request.Request(
            'http://127.0.0.1:' + str(owner.ports['api']) + path, data=body, headers=headers), timeout=15)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read(12 * 1024 * 1024 + 1)
        require(len(raw) <= 12 * 1024 * 1024, 'Profile response exceeds its byte bound')
        require(response.status in (expected if isinstance(expected, tuple) else (expected,)),
                'Unexpected document HTTP status ' + str(response.status))
        return dict(response.headers), raw


def verify_actual_renderer(owner):
    metadata = owner.service('renderer')
    mounts = metadata.get('Mounts', [])
    require(len(mounts) == 1, 'Renderer received additional mounted data')
    # Compose serializes the same NNP option with an explicit true value.
    normalized = copy.deepcopy(metadata)
    if normalized['HostConfig'].get('SecurityOpt') == ['no-new-privileges:true']:
        normalized['HostConfig']['SecurityOpt'] = ['no-new-privileges']
    validate_renderer(normalized, mounts[0]['Name'])
    require(metadata['Image'] == owner.images['freight'], 'Renderer image drift')
    owner.report['renderer'] = dict(image=metadata['Image'], container=metadata['Id'],
        user=metadata['Config']['User'], socket_volume=mounts[0]['Name'],
        network='none', private_ipc=True, readonly_root=True, no_service_environment=True,
        memory_bytes=metadata['HostConfig']['Memory'], nano_cpus=metadata['HostConfig']['NanoCpus'],
        pids=metadata['HostConfig']['PidsLimit'])
    owner.report.setdefault('renderer_generations', []).append(copy.deepcopy(owner.report['renderer']))
    return metadata


def exercise(owner):
    renderer = verify_actual_renderer(owner)
    stdout = owner.run(['docker', '--host', 'unix:///var/run/docker.sock',
        'logs', renderer['Id']], label='renderer startup policy')[0]
    policies = [json.loads(line[len('PDF renderer policy '):]) for line in stdout.splitlines()
                if line.startswith('PDF renderer policy ')]
    require(len(policies) == 1, 'Missing actual renderer startup policy')
    validate_policy(policies[0]);owner.report['renderer_worker_policy'] = policies[0]
    owner.check('actual profile renderer uses isolated credential-free image and enforced worker policy')
    original = (ROOT / 'tests/fixtures/documents/preview-two-pages.pdf').read_bytes()
    entry = owner.http('/v1/loads?limit=1')['loads'][0]
    load = entry['id']
    _, body = request(owner, '/v1/loads/' + load + '/documents?kind=pod&filename=profile.pdf',
        body=original, key='owned-profile-document', expected=(200, 201))
    document = json.loads(body)['data']['id']
    path = '/v1/documents/' + document + '/preview'
    owner.preview_fixture = dict(original=original, load=load, document=document, path=path,
                                 reference=entry['reference'])
    before = owner.http('/v1/loads/' + load + '/documents')
    for page, width, height, color in [(1, 720, 960, [255, 0, 0]), (2, 960, 720, [0, 0, 255])]:
        headers, pixels = request(owner, path + '?page=' + str(page))
        verify_pixels(headers, pixels, document, load, original, page, width, height, color)
    request(owner, path + '?page=3', expected=422)
    for credential, status in [(False, 401), (owner.other_token, 404)]:
        request(owner, path, credential=credential, expected=status)
        request(owner, '/v1/documents/' + document + '/content', credential=credential, expected=status)
    require(request(owner, '/v1/documents/' + document + '/content')[1] == original,
            'Profile original changed')
    require(owner.http('/v1/loads/' + load + '/documents') == before, 'Preview changed review metadata')
    owner.check('durable profile PDF pages match every pixel and retain original/review/tenant boundaries',
                original_sha256=hashlib.sha256(original).hexdigest(), document=document)
    api = owner.service('api')['Id']
    owner.compose_run('stop', 'renderer')
    require(not owner.service('renderer')['State']['Running'], 'Renderer outage was not induced')
    request(owner, path, expected=503)
    require(request(owner, '/v1/documents/' + document + '/content')[1] == original,
            'Renderer outage made original unavailable')
    owner.http('/ready')
    owner.compose_run('up', '--detach', '--no-deps', '--no-build', '--pull', 'never', 'renderer')
    deadline = time.monotonic() + 40
    while owner.service('renderer')['State'].get('Health', {}).get('Status') != 'healthy':
        require(time.monotonic() < deadline, 'Profile renderer recovery timed out');time.sleep(.2)
    verify_actual_renderer(owner)
    require(owner.service('api')['Id'] == api, 'Renderer recovery recreated the API')
    retained(owner, record=False)
    owner.check('profile renderer outage fails closed while originals/readiness survive and restart recovers without API recreation')


def retained(owner, record=True):
    fixture = owner.preview_fixture
    headers, pixels = request(owner, fixture['path'])
    verify_pixels(headers, pixels, fixture['document'], fixture['load'], fixture['original'],
                  1, 720, 960, [255, 0, 0])
    require(request(owner, '/v1/documents/' + fixture['document'] + '/content')[1] == fixture['original'],
            'Credential rotation/restart changed PDF original')
    if record:
        owner.check('rotated runtime credential and recreated API retain exact PDF original and source-bound preview')


def exercise_client(owner, phase):
    if owner.client_binary is None:
        return
    expected = owner.report['client_graphics']['sha256']
    require(hashlib.sha256(owner.client_binary.read_bytes()).hexdigest() == expected, 'TUI executable drift')
    spec = importlib.util.spec_from_file_location('profile_tui_graphics', ROOT / 'scripts/check-tui-graphics.py')
    graphics = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(graphics)
    modes = ['auto', 'negative', 'off', 'tmux-graphics', 'tmux-graphics-mono']
    live = dict(origin='http://127.0.0.1:' + str(owner.ports['api']), token=owner.token,
                reference=owner.preview_fixture['reference'], pdf=True)
    for mode in modes:
        graphics.exercise(owner.client_binary, mode, live)
    require(hashlib.sha256(owner.client_binary.read_bytes()).hexdigest() == expected, 'TUI executable drift')
    owner.report['client_graphics']['phases'].append(dict(phase=phase, modes=modes,
        real_terminal_window=False, scope='Actual profile API plus TUI on owned PTYs; no rendered-terminal/operator acceptance claim.'))
    owner.check('actual profile API/TUI PDF graphics pass all five modes ' + phase)
