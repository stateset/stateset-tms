"""Slow-read fixtures must preserve the real API's exact command contract."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import http.client
import threading
import time
import unittest
from tui_fixture_proxy import DelayedReads


class RelayTests(unittest.TestCase):
    def test_delayed_read_and_refused_write_preserve_path_headers_body_and_status(self):
        seen = []

        class API(BaseHTTPRequestHandler):
            def log_message(self, *args): pass
            def do_GET(self):
                seen.append(self.path)
                self.send_response(200); self.end_headers(); self.wfile.write(b'{"issues":[]}')
            def do_POST(self):
                seen.append((self.path, self.headers['Idempotency-Key'], self.headers['Authorization'],
                             self.rfile.read(int(self.headers['Content-Length']))))
                self.send_response(409); self.send_header('Content-Type', 'application/json'); self.end_headers()
                self.wfile.write(b'{"error":{"code":"request_stopped"}}')

        upstream = ThreadingHTTPServer(('127.0.0.1', 0), API)
        thread = threading.Thread(target=upstream.serve_forever, daemon=True); thread.start()
        relay = DelayedReads('http://127.0.0.1:%d' % upstream.server_port, 50)
        try:
            client = http.client.HTTPConnection('127.0.0.1', relay.server.server_port, timeout=5)
            start = time.monotonic()
            client.request('GET', '/v1/operations?search=owned%20case')
            response = client.getresponse(); self.assertEqual(response.status, 200); response.read()
            self.assertGreaterEqual(time.monotonic() - start, .05)
            self.assertEqual(relay.delayed, 1)
            body = b'{ "expected_version": 2, "note": "exact original" }'
            client.request('POST', '/v1/operations/issues/owned/actions', body,
                           {'Idempotency-Key': 'owned-original', 'Authorization': 'Bearer synthetic'})
            response = client.getresponse()
            self.assertEqual(response.status, 409)
            self.assertEqual(response.read(), b'{"error":{"code":"request_stopped"}}')
            self.assertEqual(seen, ['/v1/operations?search=owned%20case',
                                   ('/v1/operations/issues/owned/actions', 'owned-original', 'Bearer synthetic', body)])
            client.close()
        finally:
            relay.close(); upstream.shutdown(); upstream.server_close(); thread.join(timeout=5)

    def test_only_owned_loopback_and_bounded_positive_delay(self):
        for origin, delay in [('https://127.0.0.1:1', 50), ('http://example.invalid', 50),
                              ('http://127.0.0.1:1', 0), ('http://127.0.0.1:1', 5001)]:
            with self.assertRaises(ValueError): DelayedReads(origin, delay)


if __name__ == '__main__': unittest.main()
