#!/usr/bin/env python3
"""Drive the real TUI through bounded pages on an owned HTTP fixture/PTY.

The API's own financial, tenant and cursor semantics are tested independently
in console_catalog_tests.rs. This fixture observes the terminal's HTTP reads.
"""
import fcntl
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import select
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
from urllib.parse import parse_qs, urlsplit


def exercise(binary):
    requests, failures = [], []
    token = "owned-catalog-terminal-credential-canary"
    identity = {"tenant_id": "owned-catalog", "access": {
        "subject": "user:owned-reader", "role": "viewer", "capabilities": ["workspace.read"]},
        "features": {"console_catalog": True}}
    bills = [{"id": f"bill-{n:03}", "number": f"INV-{n:03}", "load_id": f"BILL-{n:03}",
              "customer_id": "owned-customer", "status": "issued", "version": 1,
              "snapshot": {"amount_cents": 100}, "totals": {"balance_cents": 100, "overdue_days": 1}}
             for n in range(123)]
    settlements = [{"id": f"settlement-{n:03}", "load_id": f"PAY-{n:03}", "carrier_id": "owned-carrier",
                    "status": "prepared", "version": 1, "total_amount_cents": 100, "balance_cents": 100}
                   for n in range(123)]
    queues = [(direction, {"id": f"same-{n:03}", field: f"{prefix}-{n:03}", "destination": "owned-destination",
                           "status": "failed" if n == 60 else "sent", "attempts": 1, "total_attempts": 1})
              for direction, field, prefix in [("outbox", "load_id", "OUT"), ("inbox", "document_id", "IN")]
              for n in range(61)]

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            try:
                path = urlsplit(self.path)
                # Name the rejected path and peer; never the presented header.
                assert self.headers.get("authorization") == "Bearer " + token, \
                    "request without the owned credential: GET %s from %s" % (path.path, self.client_address[0])
                params = parse_qs(path.query)
                requests.append({"path": path.path, "query": params})
                if path.path == "/v1/access":
                    data = identity
                else:
                    assert path.path == "/v1/console", "whole-workspace read"
                    assert params["limit"] == ["50"]
                    view = params["view"][0]
                    q = params.get("q", [""])[0].strip().lower()
                    if view in ["billing", "payables"]:
                        assert "direction" not in params and "attention" not in params
                        rows = [("bills" if view == "billing" else "settlements", r)
                                for r in (bills if view == "billing" else settlements)]
                        assert params["balance"][0] in ["all", "open"]
                    else:
                        assert view == "integrations" and "balance" not in params
                        rows = [(direction, r) for direction, r in queues
                                if params.get("direction", ["all"])[0] in ["all", {"outbox": "outbound", "inbox": "inbound"}[direction]]]
                        if params.get("attention", ["false"])[0] == "true":
                            rows = [(direction, r) for direction, r in rows if r["status"] == "failed"]
                    rows = [(direction, r) for direction, r in rows if not q or q in json.dumps(r).lower()]
                    scope = hashlib.sha256(json.dumps({k: v for k, v in params.items() if k != "cursor"}, sort_keys=True).encode()).hexdigest()
                    cursor = params.get("cursor", [None])[0]
                    start = 0
                    if cursor:
                        offset, previous_scope = cursor.split(":")
                        assert previous_scope == scope, "stale cursor after scope change"
                        start = int(offset)
                    selected = rows[start:start + 50]
                    data = {"tenant_id": "owned-catalog", "view": view, "limit": 50, "total": len(rows),
                            "next_cursor": f"{start + 50}:{scope}" if start + 50 < len(rows) else None,
                            "totals_scope": "workspace", "finance": {"receivable_cents": 12300, "overdue_cents": 12300, "prepared_payable_cents": 12300},
                            "bills": [], "settlements": [], "outbox": [], "inbox": []}
                    for direction, row in selected:
                        data[direction].append(row)
                raw = json.dumps(data).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(raw)))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(raw)
            except Exception as error:
                failures.append(str(error) or type(error).__name__)
                self.send_error(422)

        def do_POST(self):
            failures.append("unexpected write")
            self.send_error(405)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    directory = Path(tempfile.mkdtemp(prefix="freight-tui-catalog-pty-"))
    os.chmod(directory, 0o700)
    master, slave = os.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 100, 0, 0))
    output = bytearray()
    proc = None

    def attach():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    def until(needle, start=0):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            if needle in output[start:]:
                return
            assert proc.poll() is None, "TUI exited early"
            if select.select([master], [], [], 0.05)[0]:
                chunk = os.read(master, 65536)
                output.extend(chunk)
                if b"\x1b[6n" in chunk:
                    os.write(master, b"\x1b[1;1R")
        raise AssertionError("missing terminal marker " + repr(needle))

    def send(keys, marker):
        start = len(output)
        os.write(master, keys)
        until(marker, start)
        time.sleep(0.12)

    try:
        proc = subprocess.Popen([str(Path(binary).resolve()), "--api", f"http://127.0.0.1:{server.server_port}", "--view", "billing"],
                                stdin=slave, stdout=slave, stderr=slave, cwd=directory, preexec_fn=attach,
                                env={"TERM": "xterm-ghostty", "COLORTERM": "truecolor", "FREIGHT_TUI_TOKEN": token, "TOKIO_WORKER_THREADS": "2"})
        until(b"INV-000")
        send(b"n", b"INV-050")
        send(b"p", b"INV-000")
        send(b"/\x1b[200~INV-122\x1b[201~\r", b"INV-122")
        # Wait for the matching row read, not just the search draft text.
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not any(r["query"].get("q") == ["INV-122"] for r in requests):
            time.sleep(0.03)
        assert any(r["query"].get("q") == ["INV-122"] for r in requests)
        time.sleep(0.3)
        send(b"f", b"Open balances")
        time.sleep(0.3)
        send(b"\x7f", b"INV-000")
        send(b"4", b"PAY-000")
        send(b"n", b"PAY-050")
        send(b"p", b"PAY-000")
        send(b"5", b"OUT-000")
        send(b"n", b"IN-000")
        send(b"d", b"OUT-000")
        send(b"d", b"IN-000")
        send(b"f", b"IN-060")
        os.write(master, b"q")
        proc.wait(timeout=5)
        assert proc.returncode == 0
        assert termios.tcgetattr(slave) == original, "terminal modes not restored"
        assert not failures, failures
        assert token.encode() not in output, "credential rendered"
        reads = [r for r in requests if r["path"] == "/v1/console"]
        assert {r["query"]["view"][0] for r in reads} == {"billing", "payables", "integrations"}
        for view in ["billing", "payables", "integrations"]:
            assert any(r["query"]["view"] == [view] and "cursor" in r["query"] for r in reads)
        assert any(r["query"].get("balance") == ["open"] and "cursor" not in r["query"] for r in reads)
        assert any(r["query"].get("direction") == ["inbound"] and r["query"].get("attention") == ["true"] and "cursor" not in r["query"] for r in reads)
        print("PASS owned HTTP/Ghostty catalog paging: Billing, Payables, mixed Queues, global search, filter resets, terminal restoration; " + str(len(reads)) + " bounded reads")
    finally:
        if proc is not None and proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)
        os.close(master)
        os.close(slave)
        (directory / "terminal.log").write_bytes(output)
        (directory / "requests.json").write_text(json.dumps({"requests": requests, "failures": failures}, indent=2) + "\n")
        print("Owned catalog diagnostics:", directory)


if __name__ == "__main__":
    exercise(sys.argv[1])
