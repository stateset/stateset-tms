#!/usr/bin/env python3
"""Qualify PDF cancellation, worker faults and finite overload on a local API image.

Uses owned synthetic data, separate constrained API/renderer containers and an
owned tmpfs socket volume, with no shared database/credentials or remote Docker context. This is not sustained capacity,
an OS vulnerability assessment or proof that an arbitrary image matches source.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import socket
import sys
import tempfile
import threading
import time
import traceback

from document_preview_runtime import Runtime, VIEWER, RESPONSE_BYTES, require

ROOT = Path(__file__).resolve().parents[1]


def heavy_pdf():
    """Deterministic, valid near-limit PDF; repeated fills exercise CPU admission."""
    stream = b'0 0 1 rg 0 0 240 320 re f\n' * 250000
    objects = [
        b'<< /Type /Catalog /Pages 2 0 R >>',
        b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
        b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 240 320] /Resources << >> /Contents 4 0 R >>',
        b'<< /Length ' + str(len(stream)).encode() + b' >>\nstream\n' + stream + b'endstream',
    ]
    pdf = bytearray(b'%PDF-1.4\n')
    offsets = []
    for number, obj in enumerate(objects, 1):
        offsets.append(len(pdf))
        pdf.extend(str(number).encode() + b' 0 obj\n' + obj + b'\nendobj\n')
    xref = len(pdf)
    pdf.extend(('xref\n0 %d\n0000000000 65535 f \n' % (len(objects) + 1)).encode())
    for offset in offsets:
        pdf.extend(('%010d 00000 n \n' % offset).encode())
    pdf.extend(('trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n' %
                (len(objects) + 1, xref)).encode())
    require(len(pdf) < 10 * 1024 * 1024, 'Synthetic PDF exceeds upload limit')
    return bytes(pdf)


def percentile(values, fraction):
    require(values, 'Missing latency observations')
    return sorted(values)[max(0, int(len(values) * fraction + .999) - 1)]


def validate_wave(wave, document, load, source_sha):
    admitted = [response for response in wave if response[0] != 503]
    require(len(wave) == 12 and len(admitted) == 1 and admitted[0][0] in [200, 422],
            'Concurrent arrivals did not enforce one bounded heavy renderer')
    require(all(response[3] <= 500 for response in wave if response[0] == 503),
            'Busy response exceeded its predeclared budget')
    require(admitted[0][3] <= 7000, 'Admitted preview exceeded its finite-workload deadline')
    if admitted[0][0] == 200:
        # Faster hardware/renderers may finish within the CPU budget. Success is
        # valid only with the independently specified complete source pixels.
        _, headers, rgb, _ = admitted[0]
        fields = {key.lower(): value for key, value in headers.items()}
        require(fields.get('content-type') == 'application/vnd.stateset.rgb8'
                and fields.get('x-preview-width') == '720'
                and fields.get('x-preview-height') == '960'
                and fields.get('x-preview-page') == fields.get('x-preview-pages') == '1'
                and fields.get('x-document-id') == document and fields.get('x-load-id') == load
                and fields.get('x-source-sha256') == source_sha
                and fields.get('x-content-sha256') == hashlib.sha256(rgb).hexdigest(),
                'Successful heavy preview has invalid identity or integrity')
        require(rgb == bytes([0, 0, 255]) * (720 * 960), 'Successful heavy preview has wrong pixels')


def run(runtime, report, work):
    pdf = heavy_pdf()
    fixture = ROOT / 'tests/fixtures/documents/preview-text.pdf'
    text = fixture.read_bytes()
    (work / 'fixture-binding.json').write_text(json.dumps(dict(
        heavy_bytes=len(pdf), heavy_sha256=hashlib.sha256(pdf).hexdigest(),
        text_sha256=hashlib.sha256(text).hexdigest()), indent=2) + '\n')
    heavy = runtime.upload('heavy.pdf', pdf, 'owned-pressure-heavy')
    text_id = runtime.upload('text.pdf', text, 'owned-pressure-text')
    heavy_path = '/v1/documents/' + heavy + '/preview'
    text_path = '/v1/documents/' + text_id + '/preview'
    before = runtime.request('/v1/loads/' + runtime.load + '/documents')[2]
    observations = report['read_observations']
    stop = threading.Event()

    def healthy():
        status, headers, body, _ = runtime.request(text_path)
        require(status == 200, 'Text preview retry failed: ' + str(status))
        fields = {key.lower(): value for key, value in headers.items()}
        require(fields['x-source-sha256'] == hashlib.sha256(text).hexdigest(), 'Text source changed')
        require(fields['x-content-sha256'] == hashlib.sha256(body).hexdigest(), 'Text pixels corrupted')
        require(fields['x-preview-page'] == fields['x-preview-pages'] == '1', 'Text page identity changed')

    def ordinary_reads():
        while not stop.is_set():
            for name, path, credential in [('health', '/health', None), ('ready', '/ready', None),
                                           ('catalog', '/v1/loads?limit=50', VIEWER)]:
                status, _, body, milliseconds = runtime.request(path, credential)
                observations.append(dict(kind=name, status=status, ms=milliseconds,
                                         body_sha256=hashlib.sha256(body).hexdigest()))
                require(status == 200, 'Ordinary API read failed')
            stop.wait(.05)

    healthy()
    with ThreadPoolExecutor(max_workers=16) as pool:
        monitor = pool.submit(ordinary_reads)
        try:
            # A real HTTP disconnect, after a real renderer has been stopped.
            with socket.create_connection(('127.0.0.1', runtime.binding['port']), timeout=3) as client:
                request = ('GET ' + heavy_path + ' HTTP/1.1\r\nHost: 127.0.0.1\r\n' +
                           'Authorization: Bearer ' + VIEWER + '\r\nConnection: close\r\n\r\n')
                client.sendall(request.encode())
                renderer = runtime.wait_renderer()
                runtime.signal(renderer, 'STOP')
            started = time.monotonic()
            busy = list(pool.map(lambda _: runtime.request(heavy_path), range(8)))
            report['cancelled_stopped_worker'] = dict(renderer=renderer, busy=[
                dict(status=result[0], ms=result[3]) for result in busy])
            require(all(result[0] == 503 and result[3] <= 500 for result in busy),
                    'Cancellation released the permit early or busy responses were slow')
            while runtime.renderers():
                require(time.monotonic() - started < 7, 'Stopped renderer was not killed/reaped')
                time.sleep(.05)
            report['cancelled_stopped_worker']['recovery_seconds'] = time.monotonic() - started
            healthy()

            future = pool.submit(runtime.request, heavy_path)
            renderer = runtime.wait_renderer()
            runtime.signal(renderer, 'KILL')
            response = future.result(timeout=7)
            report['killed_worker'] = dict(renderer=renderer, status=response[0], ms=response[3])
            require(response[0] == 422, 'Killed renderer did not fail safely')
            require(not runtime.renderers(), 'Killed renderer was not reaped')
            healthy()

            gate = threading.Barrier(12)

            def simultaneous(_):
                gate.wait(timeout=3)
                return runtime.request(heavy_path)

            wave = list(pool.map(simultaneous, range(12)))
            report['concurrent_wave'] = [dict(status=result[0], ms=result[3]) for result in wave]
            validate_wave(wave, heavy, runtime.load, hashlib.sha256(pdf).hexdigest())
            require(not runtime.renderers(), 'CPU-limited renderer was not reaped')
            healthy()
        finally:
            # A failing assertion must stop the monitor before pool.__exit__ waits.
            stop.set()
            monitor.result(timeout=5)

    require(before == runtime.request('/v1/loads/' + runtime.load + '/documents')[2],
            'Faults changed document metadata or review state')
    require(runtime.request('/v1/documents/' + heavy + '/content')[2] == pdf, 'Heavy original changed')
    require(runtime.request('/v1/documents/' + text_id + '/content')[2] == text, 'Text original changed')
    require(runtime.owned()['State']['Running'], 'API exited during qualification')
    report['ordinary_reads'] = {}
    for name, limit in [('health', 250), ('ready', 250), ('catalog', 500)]:
        values = [row['ms'] for row in observations if row['kind'] == name]
        require(len(values) >= 50, 'Insufficient ' + name + ' observations')
        require(percentile(values, .95) <= limit, name + ' latency budget exceeded')
        report['ordinary_reads'][name] = dict(count=len(values), p95_ms=percentile(values, .95),
                                              max_ms=max(values), budget_ms=limit)
    report['originals_and_pending_review_preserved'] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', required=True, help='Already built local Freight API image; never pulled')
    parser.add_argument('--output', required=True, type=Path, help='Fresh JSON report; artifacts retained beside it')
    args = parser.parse_args()
    output = args.output.resolve()
    require(not args.output.is_symlink() and not output.exists(), 'Preserve reports; choose a fresh output')
    require(sys.platform == 'linux', 'Preview fault qualification requires a Linux host and local Docker Engine')
    output.parent.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix='freight-preview-qualification-', dir=str(output.parent)))
    inputs = [Path(__file__), ROOT / 'scripts/document_preview_runtime.py',
              ROOT / 'tests/fixtures/documents/preview-text.pdf']
    hashes = {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in inputs}
    plan = dict(schema_version=1, target_image=args.image, heavy_fill_operations=250000,
                cpu_limit=2, memory_limit_bytes=512 * 1024 * 1024, pids_limit=64,
                renderer_cpu_limit=1, renderer_memory_limit_bytes=384 * 1024 * 1024,
                renderer_pids_limit=16, renderer_network="none",
                resource_scope="API limits unchanged; dedicated renderer adds one CPU/384MiB/16PIDs. "
                               "This is not a same-resource performance comparison.",
                health_p95_ms=250, ready_p95_ms=250, catalog_p95_ms=500, minimum_reads_per_kind=50,
                busy_max_ms=500, wall_recovery_seconds=7, simultaneous_heavy_arrivals=12,
                admitted_max_ms=7000, admitted_statuses=[200, 422], response_byte_limit=RESPONSE_BYTES,
                scope='Finite cancelled/stopped/killed/overloaded PDF checks with ordinary reads; '
                      'not sustained capacity, a complete release matrix, independent security '
                      'clearance or proof that a supplied image matches application source.')
    (work / 'plan.json').write_text(json.dumps(plan, indent=2, sort_keys=True) + '\n')
    report = dict(schema_version=1, plan=plan, inputs=hashes, artifacts=str(work),
                  read_observations=[], result='failure')
    try:
        with Runtime(work, args.image) as runtime:
            report['binding'] = runtime.binding
            run(runtime, report, work)
        require(hashes == {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in inputs},
                'Qualification inputs changed during execution')
        report['result'] = 'pass'
    except BaseException as error:
        report['error'] = str(error)
        (work / 'failure.log').write_text(traceback.format_exc())
        raise
    finally:
        report['retained_artifacts'] = [dict(path=str(path), bytes=path.stat().st_size,
            sha256=hashlib.sha256(path.read_bytes()).hexdigest()) for path in sorted(work.rglob('*'))
            if path.is_file()]
        with output.open('x') as stream:
            json.dump(report, stream, indent=2, sort_keys=True)
            stream.write('\n')
    print('PASS PDF cancellation, worker faults, finite overload, originals and ordinary API reads')
    print('REPORT', output)


if __name__ == '__main__':
    main()
