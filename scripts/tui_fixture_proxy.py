"""Loopback-only relay for deterministic slow-read API/TUI process checks."""
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
import time
from urllib.parse import parse_qs, urlsplit


class DelayedReads:
    def __init__(self, origin, delay_ms):
        upstream = urlsplit(origin)
        if upstream.scheme != 'http' or upstream.hostname != '127.0.0.1':
            raise ValueError('The owned delay fixture requires an HTTP loopback API')
        if not 0 < delay_ms <= 5000:
            raise ValueError('Read delay must be between 1 and 5000 milliseconds')
        self.delayed = 0
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def forward(self):
                path = urlsplit(self.path)
                query = parse_qs(path.query)
                if self.command == 'GET' and path.path == '/v1/operations' and query.get('search'):
                    time.sleep(delay_ms / 1000)
                    fixture.delayed += 1
                length = int(self.headers.get('Content-Length', '0'))
                if not 0 <= length <= 1024 * 1024:
                    self.send_error(413)
                    return
                body = self.rfile.read(length) if length else None
                headers = {k: v for k, v in self.headers.items()
                           if k.lower() not in ('host', 'connection', 'content-length')}
                connection = http.client.HTTPConnection(upstream.hostname, upstream.port, timeout=25)
                try:
                    connection.request(self.command, self.path, body=body, headers=headers)
                    response = connection.getresponse()
                    payload = response.read()
                    self.send_response(response.status)
                    for key, value in response.getheaders():
                        if key.lower() in ('content-type', 'cache-control'):
                            self.send_header(key, value)
                    self.send_header('Content-Length', str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                finally:
                    connection.close()

            do_GET = do_POST = forward

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.origin = 'http://127.0.0.1:%d' % self.server.server_port

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
