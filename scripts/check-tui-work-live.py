#!/usr/bin/env python3
"""Owned API/TUI process acceptance: all work writes are sent by the real TUI."""
import fcntl
import json
import os
from pathlib import Path
import select
import struct
import subprocess
import sys
import termios
import time
import urllib.parse
import urllib.request

from tui_terminal import Screen
from tui_fixture_proxy import DelayedReads


def main():
    binary, origin, directory = sys.argv[1:]
    root = Path(directory)
    token = os.environ["FREIGHT_TUI_TOKEN"]
    delay_ms = int(os.environ.get("FREIGHT_TUI_TEST_READ_DELAY_MS", "0"))
    relay = DelayedReads(origin, delay_ms) if delay_ms else None
    if relay is not None:
        origin = relay.origin
    recovery = root / "work-recovery"
    recovery.mkdir(mode=0o700)

    def get(path):
        request = urllib.request.Request(origin + path, headers={"Authorization": "Bearer " + token})
        with urllib.request.urlopen(request, timeout=5) as response:
            return json.load(response)

    candidates = get("/v1/operations?view=unowned&limit=50")["issues"]
    row = None
    for candidate in candidates:
        if candidate["signal"]["priority"] > 0:
            query = candidate["signal"]["title"]
            matches = get("/v1/operations?view=unowned&limit=50&search=" + urllib.parse.quote(query))["issues"]
            if matches and matches[0]["signal"]["priority"] > 0:
                row = matches[0]
                break
    assert row is not None, "owned API must have noncritical unowned work"
    issue = row["id"]
    before = get("/v1/workspace?history=deferred")
    identity = get("/v1/access")
    master, slave = os.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 44, 140, 0, 0))
    output = bytearray()
    screen = Screen(44, 140)
    started = time.monotonic()
    timeline = []

    def attach():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    env = dict(os.environ, TERM="xterm-ghostty", FREIGHT_TUI_RECOVERY_KEY="43" * 32,
               FREIGHT_TUI_RECOVERY_DIR=str(recovery))
    proc = subprocess.Popen([binary, "--api", origin, "--view", "operations", "--operations-scope", "unowned", "--no-color"],
                            stdin=slave, stdout=slave, stderr=slave, env=env, preexec_fn=attach)

    def pump(duration):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            if select.select([master], [], [], min(.05, max(0, deadline - time.monotonic())))[0]:
                chunk = os.read(master, 65536)
                output.extend(chunk)
                screen.feed(chunk)
                if b"\x1b[6n" in chunk:
                    os.write(master, b"\x1b[1;1R")
            assert proc.poll() is None, "TUI exited during owned workflow"

    def wait(predicate, message):
        phase_started = time.monotonic()
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            pump(.05)
            if predicate():
                timeline.append({"phase": message, "elapsed_ms": round((time.monotonic() - phase_started) * 1000),
                                 "since_start_ms": round((time.monotonic() - started) * 1000)})
                return
        timeline.append({"phase": message, "failed": True,
                         "elapsed_ms": round((time.monotonic() - phase_started) * 1000)})
        raise AssertionError("Timed out waiting for " + message + " (retained work-timeline.json and current work-screen.txt)")

    def ready():
        current = screen.text
        return (" shown" in current and not any(text in current for text in
                ["Search:", "Loading", "Refreshing", "Sending original request", "Reviewed work action", "Commands"]))

    def visible(text):
        wait(lambda: text in screen.text, "rendered " + text)

    def paste(text):
        os.write(master, b"\x1b[200~" + text.encode() + b"\x1b[201~")

    try:
        wait(ready, "verified initial read")
        os.write(master, b"/")
        paste(query)
        os.write(master, b"\r")
        wait(lambda: ready() and "[Unowned] / " + query in screen.text, "filtered source read complete")
        current=get("/v1/operations/issues/"+issue)
        version=current["case"]["version"] if current["case"] else 0
        notes = []
        for number, action, timing in [("1", "claim", "2h"), ("3", "note", None), ("4", "note", "3h"),
                                       ("5", "snooze", "15m"), ("6", "resume", None), ("2", "release", None)]:
            wait(ready, "fresh read before work action " + number)
            os.write(master, b"a")
            visible("Choose 1–6")
            os.write(master, number.encode())
            if timing:
                visible("Enter pins the absolute UTC time")
                paste(timing)
                os.write(master, b"\r")
            visible("Enter reviews")
            note = "Owned live TUI work " + number
            paste(note)
            os.write(master, b"\r")
            visible("y confirms and saves")
            assert note in screen.text, "rendered review must contain the exact note"
            unchanged = get("/v1/operations/issues/" + issue)
            assert (unchanged["case"]["version"] if unchanged["case"] else 0) == version, "review must not write"
            os.write(master, b"y")
            expected = version + (2 if version == 0 else 1)

            def confirmed():
                case = get("/v1/operations/issues/" + issue)["case"]
                return case and case["version"] == expected and not list(recovery.glob("*.sealed"))

            wait(confirmed, "confirmed reviewed work action " + number)
            wait(ready, "fresh read after work action " + number)
            current = get("/v1/operations/issues/" + issue)
            case = current["case"]
            assert case["history"][-1]["action"] == action
            assert case["history"][-1]["actor"] == identity["access"]["subject"]
            assert case["history"][-1]["note"] == note
            assert case["source_sha256"] == row["source_sha256"]
            assert case["owner"] == (None if action == "release" else identity["access"]["subject"])
            if action == "snooze":
                assert current["deferred"] is True
            if action in ("resume", "release"):
                assert case["deferred_until"] is None
            version = expected
            notes.append(note)
            if action == "claim":
                os.write(master, b"\x10")
                visible("Commands")
                paste("work mine")
                os.write(master, b"\r")
                wait(lambda: ready() and "Assigned to me" in screen.text, "owned work scope read complete")
        after = get("/v1/workspace?history=deferred")
        for key in ["loads", "bills", "settlements", "invoices"]:
            assert before.get(key) == after.get(key), "work changed shipment or financial truth: " + key
        history = get("/v1/operations/issues/" + issue)["case"]["history"]
        assert [event["note"] for event in history[-6:]] == notes
        os.write(master, b"q")
        proc.wait(timeout=5)
        assert proc.returncode == 0
        restored = termios.tcgetattr(slave)
        for flag in [termios.ICANON, termios.ECHO, termios.ISIG]:
            assert restored[3] & flag == original[3] & flag, "terminal flags not restored"
        assert token.encode() not in output and ("43" * 32).encode() not in output
        (root / "work-pty.log").write_bytes(output)
        print("PASS owned API/TUI work loop: claim, note, follow-up, defer, resume, release; fresh reads and encrypted cleanup; shipment/finance truth unchanged")
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=5)
        os.close(master)
        os.close(slave)
        # Diagnostics stay within the owned, private process test directory.
        assert token.encode() not in output and ("43" * 32).encode() not in output
        (root / "work-pty.log").write_bytes(output)
        (root / "work-screen.txt").write_text(screen.text)
        (root / "work-timeline.json").write_text(json.dumps(timeline, indent=2) + "\n")
        if relay is not None:
            relay.close()
            (root / "read-delay.json").write_text(json.dumps({"delay_ms": delay_ms, "delayed_reads": relay.delayed}) + "\n")
            assert relay.delayed > 0, "slow-read fixture must observe filtered TUI reads"


if __name__ == "__main__":
    main()
