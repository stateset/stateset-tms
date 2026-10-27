#!/usr/bin/env python3
"""Fixed-arrival financial traffic plus source-bound PNG/PDF evidence previews.

Keeps the inherited complete originals, receipt/replay and financial budgets.
A diagnostic is not sustained or production acceptance. Renderer resources are
additional to the declared API/PostgreSQL limits, never hidden in their budget.
"""
from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

from capacity_budget import percentile
from document_preview_runtime import Runtime, RENDERER_MEMORY_BYTES, validate_image
from qualification_artifacts import write_artifact

spec = importlib.util.spec_from_file_location('financial_previews_base',
    Path(__file__).with_name('qualify-financial-capacity.py'))
financial = importlib.util.module_from_spec(spec)
spec.loader.exec_module(financial)
base = financial.base.base
require = financial.base.require


def preview_schedule(duration):
    require(type(duration) is int and 10 <= duration <= 600 and duration % 10 == 0,
            'Preview duration must be a whole ten-second interval')
    return [(offset / 1000, kind, n) for n in range(duration // 10)
            for offset, kind in [(n * 10000 + 1500, 'png'), (n * 10000 + 6500, 'pdf')]]


def verify_preview(headers, raw, document, load, source, kind, page):
    fields = {k.lower(): v for k, v in headers.items()}
    width, height, color, pages = ((960, 960, (255, 0, 0), 1) if kind == 'png' else
        (720, 960, (255, 0, 0), 2) if page == 1 else (960, 720, (0, 0, 255), 2))
    expected = {'content-type': 'application/vnd.stateset.rgb8', 'cache-control': 'private, no-store',
        'x-content-type-options': 'nosniff', 'x-document-id': document, 'x-load-id': load,
        'x-source-sha256': source, 'x-content-sha256': hashlib.sha256(raw).hexdigest(),
        'x-preview-width': str(width), 'x-preview-height': str(height),
        'x-preview-page': str(page), 'x-preview-pages': str(pages)}
    require(all(fields.get(k) == v for k, v in expected.items()),
            'Preview identity, bounds, page or integrity differs')
    require(raw == bytes(color) * (width * height), 'Preview pixels differ from independent fixture')


def evaluate_previews(rows, duration):
    expected = {(kind, n) for _, kind, n in preview_schedule(duration)}
    seen = [(r['kind'], r['number']) for r in rows]
    violations = []
    if len(seen) != len(set(seen)) or set(seen) != expected:
        violations.append('missing_duplicate_or_unsent_preview')
    observations = {}
    for kind in ['png', 'pdf']:
        items = [r for r in rows if r['kind'] == kind]
        good = [r for r in items if r.get('status') == 200 and r.get('verified') is True
                and not r.get('error')]
        def timing(value):
            return type(value) in (int, float) and math.isfinite(value) and value >= 0
        valid = [r for r in items if timing(r.get('start_lateness_ms'))
                 and timing(r.get('wall_ms'))]
        if len(valid) != len(items):
            violations.append('invalid_preview_timing:' + kind)
        times = [r['start_lateness_ms'] + r['wall_ms'] for r in valid]
        late = [r['start_lateness_ms'] for r in valid]
        observations[kind] = {'offered': duration // 10, 'completed': len(items),
            'successes': len(good), 'p95_offered_to_response_ms': percentile(times, .95),
            'p99_start_lateness_ms': percentile(late, .99)}
        if len(good) != duration // 10:
            violations.append('preview_availability:' + kind)
        if not times or percentile(times, .95) > 5000:
            violations.append('preview_latency:' + kind)
        if not late or percentile(late, .99) > 1000:
            violations.append('preview_client_scheduling:' + kind)
    return {'result': 'pass' if not violations else 'fail', 'violations': violations,
        'kinds': observations, 'budget': {'success_fraction': 1, 'p95_ms': 5000,
            'p99_start_lateness_ms': 1000},
        'scope': 'Every additional absolute-clock preview counts; no retries or replacement arrivals.'}


class FinancialPreviews(financial.FinancialCapacity):
    def __init__(self, args):
        self.renderer = None
        self.preview_stop = threading.Event()
        self.preview_thread = self.renderer_thread = None
        self.renderer_stop = threading.Event()
        super().__init__(args)
        self.tokens['preview'] = base.secrets.token_urlsafe(36)
        self.secrets.append(self.tokens['preview'])
        self.preview_plan = self.plan['document_previews']
        require(set(self.preview_plan) == {'png', 'pdf', 'renderer_memory_mib', 'renderer_cpus',
                'renderer_pids', 'clock', 'budget'}, 'Incomplete preview plan')
        require(self.preview_plan['renderer_memory_mib'] == 384
                and self.preview_plan['renderer_cpus'] == 1 and self.preview_plan['renderer_pids'] == 16
                and self.preview_plan['clock'] == 'same-financial-absolute-clock: png+1500ms/pdf+6500ms each10000ms'
                and self.preview_plan['budget'] == {'success_fraction': 1, 'p95_ms': 5000,
                    'p99_start_lateness_ms': 1000}, 'Preview resource/arrival/response budget differs')
        preview_schedule(self.traffic['duration_seconds'])
        for kind in ['png', 'pdf']:
            item = self.preview_plan[kind]
            require(set(item) == {'path', 'sha256'} and base.sha(item['path']) == item['sha256'],
                    'Preview fixture changed')
        self.preview_rows = []
        self.report.update(preview_requests=self.preview_rows, renderer_resource_samples=[],
            document_previews=self.preview_plan,
            combined_resource_budget={'api_mib': args.api_memory_mib, 'postgres_mib': 768,
                'renderer_mib': 384, 'total_mib': args.api_memory_mib + 768 + 384,
                'api_cpus': 2, 'postgres_cpus': 2, 'renderer_cpus': 1})
        for name in ['qualify-financial-previews.py', 'document_preview_runtime.py']:
            self.report['harness']['scripts/' + name] = base.sha(Path(__file__).with_name(name))

    def seed_fixtures(self):
        return super().seed_fixtures() + [('preview', self.args.probe)]

    def command(self, command, *, label, **kwargs):
        if label == 'actual capped API image':
            directory = self.directory / 'renderer'
            directory.mkdir(mode=0o700)
            self.renderer = Runtime(directory, self.image)
            metadata = json.loads(self.renderer.docker('image', 'inspect', self.image))[0]
            validate_image(metadata['Config'])
            self.renderer.start_renderer()
            require(command[-1] == self.image, 'Unexpected API launch command')
            command = list(command[:-1]) + ['--read-only', '--cap-drop', 'ALL', '--security-opt',
                'no-new-privileges', '--pids-limit', '64', '--tmpfs',
                '/tmp:rw,nosuid,noexec,size=16777216', '--mount',
                'type=volume,src=' + self.renderer.socket_volume + ',dst=/run/freight-pdf,readonly',
                '-e', 'FREIGHT_PDF_RENDERER_SOCKET=/run/freight-pdf/renderer.sock',
                '-e', 'FREIGHT_PDF_RENDERER_UID=10002', self.image]
            self.report['renderer_binding'] = self.renderer.renderer_binding
        return super().command(command, label=label, **kwargs)

    def setup(self):
        super().setup()
        self.preview_documents = {}
        load = self.expected['preview']['id']
        for kind in ['png', 'pdf']:
            item = self.preview_plan[kind]
            path = '/v1/loads/' + load + '/documents?kind=pod&filename=capacity.' + kind
            request = urllib.request.Request(self.urls['freight'] + path,
                data=Path(item['path']).read_bytes(), headers={
                    'Authorization': 'Bearer ' + self.tokens['preview'],
                    'Content-Type': 'image/png' if kind == 'png' else 'application/pdf',
                    'Idempotency-Key': 'owned-capacity-' + kind})
            with self.opener.open(request, timeout=30) as response:
                require(response.status in [200, 201], 'Preview fixture upload failed')
                self.preview_documents[kind] = json.loads(response.read())['data']['id']
        self.report['preview_documents'] = self.preview_documents
        self.preview_original = self.encode_original(self.original('preview', 'preview-original-before'))
        self.preview_content = self.preview_contents()
        require(len(self.preview_content) == 2 and
            {r['source_sha256'] for r in self.preview_content} ==
            {self.preview_plan[k]['sha256'] for k in ['png', 'pdf']}, 'Stored originals differ')
        self.renderer_thread = threading.Thread(target=self.observe_renderer, daemon=True)
        self.renderer_thread.start()

    def preview_contents(self):
        return self.rows('freight_runtime', "SELECT document_id,octet_length(content) AS bytes,"
            "encode(sha256(content),'hex') AS source_sha256 FROM freight_document_content "
            "WHERE tenant_id='preview' ORDER BY document_id")

    def observe_renderer(self):
        while not self.renderer_stop.is_set():
            try:
                state = self.renderer.renderer()['State']
                row = {'at': time.monotonic(), 'state': state,
                       'counters': self.renderer.resources(renderer=True)}
                if row['counters']['cgroup_version'] == 2:
                    row['memory_peak_bytes'] = int(self.renderer.docker('exec',
                        self.renderer.renderer_cid, 'cat', '/sys/fs/cgroup/memory.peak'))
            except Exception as error:
                row = {'at': time.monotonic(), 'observer_error': self.safe(str(error))}
            self.report['renderer_resource_samples'].append(row)
            self.renderer_stop.wait(.5)

    def healthy(self):
        super().healthy()
        if self.renderer:
            require(self.renderer.renderer()['State']['Running'], 'Renderer exited')
            for row in self.report['renderer_resource_samples']:
                require('observer_error' not in row, 'Renderer observation failed')
                self.renderer.validate_resources(row['counters'], RENDERER_MEMORY_BYTES)
                require(not row['state']['OOMKilled'], 'Renderer OOM')

    def preview_request(self, kind, number, planned):
        row = {'kind': kind, 'number': number, 'planned_at': planned,
               'start_lateness_ms': (time.monotonic() - planned) * 1000, 'status': 0}
        started = time.monotonic()
        document = self.preview_documents[kind]
        page = (number % 2 + 1) if kind == 'pdf' else 1
        path = '/v1/documents/' + document + '/preview?page=' + str(page)
        raw = b''
        try:
            request = urllib.request.Request(self.urls['freight'] + path,
                headers={'Authorization': 'Bearer ' + self.tokens['preview']})
            try:
                response = self.opener.open(request, timeout=15)
            except urllib.error.HTTPError as error:
                response = error
            with response:
                raw = response.read(960 * 960 * 3 + 1)
                row.update(status=response.status, headers=dict(response.headers),
                           bytes=len(raw), wall_ms=(time.monotonic() - started) * 1000)
            require(row['status'] == 200, 'Preview HTTP status ' + str(row['status']))
            verify_preview(row['headers'], raw, document, self.expected['preview']['id'],
                self.preview_plan[kind]['sha256'], kind, page)
            row['verified'] = True
        except Exception as error:
            row['error'] = self.safe(type(error).__name__ + ': ' + str(error))
            row.setdefault('wall_ms', (time.monotonic() - started) * 1000)
        row['body'] = write_artifact(self.directory / ('preview-%s-%04d.rgb' % (kind, number)),
                                    raw, 'gzip')
        self.preview_rows.append(row)

    def run_previews(self):
        while 'traffic_started_monotonic' not in self.report:
            if self.preview_stop.wait(.01):
                return
        clock = self.report['traffic_started_monotonic']
        with ThreadPoolExecutor(max_workers=2) as pool:
            jobs = []
            for offset, kind, number in preview_schedule(self.traffic['duration_seconds']):
                if self.preview_stop.wait(max(0, clock + offset - time.monotonic())):
                    break
                jobs.append(pool.submit(self.preview_request, kind, number, clock + offset))
            for job in jobs:
                job.result()

    def checks(self):
        self.preview_thread = threading.Thread(target=self.run_previews, daemon=True)
        self.preview_thread.start()
        try:
            super().checks()
        finally:
            self.preview_stop.set()
            self.preview_thread.join(45)
            require(not self.preview_thread.is_alive(), 'Preview traffic did not finish')
            self.report['preview_budget_evaluation'] = evaluate_previews(
                self.preview_rows, self.traffic['duration_seconds'])
        expected_reads = len(financial.base.schedule(self.traffic['duration_seconds'],
            self.traffic['read_interval_ms']))
        self.report['combined_useful_read_contract'] = {
            'required_complete_reads': expected_reads,
            'actual_complete_reads': self.report.get('accepted_large_reads', 0)}
        require(self.report.get('accepted_large_reads') == expected_reads,
                'Combined capacity requires every offered large read to complete')
        require(self.report['preview_budget_evaluation']['result'] == 'pass',
                'Declared preview budgets failed')

    def verify_sources_after(self):
        super().verify_sources_after()
        if hasattr(self, 'preview_original'):
            try:
                require(self.encode_original(self.original('preview', 'preview-original-after'))
                        == self.preview_original and self.preview_contents() == self.preview_content,
                        'Preview rewrote original workspace, review or file bytes')
                self.report['preview_originals_unchanged'] = True
            except Exception as error:
                self.report.update(result='failed', preview_original_error=self.safe(str(error)))

    def finish(self):
        self.preview_stop.set()
        if self.preview_thread:
            self.preview_thread.join(45)
        self.renderer_stop.set()
        if self.renderer_thread:
            self.renderer_thread.join(30)
        try:
            super().finish()
        finally:
            if self.renderer:
                self.renderer.__exit__(None, None, None)


if __name__ == '__main__':
    raise SystemExit(base.main(FinancialPreviews))
