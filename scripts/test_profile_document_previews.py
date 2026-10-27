"""Negative controls for document acceptance in managed-role/TLS deployments."""
import copy
import hashlib
import unittest

from profile_document_previews import verify_compose, verify_pixels


def compose():
    return dict(services=dict(
        api=dict(image='sha256:owned-image',
            environment=dict(FREIGHT_PDF_RENDERER_SOCKET='/run/freight-pdf/renderer.sock',
                             FREIGHT_PDF_RENDERER_UID='10002'),
            volumes=[dict(type='volume', source='socket', target='/run/freight-pdf', read_only=True)],
            depends_on=dict(renderer=dict(condition='service_healthy'))),
        renderer=dict(image='sha256:owned-image', command=['--pdf-renderer-service'],
            user='10002:10001', read_only=True, network_mode='none', ipc='private',
            cap_drop=['ALL'], security_opt=['no-new-privileges:true'], mem_limit=402653184,
            memswap_limit=402653184, cpus=1, pids_limit=16,
            volumes=[dict(type='volume', source='socket', target='/run/freight-pdf')]),
        migrate=dict(environment={}), grants=dict(environment={})),
        volumes=dict(socket=dict(driver='local', driver_opts=dict(type='tmpfs', device='tmpfs',
            o='size=1048576,uid=10002,gid=10001,mode=0750'))))


class ProfileDocumentPreviews(unittest.TestCase):
    def test_shared_namespaces_privilege_expansion_credentials_and_budget_drift_are_rejected(self):
        base = compose()
        verify_compose(base)
        strings = copy.deepcopy(base)
        strings['services']['renderer'].update(mem_limit='402653184', memswap_limit='402653184')
        verify_compose(strings)
        for key, value in [('image', 'sha256:other'), ('command', ['--serve']),
                ('user', '10001:10001'), ('read_only', False), ('network_mode', 'host'),
                ('ipc', 'host'), ('pid', 'host'), ('privileged', True), ('cap_drop', []),
                ('cap_add', ['SYS_ADMIN']), ('security_opt', ['seccomp=unconfined']),
                ('environment', {'PGPASSWORD': 'must-not-forward'}), ('env_file', ['.env']),
                ('ports', ['8090:8090']), ('devices', ['/dev/sda']),
                ('mem_limit', 805306368), ('memswap_limit', -1), ('cpus', 2), ('pids_limit', 0)]:
            value_config = copy.deepcopy(base)
            value_config['services']['renderer'][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                verify_compose(value_config)

    def test_tls_material_mounts_persistent_socket_storage_and_writable_api_socket_are_rejected(self):
        base = compose()
        # The API may receive the public database CA; the renderer may not.
        ca = dict(type='bind', source='/owned/ca.crt', target='/run/freight-database-ca.crt', read_only=True)
        base['services']['api']['volumes'].append(ca)
        verify_compose(base)
        for mutate in [
            lambda c: c['services']['renderer']['volumes'].append(ca),
            lambda c: c['services']['api']['volumes'][0].update(read_only=False),
            lambda c: c['services']['api']['volumes'][0].update(source='other'),
            lambda c: c['volumes']['socket']['driver_opts'].update(type='ext4'),
            lambda c: c['volumes']['socket']['driver_opts'].update(o='size=1048576,mode=0777'),
            lambda c: c['services']['migrate']['environment'].update(FREIGHT_PDF_RENDERER_UID='10002'),
            lambda c: c['services']['api']['environment'].pop('FREIGHT_PDF_RENDERER_SOCKET'),
            lambda c: c['services']['api']['depends_on']['renderer'].update(condition='service_started')]:
            value = copy.deepcopy(base);mutate(value)
            with self.assertRaises(AssertionError):
                verify_compose(value)

    def test_forged_integrity_hash_does_not_replace_independent_pixel_expectation(self):
        pixels = bytes([255, 0, 0]) * 4
        source = b'owned-original'
        headers = {'content-type': 'application/vnd.stateset.rgb8', 'cache-control': 'private, no-store',
            'x-document-id': 'doc', 'x-load-id': 'load', 'x-source-sha256': hashlib.sha256(source).hexdigest(),
            'x-content-sha256': hashlib.sha256(pixels).hexdigest(), 'x-preview-page': '1',
            'x-preview-pages': '2', 'x-preview-width': '2', 'x-preview-height': '2'}
        verify_pixels(headers, pixels, 'doc', 'load', source, 1, 2, 2, [255, 0, 0])
        wrong = bytes([0]) + pixels[1:]
        forged = dict(headers, **{'x-content-sha256': hashlib.sha256(wrong).hexdigest()})
        with self.assertRaisesRegex(AssertionError, 'independent fixture'):
            verify_pixels(forged, wrong, 'doc', 'load', source, 1, 2, 2, [255, 0, 0])
        for key, value in [('x-document-id', 'other'), ('x-source-sha256', '0' * 64),
                           ('x-preview-page', '2'), ('x-preview-width', '1')]:
            with self.subTest(key=key), self.assertRaises(AssertionError):
                verify_pixels(dict(headers, **{key: value}), pixels, 'doc', 'load', source,
                              1, 2, 2, [255, 0, 0])


if __name__ == '__main__':
    unittest.main()
