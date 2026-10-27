"""Capacity-budget, lossless evidence and uncertain-write negative controls."""

import copy
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from capacity_budget import KINDS, evaluate_budget, validate_budget
from financial_observer import original_query, substitute
from qualification_artifacts import read_artifact, write_artifact

spec = importlib.util.spec_from_file_location('financial_test', Path(__file__).with_name('qualify-financial-capacity.py'))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class BudgetControls(unittest.TestCase):
    def fixture(self):
        budget = {'schema': 1, 'max_p99_start_lateness_ms': 500,
                  'kinds': {kind: {'min_success_fraction': 1, 'max_p95_ms': 1000} for kind in KINDS}}
        traffic = dict(duration_seconds=2, **{kind + '_interval_ms': 1000 for kind in KINDS})
        report = {'declared_sustained': traffic, 'arrivals': [], 'requests': []}
        for kind in KINDS:
            for n in range(2):
                name = 'arrival-%s-%04d' % (kind, n)
                report['arrivals'].append({'name': name, 'start_lateness_ms': 10})
                report['requests'].append({'name': name, 'status': 200, 'wall_ms': 20})
        return report, budget

    def evaluate(self, report, budget):
        return evaluate_budget(report, budget, driver.base.schedule)

    def test_complete_offered_workload_passes_and_ignores_replays_and_warmup(self):
        report, budget = self.fixture()
        report['requests'].extend([{'name': 'warm-large'}, {'name': 'arrival-write-0000-replay'}])
        result = self.evaluate(report, budget)
        self.assertEqual(result['result'], 'pass')
        self.assertEqual(result['kinds']['read']['p95_offered_to_response_ms'], 30)

    def test_missing_unsent_duplicate_and_undeclared_arrivals_cannot_shrink_denominator(self):
        report, budget = self.fixture()
        for mutate in [lambda r: r['requests'].pop(), lambda r: r['arrivals'].pop(),
                       lambda r: r['arrivals'][0].update(not_sent='client_bound'),
                       lambda r: r['requests'].append(r['requests'][0]),
                       lambda r: r['arrivals'].append(dict(r['arrivals'][0], name='extra'))]:
            r = copy.deepcopy(report)
            mutate(r)
            self.assertEqual(self.evaluate(r, budget)['result'], 'fail')

    def test_busy_response_counts_against_availability_and_keeps_latency(self):
        report, budget = self.fixture()
        report['requests'][0].update(status=503, wall_ms=2500)
        result = self.evaluate(report, budget)
        self.assertEqual(result['kinds']['read']['success_fraction'], .5)
        self.assertEqual(result['kinds']['read']['p95_offered_to_response_ms'], 2510)
        self.assertIn('availability:read', result['violations'])
        self.assertIn('latency:read', result['violations'])

    def test_dispatch_delay_includes_coordinated_omission_in_response_budget(self):
        report, budget = self.fixture()
        report['arrivals'][0]['start_lateness_ms'] = 1000
        result = self.evaluate(report, budget)
        self.assertIn('latency:read', result['violations'])
        self.assertIn('client_scheduling:read', result['violations'])

    def test_transport_errors_and_missing_nonfinite_or_negative_timings_fail(self):
        report, budget = self.fixture()
        for values in [{'status': 0}, {'status': 200, 'error': 'disconnect'},
                       {'wall_ms': float('nan')}, {'wall_ms': -1}, {'wall_ms': None}]:
            r = copy.deepcopy(report)
            r['requests'][0].update(values)
            self.assertEqual(self.evaluate(r, budget)['result'], 'fail')

    def test_ambiguous_incomplete_nonfinite_and_trivial_budgets_refuse(self):
        _, budget = self.fixture()
        for values in [dict(budget, schema=True), dict(budget, kinds={}),
                       dict(budget, max_p99_start_lateness_ms=float('inf')),
                       dict(budget, kinds={kind: {'min_success_fraction': 0, 'max_p95_ms': 1} for kind in KINDS})]:
            with self.assertRaises(ValueError):
                validate_budget(values)


class EvidenceControls(unittest.TestCase):
    def test_plain_and_gzip_preserve_exact_arbitrary_bytes_and_refuse_overwrite(self):
        raw = b'\x00\xfforiginal\r\n' * 100
        with tempfile.TemporaryDirectory() as directory:
            for compression in [None, 'gzip']:
                path = Path(directory) / ('plain' if compression is None else 'compressed')
                record = write_artifact(path, raw, compression)
                self.assertEqual(read_artifact(record), raw)
                before = Path(record['path']).read_bytes()
                with self.assertRaises(FileExistsError):
                    write_artifact(path, b'changed', compression)
                self.assertEqual(Path(record['path']).read_bytes(), before)

    def test_changed_stored_or_uncompressed_digest_size_and_encoding_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            record = write_artifact(Path(directory) / 'original', b'original', 'gzip')
            for change in [{'sha256': '0' * 64}, {'uncompressed_sha256': '0' * 64},
                           {'uncompressed_bytes': 1}, {'uncompressed_bytes': True}, {'compression': 'zip'}]:
                with self.subTest(change=change), self.assertRaises(ValueError):
                    read_artifact(dict(record, **change))
            Path(record['path']).write_bytes(b'changed')
            with self.assertRaises(ValueError):
                read_artifact(record)

    def test_compressed_evidence_is_deterministic(self):
        with tempfile.TemporaryDirectory() as directory:
            a = write_artifact(Path(directory) / 'a', b'original' * 100, 'gzip')
            b = write_artifact(Path(directory) / 'b', b'original' * 100, 'gzip')
            self.assertEqual(a['sha256'], b['sha256'])


class WriteAndObserverControls(unittest.TestCase):
    def test_verification_exception_prevents_subsequent_new_write(self):
        cls = driver.base.SustainedEnvelope
        instance = cls.__new__(cls)
        instance.writer_uncertain = False
        with patch.object(cls, '_observe_write', side_effect=AssertionError('damaged original')):
            with self.assertRaises(AssertionError):
                instance.observe_write('owned')
        self.assertTrue(instance.writer_uncertain)
        self.assertEqual(instance.observe_write('next'), {'name': 'next', 'not_sent': 'preceding_write_outcome_uncertain'})

    def test_interruption_also_closes_write_guard(self):
        cls = driver.base.SustainedEnvelope
        instance = cls.__new__(cls)
        instance.writer_uncertain = False
        with patch.object(cls, '_observe_write', side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                instance.observe_write('owned')
        self.assertTrue(instance.writer_uncertain)

    def test_observer_only_substitutes_exact_escaped_literal_read(self):
        statement = "SELECT freight_workspace_originals('tenant''quote')::text"
        self.assertEqual(substitute(statement), original_query("'tenant''quote'"))
        for statement in ["DELETE FROM freight_workspaces", 'SELECT freight_workspace_originals($1)::text',
                          "SELECT freight_workspace_originals('a')::text; DROP TABLE x"]:
            self.assertEqual(substitute(statement), statement)
        with self.assertRaises(ValueError):
            original_query("'a'); DROP TABLE x; --")

    def test_http_evidence_compression_preserves_bytes_and_existing_file(self):
        cls = driver.base.base.Envelope
        raw = b'{"exact":"Z\\u00fcrich"}'
        class Response(io.BytesIO):
            status = 200
            headers = {}
        with tempfile.TemporaryDirectory() as directory:
            instance = cls.__new__(cls)
            instance.directory = Path(directory)
            instance.evidence_compression = 'gzip'
            instance.tokens = {'owned': 'synthetic'}
            instance.urls = {'freight': 'http://127.0.0.1:1'}
            instance.safe = lambda value: value
            class Opener:
                def open(self, *_args, **_kwargs):
                    return Response(raw)
            instance.opener = Opener()
            result = instance.request('owned', '/owned', 'first')
            self.assertEqual(read_artifact(result['response']), raw)
            self.assertEqual(result['bytes'], len(raw))
            self.assertFalse((Path(directory) / 'first.json').exists())
            original = Path(directory) / 'existing.json'
            original.write_bytes(b'prior')
            result = instance.request('owned', '/owned', 'existing')
            self.assertIn('error', result)
            self.assertNotIn('response', result)
            self.assertEqual(original.read_bytes(), b'prior')


class ChainedWriteControls(unittest.TestCase):
    def workspace(self):
        load = {'id': 'writer-load', 'version': 1, 'location': 'Origin', 'reference': 'W-1',
                'updated_at': '2026-10-22T00:00:00Z', 'last_tracking_at': '2026-10-22T00:00:00Z'}
        return {'loads': [load, {'id': 'other', 'version': 4}], 'events': [{'id': 'seed'}],
                'receipts': {'seed': {'fingerprint': 'f', 'response': {}}}, 'bills': [{'id': 'b1'}]}

    def commit(self, state, name, n):
        """Apply one actual-shaped location command, returning body/key/load."""
        mixed = driver.base.mixed
        body = {'action': 'location', 'expected_version': state['loads'][0]['version'],
                'location': 'Synthetic sustained observation ' + name}
        key = 'sustained-original-' + name
        load = dict(state['loads'][0], version=body['expected_version'] + 1, location=body['location'],
                    updated_at='2026-10-22T00:00:%02dZ' % n, last_tracking_at='2026-10-22T00:00:%02dZ' % n)
        state['loads'][0] = load
        state['events'].append({'entity_id': load['id'], 'kind': 'load.location', 'actor': 'envelope:operator',
                                'message': 'Location updated: ' + body['location']})
        signature = mixed.hashlib.sha256(mixed.json.dumps(
            ['load.command', load['id'], body], sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        state['receipts'][key] = {'fingerprint': signature, 'response': copy.deepcopy(load)}
        return body, key, copy.deepcopy(load)

    def chain(self):
        before = self.workspace()
        after = copy.deepcopy(before)
        first = self.commit(after, 'arrival-write-0000', 1)
        busy = ({'action': 'location', 'expected_version': 2,
                 'location': 'Synthetic sustained observation arrival-write-0001'},
                'sustained-original-arrival-write-0001', None)
        third = self.commit(after, 'arrival-write-0002', 2)
        return before, after, [first, busy, third]

    def classify(self, before, after, writes):
        return driver.base.classify_chain(before, after, writes, driver.base.mixed.classify_write)

    def test_committed_and_busy_writes_classify_exactly_once(self):
        before, after, writes = self.chain()
        self.assertEqual(self.classify(before, after, writes), ['committed_once', 'unapplied', 'committed_once'])

    def test_busy_response_that_actually_committed_refuses(self):
        before, after, writes = self.chain()
        hidden = self.commit(after, 'arrival-write-0003', 3)
        writes.append((hidden[0], hidden[1], None))
        with self.assertRaises(AssertionError):
            self.classify(before, after, writes)

    def test_changed_unrelated_original_or_response_or_missing_receipt_refuses(self):
        for damage in ['unrelated', 'response', 'receipt', 'other_load']:
            before, after, writes = self.chain()
            if damage == 'unrelated':
                after['bills'][0]['id'] = 'rewritten'
            elif damage == 'response':
                writes[0][2]['location'] = 'Different'
            elif damage == 'receipt':
                del after['receipts'][writes[2][1]]
            else:
                after['loads'][1]['version'] = 5
            with self.subTest(damage=damage), self.assertRaises((AssertionError, KeyError)):
                self.classify(before, after, writes)

    def sender(self, status, value):
        cls = driver.base.SustainedEnvelope
        instance = cls.__new__(cls)
        instance.writer_uncertain = False
        instance.writer_version = 1
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'response.json'
            artifact = write_artifact(path, driver.base.exact.encode(value).encode(), 'gzip')
            instance.request = lambda *_args, **_kwargs: {'status': status, 'response': artifact}
            observation = {'body': {'action': 'location', 'expected_version': 1, 'location': 'Owned'},
                           'key': 'owned', 'path': '/owned'}
            try:
                instance._send_chained_write('owned', observation)
            finally:
                path.unlink(missing_ok=True)
        return instance, observation

    def test_in_band_write_trusts_only_a_matching_original_response(self):
        load = {'version': 2, 'location': 'Owned'}
        instance, observation = self.sender(200, {'data': load, 'replayed': False})
        self.assertEqual((instance.writer_version, observation['response_load']), (2, load))
        instance, observation = self.sender(503, {'error': {'code': 'write_capacity_exceeded'}})
        self.assertEqual((instance.writer_version, observation['response_load']), (1, None))
        for value in [{'data': load, 'replayed': True}, {'data': dict(load, version=3), 'replayed': False},
                      {'data': dict(load, location='Other'), 'replayed': False}]:
            with self.subTest(value=value), self.assertRaises(AssertionError):
                self.sender(200, value)
        with self.assertRaises(AssertionError):
            self.sender(409, {'error': {'message': 'Load changed'}})

    def test_write_verification_modes_are_declared(self):
        self.assertEqual(driver.base.WRITE_VERIFICATION, {'per_write', 'post_traffic_chain'})
