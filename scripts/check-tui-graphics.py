#!/usr/bin/env python3
"""Qualify graphics bytes/lifecycle on owned PTYs; does not emulate rendered pixels."""
import argparse
import base64
from contextlib import contextmanager
import json
import socket
import tempfile
import urllib.request
import fcntl
import os
from pathlib import Path
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import time
import zlib
from tui_terminal import Screen


def require(value, message):
    if not value:
        raise AssertionError(message)


def exercise(binary, mode, live=None):
    master, slave = os.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 38, 110, 880, 608))
    env = dict(os.environ, TERM='xterm-ghostty', TERM_PROGRAM='ghostty')
    for name in ['TMUX', 'ZELLIJ', 'STY', 'NO_COLOR']:
        env.pop(name, None)
    args = [str(binary), '--demo'] if live is None else [str(binary), '--api', live['origin']]
    if live is not None:
        env['FREIGHT_TUI_TOKEN'] = live['token']
        env.pop('FREIGHT_TUI_ORGANIZATION_ID', None)
    if mode in ['off', 'mono']:
        args += ['--graphics', 'off']
    if mode == 'mono':
        args += ['--no-color']
    if mode in ['unknown', 'unknown-supported']:
        env.update(TERM='xterm-256color', TERM_PROGRAM='owned-unknown')
    if mode == 'force':
        args += ['--graphics', 'kitty']
    if mode == 'multiplexer':
        env['TMUX'] = 'owned-pty-multiplexer-path'
    if mode == 'screen':
        env['STY'] = 'owned-pty-screen-path'
    if mode in ['tmux-term', 'screen-term']:
        env['TERM'] = 'tmux-256color' if mode == 'tmux-term' else 'screen-256color'
    if mode == 'dumb':
        env['TERM'] = 'dumb'
    tmux_graphics = mode.startswith('tmux-graphics')
    if tmux_graphics:
        args += ['--graphics', 'kitty-tmux']
        env['TMUX'] = 'owned-pty-tmux-path'
    if mode == 'tmux-graphics-mono':
        args += ['--no-color']
    if mode == 'tmux-graphics-env-mono':
        env['NO_COLOR'] = '1'
    monochrome = mode in ['mono', 'tmux-graphics-mono', 'tmux-graphics-env-mono']

    def protocol_bytes():
        wire = bytes(output)
        if not tmux_graphics:
            return wire
        pattern = rb'\x1bPtmux;\x1b\x1b7\x1b\x1b_G([^\x1b]*)\x1b\x1b\\\x1b\x1b8\x1b\\'
        matches = re.findall(pattern, wire)
        require(wire.count(b'\x1bPtmux;') == len(matches), 'incomplete or invalid tmux wrapper')
        require(b'\x1b_G' not in re.sub(pattern, b'', wire), 'unwrapped graphics escaped tmux transport')
        return re.sub(pattern, lambda m: b'\x1b_G' + m.group(1) + b'\x1b\\', wire)
    output = bytearray()
    query_seen = False
    query_id = None
    reply_sent = False
    query_offset = 0

    def attach():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    proc = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave, env=env, preexec_fn=attach)
    try:
        def drain(seconds=0.3):
            nonlocal query_seen, query_id, reply_sent, query_offset
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    chunk = os.read(master, 65536)
                    output.extend(chunk)
                    if b'\x1b[6n' in chunk:
                        os.write(master, b'\x1b[1;1R')
                    match = re.search(rb'\x1b_Ga=q,t=d,f=24,o=z,s=1,v=1,i=(\d+);([^\x1b]+)\x1b\\', output)
                    if match and not query_seen:
                        query_seen = True
                        query_id = match.group(1)
                        query_offset = len(output)
                        require(zlib.decompress(base64.b64decode(match.group(2), validate=True)) == b'\0\0\0', 'query did not test exact compressed RGB')
                        response = None
                        if mode in ['auto', 'signal', 'unknown-supported', 'fragmented', 'early-key']:
                            response = b'\x1b_Gi=' + query_id + b';OK\x1b\\'
                        elif mode == 'negative':
                            response = b'\x1b_Gi=' + query_id + b';ENOTSUP:r q a c\x1b\\'
                        elif mode == 'wrong-id':
                            response = b'\x1b_Gi=' + str(int(query_id) % 4294967295 + 1).encode() + b';OK\x1b\\'
                        elif mode == 'oversized':
                            response = b'\x1b_Gi=' + query_id + b';' + b'q'*2000 + b'\x1b\\'
                        if mode == 'early-key':
                            # A normal input before the reply must still reach the UI.
                            os.write(master, b'v')
                        if response:
                            if mode == 'fragmented':
                                for byte in response:
                                    os.write(master, bytes([byte]))
                                    time.sleep(.001)
                            else:
                                os.write(master, response)
                            reply_sent = True
                    if mode == 'late' and query_seen and not reply_sent:
                        # Wait beyond the fixed deadline; the late reply stays input-filtered.
                        time.sleep(.9)
                        os.write(master, b'\x1b_Gi=' + query_id + b';OK\x1b\\')
                        reply_sent = True

        def until(needle, start=0):
            deadline = time.monotonic() + 10
            while needle not in output[start:]:
                require(time.monotonic() < deadline, 'missing output: ' + repr(needle))
                require(proc.poll() is None, 'console exited early')
                drain(0.1)

        until(b'FRT-10421' if live is None else live['reference'].encode())
        if mode != 'early-key':
            os.write(master, b'v')
        kitty = mode in ['auto', 'signal', 'unknown-supported', 'fragmented', 'early-key', 'force', 'tmux-graphics', 'tmux-graphics-signal']
        if kitty:
            until(b'a=p,')
            commands = re.findall(rb'\x1b_G([^;]*);(.*?)\x1b\\', protocol_bytes(), re.S)
            payload = bytearray()
            image_id = None
            dimensions = None
            for header, chunk in commands:
                fields = dict(item.split(b'=', 1) for item in header.split(b','))
                if fields.get(b'a') == b't':
                    image_id = fields[b'i']
                    dimensions = (int(fields[b's']), int(fields[b'v']))
                    require(fields[b'f'] == b'24' and fields[b't'] == b'd' and fields[b'o'] == b'z', 'wrong transfer format')
                if fields.get(b'a') in [b't', None] and b'm' in fields:
                    require(len(chunk) <= 4096 and len(chunk) % 4 == 0, 'chunk bound')
                    payload.extend(base64.b64decode(chunk, validate=True))
            rgb = zlib.decompress(payload)
            expected = (720, 960) if live and live.get('pdf') else (360, 480)
            require(dimensions == expected and len(rgb) == expected[0]*expected[1]*3, 'lossless synthetic pixel dimensions')
            if live and live.get('pdf'):
                require(rgb == bytes([255, 0, 0]) * (720*960), 'first PDF page pixel content differs')
                def page_preview(key, expected_dimensions, expected_color):
                    start = len(output)
                    os.write(master, key)
                    until(b'a=p,', start)
                    items = re.findall(rb'\x1b_G([^;]*);(.*?)\x1b\\', protocol_bytes(), re.S)
                    transfers = []
                    for header, chunk in items:
                        fields = dict(item.split(b'=', 1) for item in header.split(b','))
                        if fields.get(b'a') == b't':
                            transfers.append([int(fields[b's']), int(fields[b'v']), bytearray()])
                        if fields.get(b'a') in [b't', None] and b'm' in fields:
                            transfers[-1][2].extend(base64.b64decode(chunk, validate=True))
                    require(tuple(transfers[-1][:2]) == expected_dimensions, 'PDF page dimensions differ')
                    require(zlib.decompress(transfers[-1][2]) == bytes(expected_color) * (expected_dimensions[0]*expected_dimensions[1]), 'PDF page RGB differs')
                page_preview(b'\x1b[6~', (960, 720), (0, 0, 255))
                page_preview(b'\x1b[5~', (720, 960), (255, 0, 0))
            require(b'\x1b_Ga=d;' not in output, 'unscoped image deletion')
            first_transmits = bytes(output).count(b'a=t,')
            start = len(output)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 90, 720, 480))
            os.kill(proc.pid, signal.SIGWINCH)
            until(b'a=p,', start)
            drain()
            require(bytes(output).count(b'a=t,') == first_transmits + 1, 'resize did not reconstruct image after terminal clear')
            if mode in ['signal', 'tmux-graphics-signal']:
                os.kill(proc.pid, signal.SIGTERM)
            else:
                start = len(output)
                os.write(master, b'g')
                until(b'd=I,i=' + image_id, start)
                os.write(master, b'\x1b')
                drain()
                os.write(master, b'q')
        else:
            if not monochrome:
                until('▀'.encode())
            drain()
            if live and live.get('pdf'):
                def current_page(page):
                    deadline = time.monotonic() + 10
                    while True:
                        screen = Screen(38, 110)
                        screen.feed(re.sub(rb'\x1b_G.*?\x1b\\', b'', protocol_bytes(), flags=re.S))
                        if ('Page %d / 2' % page) in screen.text and 'Preview only' in screen.text:
                            return
                        require(time.monotonic() < deadline, 'PDF fallback did not finish the requested page')
                        require(proc.poll() is None, 'PDF fallback exited during navigation')
                        drain(.1)
                current_page(1)
                os.write(master, b'\x1b[6~')
                current_page(2)
                os.write(master, b'\x1b[5~')
                current_page(1)
            commands = re.findall(rb'\x1b_G([^;]*);(.*?)\x1b\\', protocol_bytes(), re.S)
            require(all(b'a=q,' in header for header, _ in commands), 'fallback emitted stored/placed image commands')
            require(proc.poll() is None, 'protocol reply payload executed quit')
            if monochrome:
                require('▀'.encode() not in output, 'monochrome mode emitted colored preview')
            os.write(master, b'\x1b')
            drain()
            os.write(master, b'q')
        deadline = time.monotonic() + 5
        while proc.poll() is None and time.monotonic() < deadline:
            drain(0.1)
        require(proc.wait(timeout=2) == 0, 'unclean exit')
        drain()
        require(termios.tcgetattr(slave) == original, 'terminal flags not restored')
        for sequence in [b'\x1b[?1049h', b'\x1b[?1049l', b'\x1b[?2004h', b'\x1b[?2004l']:
            require(sequence in output, 'missing lifecycle sequence')
        if kitty:
            require(b'd=I,i=' + image_id in output, 'owned pixels not freed')
            if tmux_graphics:
                require(b'a=p,U=1,' in output and '\U0010eeee'.encode() in output, 'missing virtual placement or pane-relative text')
                require(b'\x1b[?2026h' not in output, 'raw synchronization bypasses tmux')
                require(all(b'q=2' in h for h, _ in commands), 'tmux command can trigger replies')
            else:
                require(b'\x1b[?2026h' in output and b'\x1b[?2026l' in output, 'synchronized frame lifecycle missing')
        expected_query = not tmux_graphics and mode not in ['off', 'mono', 'force', 'multiplexer', 'screen', 'tmux-term', 'screen-term', 'dumb']
        require(query_seen == expected_query, 'wrong query policy')
        if query_seen:
            require(bytes(output).count(b'a=q,') == 1, 'query repeated')
        print('PASS graphics PTY:', mode, 'real PDF API' if live and live.get('pdf') else 'real API' if live else 'synthetic demo')
    except BaseException:
        import tempfile
        with tempfile.NamedTemporaryFile(prefix="freight-graphics-pty-failure-", suffix=".bin", delete=False) as trace:
            trace.write(output)
            print("Owned synthetic trace:", trace.name)
        raise
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()
        os.close(master)
        os.close(slave)


@contextmanager
def api_fixture(binary, pdf=False):
    # Synthetic credentials are scoped only to this owned, ephemeral listener.
    token = 'owned-graphics-viewer-fixture-credential-32'
    operator = 'owned-graphics-operator-fixture-credential-32'
    keys = {token: {'tenant_id': 'demo', 'subject': 'fixture:viewer', 'role': 'viewer'},
            operator: {'tenant_id': 'demo', 'subject': 'fixture:operator', 'role': 'operations'}}
    with tempfile.TemporaryDirectory(prefix='freight-graphics-live-') as directory:
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        origin = 'http://127.0.0.1:%d' % port
        env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin'), 'LANG': 'C.UTF-8',
               'FREIGHT_DEMO': 'true', 'FREIGHT_BIND_ADDR': '127.0.0.1:%d' % port,
               'FREIGHT_API_KEYS_JSON': json.dumps(keys), 'FREIGHT_INTEGRATIONS_JSON': '{}',
               'TOKIO_WORKER_THREADS': '2', 'RUST_LOG': 'warn'}
        Path(directory, '.env').write_text('# owned empty fixture\n')
        with open(Path(directory, 'api.log'), 'wb') as log:
            api = subprocess.Popen([str(binary)], cwd=directory, env=env, stdout=log, stderr=log)
            try:
                def request(path, credential=token, body=None, media=None):
                    headers = {'Authorization': 'Bearer ' + credential}
                    if media:
                        headers.update({'Content-Type': media, 'Idempotency-Key': 'owned-graphics-upload'})
                    return urllib.request.urlopen(urllib.request.Request(origin + path, data=body, headers=headers), timeout=5).read()
                deadline = time.monotonic() + 20
                while True:
                    require(api.poll() is None, 'owned API exited early')
                    try:
                        loads = json.loads(request('/v1/loads?limit=50'))['loads']
                        break
                    except OSError:
                        require(time.monotonic() < deadline, 'owned API did not become ready')
                        time.sleep(.1)
                load = loads[0]
                # A small PNG generated from explicit synthetic RGB pixels.
                width, height = 360, 480
                rows = bytearray()
                for y in range(height):
                    rows.append(0)
                    for x in range(width):
                        color = (20,72,89) if 24 <= x < 336 and 24 <= y < 86 else (250,250,247)
                        rows.extend(color)
                def chunk(kind, data):
                    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data) & 0xffffffff)
                png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width,height,8,2,0,0,0)) + chunk(b'IDAT',zlib.compress(rows)) + chunk(b'IEND',b'')
                if pdf:
                    original = (Path(__file__).resolve().parents[1] / 'tests/fixtures/documents/preview-two-pages.pdf').read_bytes()
                    record = json.loads(request('/v1/loads/%s/documents?kind=pod&filename=owned.pdf' % load['id'], operator, original, 'application/pdf'))['data']
                else:
                    record = json.loads(request('/v1/loads/%s/documents?kind=pod&filename=owned.png' % load['id'],operator,png,'image/png'))['data']
                require(record['load_id'] == load['id'], 'fixture document binding')
                yield {'origin': origin, 'token': token, 'reference': load['reference'], 'pdf': pdf}
            finally:
                api.terminate()
                try:
                    api.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    api.kill()
                    api.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', nargs='?', default='target/debug/freight-tui')
    parser.add_argument('--pdf', action='store_true', help='Exercise both pages of the synthetic PDF through the actual API')
    parser.add_argument('--api-binary', help='Run real API plus TUI preview checks on an owned in-memory tenant')
    args = parser.parse_args()
    if args.pdf and not args.api_binary:
        parser.error('--pdf requires --api-binary')
    binary = Path(args.binary).resolve()
    if args.api_binary:
        with api_fixture(Path(args.api_binary).resolve(), pdf=args.pdf) as live:
            for mode in ['auto', 'negative', 'off', 'tmux-graphics', 'tmux-graphics-mono']:
                exercise(binary, mode, live)
    else:
        for mode in ['auto', 'signal', 'off', 'unknown', 'mono', 'unknown-supported', 'fragmented', 'early-key', 'force', 'multiplexer', 'screen', 'tmux-term', 'screen-term', 'dumb', 'negative', 'wrong-id', 'late', 'oversized', 'tmux-graphics', 'tmux-graphics-signal', 'tmux-graphics-mono', 'tmux-graphics-env-mono']:
            exercise(binary, mode)


if __name__ == '__main__':
    main()
