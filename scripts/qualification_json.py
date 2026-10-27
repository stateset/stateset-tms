"""One owned process for exact original JSON work during offered traffic.

This changes the qualification controller, never API processing or its budgets.
The socket pair is private to parent/child; pickle globals are restricted to
Decimal. Input validation precedes serialization and rejects arbitrary Python
objects. No database credentials or API tokens are sent to the child.
"""

from decimal import Decimal
from contextlib import contextmanager
import importlib.util
import io
import json
import multiprocessing
from pathlib import Path
import pickle
import socket
import struct
import threading
import time

PLAN = dict(mode="spawned-original-json-v2", workers=1,
            max_frame_bytes=128 * 1024 * 1024, timeout_seconds=30)


def validate_plan(value):
    if value is not None and (
        type(value) is not dict or value != PLAN
        or any(type(value[k]) is not type(v) for k, v in PLAN.items())
    ):
        raise ValueError("Unsupported original JSON worker plan")
    return value


def _json_value(value, active=None, depth=0):
    """Reject custom reducers, non-string keys and cycles before pickling."""
    if type(value) in (str, int, float, bool, type(None), Decimal):
        return
    if depth > 512:
        raise ValueError("Original JSON nesting exceeds worker bound")
    if type(value) not in (dict, list):
        raise TypeError("Original JSON worker accepts only literal JSON types")
    if active is None:
        active = set()
    if id(value) in active:
        raise ValueError("Circular original JSON value")
    active.add(id(value))
    try:
        if type(value) is dict:
            for key, item in value.items():
                if type(key) is not str:
                    raise TypeError("Original JSON object keys must be strings")
                _json_value(item, active, depth + 1)
        else:
            for item in value:
                _json_value(item, active, depth + 1)
    finally:
        active.remove(id(value))


class _LiteralUnpickler(pickle.Unpickler):
    def find_class(self, module, name):
        if (module, name) == ("decimal", "Decimal"):
            return Decimal
        raise pickle.UnpicklingError("Nonliteral original JSON IPC value")


def _unpack(raw):
    return _LiteralUnpickler(io.BytesIO(raw)).load()


def _remaining(sock, deadline):
    if deadline is None:
        sock.settimeout(None)
        return
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise TimeoutError("Original JSON worker deadline exceeded")
    sock.settimeout(remaining)


def _receive_exact(sock, count, deadline):
    chunks = []
    while count:
        _remaining(sock, deadline)
        try:
            chunk = sock.recv(min(count, 65536))
        except socket.timeout:
            raise TimeoutError("Original JSON worker receive deadline exceeded") from None
        if not chunk:
            raise EOFError("Original JSON worker disconnected")
        chunks.append(chunk)
        count -= len(chunk)
    return b"".join(chunks)


def _receive(sock, maximum, deadline):
    size = struct.unpack("!Q", _receive_exact(sock, 8, deadline))[0]
    if size > maximum:
        raise ValueError("Original JSON IPC frame exceeds bound")
    return _receive_exact(sock, size, deadline)


def _send(sock, raw, maximum, deadline):
    if len(raw) > maximum:
        raise ValueError("Original JSON IPC frame exceeds bound")
    _remaining(sock, deadline)
    try:
        sock.sendall(struct.pack("!Q", len(raw)))
        _remaining(sock, deadline)
        sock.sendall(raw)
    except socket.timeout:
        raise TimeoutError("Original JSON worker send deadline exceeded") from None


_ERRORS = {c.__name__: c for c in (
    TypeError, ValueError, OverflowError, RecursionError, AssertionError,
    UnicodeDecodeError, json.JSONDecodeError)}


def _worker(sock, maximum, timeout):
    try:
        spec = importlib.util.spec_from_file_location(
            "original_json_exact", Path(__file__).with_name("profile-storage-records.py"))
        exact = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(exact)
        mixed = None
        _send(sock, b"ready-v1", maximum, time.monotonic() + 10)
        while True:
            # An idle worker may wait through warmup; only active IPC has a
            # deadline. Parent close/EOF always releases this blocking header.
            header = _receive_exact(sock, 8, None)
            size = struct.unpack("!Q", header)[0]
            if not size:
                return
            if size > maximum:
                raise ValueError("Original JSON IPC frame exceeds bound")
            deadline = time.monotonic() + timeout
            operation, value = _unpack(_receive_exact(sock, size, deadline))
            try:
                if operation == "encode":
                    result = exact.encode(value)
                elif operation == "decode":
                    result = exact.decode(value)
                elif operation == "original":
                    original = exact.decode(value)
                    result = [original, exact.encode(original)]
                elif operation == "classify_write":
                    if mixed is None:
                        spec = importlib.util.spec_from_file_location(
                            "original_json_mixed", Path(__file__).with_name("qualify-api-mixed-envelope.py"))
                        mixed = importlib.util.module_from_spec(spec)
                        spec.loader.exec_module(mixed)
                    # Execute the complete existing proof with its native exact
                    # encoder, avoiding an IPC round trip for each root.
                    result = mixed.classify_write(*value)
                else:
                    raise ValueError("Unknown original JSON operation")
                response = ("ok", result)
            except tuple(_ERRORS.values()) as error:
                args = ((error.msg, error.doc, error.pos)
                        if isinstance(error, json.JSONDecodeError) else error.args)
                response = ("error", type(error).__name__, args)
            _send(sock, pickle.dumps(response, protocol=4), maximum, deadline)
    except (EOFError, OSError, ValueError, pickle.UnpicklingError):
        return
    finally:
        sock.close()


class OriginalJsonWorker:
    def __init__(self, maximum=PLAN["max_frame_bytes"], timeout=PLAN["timeout_seconds"]):
        if type(maximum) is not int or not 64 <= maximum <= PLAN["max_frame_bytes"]:
            raise ValueError("Invalid original JSON frame bound")
        if type(timeout) not in (int, float) or not 0 < timeout <= PLAN["timeout_seconds"]:
            raise ValueError("Invalid original JSON timeout")
        self.maximum, self.timeout = maximum, timeout
        self.lock = threading.Lock()
        self.closed, self.failure = False, None
        self.calls = {"encode": 0, "decode": 0}
        self.socket, child_socket = socket.socketpair()
        self.process = multiprocessing.get_context("spawn").Process(
            target=_worker, args=(child_socket, maximum, timeout), daemon=True)
        self.started = False
        try:
            self.process.start()
            self.started = True
            child_socket.close()
            ready = _receive(self.socket, maximum, time.monotonic() + 10)
            if ready != b"ready-v1":
                raise RuntimeError("Original JSON worker startup failed")
        except BaseException:
            child_socket.close()
            self._close("startup_failed")
            raise

    def _close(self, reason=None):
        self.closed = True
        if reason is not None:
            self.failure = reason
        self.socket.close()
        if self.started:
            self.process.join(0.2)
            if self.process.is_alive():
                self.process.terminate()
                self.process.join(0.5)
            if self.process.is_alive():
                self.process.kill()
                self.process.join(1.5)
            if self.process.is_alive():
                raise RuntimeError("Original JSON worker cleanup failed")

    @contextmanager
    def _turn(self):
        if not self.lock.acquire(timeout=self.timeout):
            raise TimeoutError("Original JSON worker queue deadline exceeded")
        try:
            yield
        finally:
            self.lock.release()

    def _call(self, operation, value):
        with self._turn():
            if self.closed:
                raise RuntimeError("Original JSON worker unavailable")
            if operation in ("encode", "classify_write"):
                _json_value(value)
            else:
                if type(value) not in (str, bytes, bytearray):
                    raise TypeError("Original JSON decode requires text or bytes")
                if type(value) is bytearray:
                    value = bytes(value)
            raw = pickle.dumps((operation, value), protocol=4)
            # Oversized caller input has not touched the wire; the worker is
            # still synchronized and may safely accept a later smaller call.
            if len(raw) > self.maximum:
                raise ValueError("Original JSON IPC frame exceeds bound")
            try:
                deadline = time.monotonic() + self.timeout
                _send(self.socket, raw, self.maximum, deadline)
                response = _unpack(_receive(self.socket, self.maximum, deadline))
            except BaseException:
                self._close("ipc_failed")
                raise
            self.calls[operation] = self.calls.get(operation, 0) + 1
            if response[0] == "error":
                error_type = _ERRORS.get(response[1])
                if error_type is None:
                    self._close("invalid_response")
                    raise RuntimeError("Original JSON worker response invalid")
                raise error_type(*response[2])
            if response[0] != "ok":
                self._close("invalid_response")
                raise RuntimeError("Original JSON worker response invalid")
            return response[1]

    def encode(self, value):
        return self._call("encode", value)

    def decode(self, value):
        return self._call("decode", value)

    def original(self, raw):
        return self._call("original", raw)

    def classify_write(self, before, after, body, key):
        return self._call("classify_write", [before, after, body, key])

    def close(self):
        with self.lock:
            if self.closed:
                return
            try:
                _send(self.socket, b"", self.maximum, time.monotonic() + 0.2)
            except (OSError, TimeoutError):
                pass
            finally:
                self._close()

    def status(self):
        return dict(mode=PLAN["mode"], workers=1, pid=self.process.pid,
                    calls=dict(self.calls), closed=self.closed,
                    alive=self.process.is_alive() if self.started else False,
                    exitcode=self.process.exitcode, failure=self.failure)
