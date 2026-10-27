"""Exact-byte, isolation and failure controls for the optional controller worker."""

from concurrent.futures import ThreadPoolExecutor
from decimal import Decimal
import importlib.util
import json
import os
from pathlib import Path
import pickle
import signal
import socket
import struct
import time
import unittest
from unittest import mock

import qualification_json as worker
import test_api_mixed_envelope as classification_controls

spec = importlib.util.spec_from_file_location(
    "json_worker_test_base", Path(__file__).with_name("qualify-api-sustained-envelope.py"))
sustained = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sustained)
base = sustained.base
exact = sustained.exact


class DangerousValue:
    def __reduce__(self):
        raise AssertionError("Custom reducer must never execute")


class JsonWorkerControls(unittest.TestCase):
    def own_worker(self, **kwargs):
        instance = worker.OriginalJsonWorker(**kwargs)
        self.addCleanup(instance.close)
        return instance

    def test_exact_complete_originals_and_numeric_representation(self):
        w = self.own_worker()
        raw = ('{"unknown":{"$serde_json::private::RawValue":"literal",'
               '"$serde_json::private::Number":"also literal"},'
               '"unicode":"東京 🚚 é", "large":123456789012345678901234567890,'
               '"scale":1.2300000000000000000000000001, "negative_zero":-0.0,'
               '"scientific":1.23e-100,"flags":[true,false,null],"text":"\\u0000\\n"}')
        value = exact.decode(raw)
        self.assertEqual(w.encode(value).encode(), exact.encode(value).encode())
        result = w.decode(raw.encode())
        self.assertIs(type(result["scale"]), Decimal)
        self.assertEqual(exact.encode(result), exact.encode(value))
        self.assertEqual(w.encode(w.decode(bytearray(raw.encode()))), exact.encode(value))
        self.assertEqual(w.status()["calls"], dict(encode=2, decode=2))

    def test_native_floats_empty_scalars_and_shared_literal_values(self):
        w = self.own_worker()
        repeated = ["literal", 1, Decimal("1.0")]
        for value in [None, True, 1, 1.0, -0.0, "", [], {}, [repeated, repeated]]:
            with self.subTest(value_type=type(value).__name__):
                self.assertEqual(w.encode(value), exact.encode(value))

    def test_combined_original_operation_preserves_both_full_results(self):
        w = self.own_worker()
        raw = '{"opaque":{"$serde_json::private::RawValue":"literal"},"n":1.2300000000000000001,"s":"東京"}'
        value, encoded = w.original(raw.encode())
        self.assertEqual(exact.encode(value), exact.encode(exact.decode(raw)))
        self.assertEqual(encoded, exact.encode(exact.decode(raw)))
        self.assertEqual(w.status()["calls"]["original"], 1)

    def test_decode_errors_preserve_types_and_worker_remains_usable(self):
        w = self.own_worker()
        for raw in [b"", b"{", b"[1,]", b'"\xff"']:
            try:
                exact.decode(raw)
            except Exception as expected:
                with self.assertRaises(type(expected)) as caught:
                    w.decode(raw)
                self.assertEqual(str(caught.exception), str(expected))
            else:
                self.fail("Negative fixture unexpectedly decoded")
        self.assertEqual(w.decode(b'{"ok":1}'), {"ok": 1})
        self.assertFalse(w.status()["closed"])

    def test_nonfinite_encode_errors_match_even_under_optimized_python(self):
        w = self.own_worker()
        for value in [float("nan"), float("inf"), Decimal("NaN"), Decimal("Infinity")]:
            try:
                exact.encode(value)
            except Exception as expected:
                with self.assertRaises(type(expected)):
                    w.encode(value)
            else:
                self.fail("Nonfinite input unexpectedly encoded")
        self.assertEqual(w.encode({"next": True}), '{"next":true}')

    def test_custom_values_keys_subclasses_and_cycles_refuse_before_pickle(self):
        w = self.own_worker()

        class CustomDict(dict):
            def __reduce__(self):
                raise AssertionError("Subclass reducer must never execute")

        class CustomString(str):
            pass

        cyclic = []
        cyclic.append(cyclic)
        for value in [DangerousValue(), {"a": DangerousValue()}, CustomDict(a=1),
                      {CustomString("a"): 1}, {1: "value"}, (1, 2), cyclic]:
            with self.subTest(value_type=type(value).__name__), self.assertRaises((TypeError, ValueError)):
                w.encode(value)
        self.assertEqual(w.encode({"ok": 1}), '{"ok":1}')
        self.assertEqual(w.status()["calls"]["encode"], 1)

    def test_restricted_unpickler_refuses_arbitrary_globals(self):
        with self.assertRaises(pickle.UnpicklingError):
            worker._unpack(pickle.dumps(Path("literal"), protocol=4))
        value = {"precision": Decimal("123.45000")}
        self.assertEqual(worker._unpack(pickle.dumps(value, protocol=4)), value)

    def test_concurrent_callers_share_one_serialized_worker(self):
        w = self.own_worker()
        with ThreadPoolExecutor(max_workers=4) as pool:
            values = list(pool.map(lambda n: w.decode(w.encode({"n": n})), range(16)))
        self.assertEqual(values, [{"n": n} for n in range(16)])
        self.assertEqual(w.status()["calls"], dict(encode=16, decode=16))
        self.assertEqual(w.status()["workers"], 1)

    def test_oversized_caller_frame_preserves_synchronization(self):
        w = self.own_worker(maximum=512)
        with self.assertRaises(ValueError):
            w.encode("x" * 1024)
        self.assertEqual(w.decode(b"true"), True)

    def test_queue_wait_is_bounded_without_poisoning_active_worker(self):
        w = self.own_worker(timeout=0.05)
        with w.lock:
            with self.assertRaisesRegex(TimeoutError, "queue deadline"):
                w.encode({"waiting": True})
        self.assertEqual(w.decode(b"true"), True)

    def test_startup_handshake_failure_reaps_child(self):
        states = []

        class StartupWorker(worker.OriginalJsonWorker):
            def _close(self, reason=None):
                super()._close(reason)
                states.append(self.status())

        with mock.patch.object(worker, "_receive", return_value=b"wrong-handshake"):
            with self.assertRaisesRegex(RuntimeError, "startup failed"):
                StartupWorker()
        self.assertEqual(len(states), 1)
        self.assertTrue(states[0]["closed"])
        self.assertFalse(states[0]["alive"])
        self.assertEqual(states[0]["failure"], "startup_failed")

    def test_oversized_response_closes_worker_without_retry(self):
        w = self.own_worker(maximum=512)
        with self.assertRaises(EOFError):
            w.encode([None] * 100)
        self.assertTrue(w.status()["closed"])
        self.assertFalse(w.status()["alive"])
        with self.assertRaises(RuntimeError):
            w.encode({"no_retry": True})

    def test_worker_death_refuses_and_reaps_its_child(self):
        w = self.own_worker()
        w.process.kill()
        w.process.join(2)
        with self.assertRaises((EOFError, OSError)):
            w.decode(b"null")
        self.assertFalse(w.status()["alive"])
        self.assertEqual(w.status()["failure"], "ipc_failed")

    @unittest.skipUnless(hasattr(signal, "SIGSTOP"), "Requires a stoppable owned Unix child")
    def test_deadline_kills_even_a_stopped_child(self):
        w = self.own_worker(timeout=0.1)
        os.kill(w.process.pid, signal.SIGSTOP)
        started = time.monotonic()
        with self.assertRaises(TimeoutError):
            w.encode({"wait": True})
        self.assertLess(time.monotonic() - started, 4)
        self.assertFalse(w.status()["alive"])
        self.assertTrue(w.status()["closed"])

    def test_partial_disconnect_and_oversized_header_refuse(self):
        a, b = socket.socketpair()
        self.addCleanup(a.close)
        b.sendall(struct.pack("!Q", 100))
        b.close()
        with self.assertRaises(EOFError):
            worker._receive(a, 512, time.monotonic() + 1)
        a.close()
        a, b = socket.socketpair()
        self.addCleanup(a.close)
        self.addCleanup(b.close)
        b.sendall(struct.pack("!Q", 513))
        with self.assertRaises(ValueError):
            worker._receive(a, 512, time.monotonic() + 1)

    def test_nonreading_peer_cannot_block_send_past_deadline(self):
        a, b = socket.socketpair()
        self.addCleanup(a.close)
        self.addCleanup(b.close)
        a.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 4096)
        started = time.monotonic()
        with self.assertRaises(TimeoutError):
            worker._send(a, b"x" * 1024**2, 2 * 1024**2, started + 0.05)
        self.assertLess(time.monotonic() - started, 1)

    def test_shutdown_is_idempotent_and_records_reaped_child(self):
        w = self.own_worker()
        pid = w.status()["pid"]
        w.close()
        w.close()
        self.assertEqual(w.status()["pid"], pid)
        self.assertTrue(w.status()["closed"])
        self.assertFalse(w.status()["alive"])
        self.assertEqual(w.status()["exitcode"], 0)
        with self.assertRaises(RuntimeError):
            w.decode(b"null")

    def test_only_exact_explicit_plan_selects_new_mode(self):
        self.assertIsNone(worker.validate_plan(None))
        self.assertEqual(worker.validate_plan(dict(worker.PLAN)), worker.PLAN)
        for bad in [{}, dict(worker.PLAN, workers=True), dict(worker.PLAN, timeout_seconds=30.0),
                    dict(worker.PLAN, workers=2), dict(worker.PLAN, timeout_seconds=31),
                    dict(worker.PLAN, unexpected="field"), dict(worker.PLAN, mode="spawned-original-json-v1")]:
            with self.assertRaises(ValueError):
                worker.validate_plan(bad)

    def test_missing_worker_preserves_native_helpers(self):
        envelope = base.Envelope.__new__(base.Envelope)
        value = {"literal": Decimal("1.23000000000000001")}
        self.assertEqual(envelope.encode_original(value), exact.encode(value))
        self.assertEqual(envelope.decode_original(exact.encode(value)), value)
        self.assertEqual(envelope.read_original_document(exact.encode(value)),
                         (value, exact.encode(value)))

    def test_resource_cleanup_failure_still_closes_worker(self):
        envelope = base.Envelope.__new__(base.Envelope)
        envelope.original_json = self.own_worker()
        envelope.report = {}

        def fail():
            raise RuntimeError("Retain unbacked volume")

        envelope._finish_resources = fail
        with self.assertRaisesRegex(RuntimeError, "Retain unbacked volume"):
            envelope.finish()
        self.assertFalse(envelope.report["client_json_processing"]["alive"])
        self.assertTrue(envelope.report["client_json_processing"]["closed"])

    def test_failed_json_observation_withholds_following_write(self):
        run = sustained.SustainedEnvelope.__new__(sustained.SustainedEnvelope)
        run.writer_uncertain = False

        def fail(name):
            raise TimeoutError("Original JSON worker unavailable")

        run._observe_write = fail
        with self.assertRaises(TimeoutError):
            run.observe_write("first")
        self.assertTrue(run.writer_uncertain)
        self.assertEqual(sustained.SustainedEnvelope._observe_write(run, "next"),
                         {"name": "next", "not_sent": "preceding_write_outcome_uncertain"})


class WorkerWriteClassificationControls(classification_controls.WriteClassificationTests):
    """Run the inherited independent original/receipt controls through real IPC."""

    def setUp(self):
        super().setUp()
        self.worker = worker.OriginalJsonWorker()
        self.addCleanup(self.worker.close)

    def classify(self, value):
        return self.worker.classify_write(self.before, value, self.body, self.key)


if __name__ == "__main__":
    unittest.main()
