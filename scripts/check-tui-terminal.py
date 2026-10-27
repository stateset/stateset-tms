#!/usr/bin/env python3
"""Exercise the real terminal backend under Ghostty's TERM, with owned PTYs."""
import fcntl
import os
from pathlib import Path
import select
import signal
import struct
import subprocess
import sys
import termios
import tempfile
import time


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def exercise(binary, exit_mode):
    master, slave = os.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 77 if exit_mode in ["compact", "navigation", "inspection", "search", "related", "queues", "finance", "palette", "operations", "read-refresh", "page-positions", "parties", "work-actions"] else 140, 0, 0))

    def attach():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    env = dict(os.environ, TERM="xterm-ghostty", COLORTERM="truecolor")
    proc = None
    output = bytearray()
    try:
        setup = exit_mode.startswith("setup")
        if setup:
            for name in ["FREIGHT_TUI_TOKEN", "FREIGHT_TUI_RECOVERY_KEY", "FREIGHT_TUI_ORGANIZATION_ID"]:
                env.pop(name, None)
            env["FREIGHT_TUI_API_URL"] = "http://127.0.0.1:8090"
        proc = subprocess.Popen([str(binary), "--connect" if setup else "--demo"], stdin=slave, stdout=slave,
                                stderr=slave, env=env, preexec_fn=attach)

        def until(needle, started=None):
            started = len(output) if started is None else started
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if needle in output[started:]:
                    return
                if select.select([master], [], [], 0.1)[0]:
                    chunk = os.read(master, 65536)
                    output.extend(chunk)
                    if b"\x1b[6n" in chunk:
                        os.write(master, b"\x1b[1;1R")
                    if needle in output[started:]:
                        return
                require(proc.poll() is None, "console exited before rendering expected content")
            raise AssertionError("terminal did not render: " + repr(needle))

        if setup:
            until(b"Connection setup")
            if exit_mode == "setup-sigterm":
                os.kill(proc.pid, signal.SIGTERM)
            else:
                secret = b"PTY-PRIVATE-CREDENTIAL-CANARY"
                recovery = b"a1" * 32
                os.write(master, b"\r\x1b[200~" + secret + b"\x1b[201~\r\x1b[200~" + recovery + b"\x1b[201~\r")
                until(b"y connects")
                require(secret not in output and recovery not in output, "setup exposed a secret")
                os.write(master, b"\x1b")
        else:
            until(b"FRT-10421")
        if setup:
            pass
        elif exit_mode == "compact":
            until(b"Phoenix", 0)
            start = len(output)
            os.write(master, b"3")
            until(b"1875.00", start)
            require(b"2875.00" in output[start:], "compact billing hides invoice amount")
            start = len(output)
            os.write(master, b"4")
            until(b"1400.00", start)
            require(b"2400.00" in output[start:], "compact payables hides settlement total")
            os.write(master, b"i")
            until(b"Connection")
            os.write(master, b"\x1b")
            time.sleep(0.15)
            os.write(master, b"q")
        elif exit_mode == "navigation":
            start = len(output)
            os.write(master, b"n")
            until(b"FRT-10471", start)
            os.write(master, b"jjj\r")
            until(b"Details", start)
            # The identity is below the initial compact detail viewport.
            os.write(master, b"\x1b[F")
            # Ratatui may reuse letters already on screen when painting the ID.
            # Its unique numeric suffix identifies this selected load in the diff.
            until(b"0053", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"3")
            until(b"INV-2026-1042", start)
            start = len(output)
            os.write(master, b"1")
            until(b"FRT-10471", start)
            start = len(output)
            os.write(master, b"\r")
            until(b"Details", start)
            os.write(master, b"\x1b[F")
            until(b"0053", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            os.write(master, b"q")
        elif exit_mode == "inspection":
            start = len(output)
            os.write(master, b"\r")
            until(b"Details", start)
            start = len(output)
            os.write(master, b"\x1b[F")
            until(b"0000", start)
            start = len(output)
            os.write(master, b"\x1b[C\x1b[F")
            until(b"0001", start)
            start = len(output)
            os.write(master, b"\x1b[H\x1b[6~\x1b[F")
            until(b"0001", start)
            start = len(output)
            os.write(master, b"\x1b[D\x1b[F")
            until(b"0000", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            os.write(master, b"q")
        elif exit_mode == "search":
            start = len(output)
            os.write(master, b"/\x1b[200~DaXllas\x1b[201~\x1b[H\x1b[C\x1b[C\x1b[3~\r")
            until(b"FRT-10422", start)
            start = len(output)
            os.write(master, b"/\x15\x1b[200~NO-MATCH-PTY-CANARY\x1b[201~\r")
            until(b"No matching", start)
            start = len(output)
            os.write(master, b"\x7f")
            until(b"FRT-10421", start)
            os.write(master, b"q")
        elif exit_mode == "related":
            start = len(output)
            os.write(master, b"2")
            # Wait for a real row; the source-deadline header exists before data arrives.
            until(b"demo:dispatcher", start)
            start = len(output)
            os.write(master, b"j\r\x1b[F")
            until(b"issue-demo-1", start)
            start = len(output)
            os.write(master, b"o")
            until(b"FRT-10422", start)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"0001", start)
            start = len(output)
            os.write(master, b"b")
            until(b"demo:dispatcher", start)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"issue-demo-1", start)
            os.write(master, b"q")
        elif exit_mode == "queues":
            start = len(output)
            os.write(master, b"5")
            until(b"callback", start)
            start = len(output)
            os.write(master, b"fd\r\x1b[F")
            until(b"out-demo-1", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"d")
            until(b"No matching", start)
            start = len(output)
            os.write(master, b"\x7f")
            until(b"callback", start)
            start = len(output)
            os.write(master, b"dd\r\x1b[F")
            until(b"in-demo-1", start)
            os.write(master, b"q")
        elif exit_mode == "finance":
            start = len(output)
            os.write(master, b"3")
            until(b"INV-2026-1042", start)
            start = len(output)
            os.write(master, b"ff\r\x1b[F")
            until(b"bill-demo-2", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            # The third billing filter is refunds due; both demo invoices are open.
            os.write(master, b"f")
            until(b"No matching", start)
            start = len(output)
            os.write(master, b"\x7f")
            until(b"INV-2026-1042", start)
            start = len(output)
            os.write(master, b"4")
            until(b"1400.00", start)
            start = len(output)
            # Payables skips aging: the second filter shows carrier credits.
            os.write(master, b"ff\r\x1b[F")
            until(b"settle-demo-3", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"3")
            until(b"INV-2026-1042", start)
            start = len(output)
            os.write(master, b"4")
            # The remembered filter renders before the fresh rows arrive.
            until(b"-125.00", start)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"settle-demo-3", start)
            os.write(master, b"q")
        elif exit_mode == "palette":
            start = len(output)
            os.write(master, b"\x10")
            until(b"Commands", start)
            start = len(output)
            os.write(master, b"\x1b[200~status transit\x1b[201~\r")
            until(b"in_transit", start)
            # Wait for actual filtered rows before reopening the palette.
            until(b"FRT-10421", start)
            start = len(output)
            os.write(master, b"\x10\x1b[200~go invoice\x1b[201~\r")
            until(b"INV-2026-1042", start)
            start = len(output)
            os.write(master, b"\x10\x1b[200~balance overdue\x1b[201~\r\r\x1b[F")
            until(b"bill-demo-2", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"\x10\x1b[200~qacxy-no-such-command\x1b[201~\r")
            until(b"No commands match", start)
            require(proc.poll() is None, "palette paste triggered a shortcut")
            os.write(master, b"\x1b")
            time.sleep(0.15)
            os.write(master, b"q")
        elif exit_mode == "operations":
            start = len(output)
            os.write(master, b"2")
            until(b"demo:dispatcher", start)
            start = len(output)
            os.write(master, b"ff")
            until(b"demo:terminal", start)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"issue-demo-2", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"f")
            until(b"FRT-10422", start)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"issue-demo-1", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"\x10\x1b[200~work deferred\x1b[201~\r")
            until(b"demo:terminal", start)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"issue-demo-3", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"\x7f")
            until(b"demo:dispatcher", start)
            os.write(master, b"q")
        elif exit_mode == "read-refresh":
            start = len(output)
            os.write(master, b"n")
            until(b"FRT-10471", start)
            start = len(output)
            os.write(master, b"jjj\x12")
            time.sleep(0.3)
            os.write(master, b"\r\x1b[F")
            until(b"0053", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"i\x1b[F")
            # Instruction text can reuse unchanged cells in ratatui's diff.
            # This field uniquely identifies the newly opened status panel.
            until(b"Read target:", start)
            os.write(master, b"\x12")
            time.sleep(0.3)
            start = len(output)
            os.write(master, b"R\x1b[H")
            until(b"Every 30", start)
            start = len(output)
            os.write(master, b"r")
            # Stay in the same panel until the explicit read has completed.
            time.sleep(0.3)
            start = len(output)
            os.write(master, b"\r\r\x1b[F")
            until(b"0000", start)
            os.write(master, b"q")
        elif exit_mode == "page-positions":
            start = len(output)
            os.write(master, b"\x1b[Fn")
            until(b"FRT-10471", start)
            os.write(master, b"jjj")
            start = len(output)
            os.write(master, b"p")
            until(b"FRT-10470", start)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"0049", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"\x10\x1b[200~next page\x1b[201~\r")
            until(b"FRT-10474", start)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"0053", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            os.write(master, b"q")
        elif exit_mode == "parties":
            os.write(master, b"\x10\x1b[200~selected customer\x1b[201~\r")
            time.sleep(0.3)
            start = len(output)
            os.write(master, b"G\r\x1b[F")
            until(b"0064", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            os.write(master, b"\x10\x1b[200~selected carrier\x1b[201~\r")
            time.sleep(0.3)
            start = len(output)
            os.write(master, b"i")
            until(b"Customer filter:", start)
            # Ratatui reuses matching cells from the table: the carrier label
            # may be split into several output runs. Its newly displayed full
            # value identifies this panel; component tests assert both labels.
            until(b"Pioneer Transport (sample)", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            os.write(master, b"\x10\x1b[200~clear customer filter\x1b[201~\r")
            time.sleep(0.3)
            start = len(output)
            os.write(master, b"n")
            until(b"FRT-10471", start)
            os.write(master, b"\x10\x1b[200~clear carrier filter\x1b[201~\r")
            time.sleep(0.3)
            start = len(output)
            os.write(master, b"G\r\x1b[F")
            until(b"0049", start)
            os.write(master, b"q")
        elif exit_mode == "work-actions":
            start = len(output)
            os.write(master, b"2")
            until(b"demo:dispatcher", start)
            os.write(master, b"ja1")
            until(b"RFC3339")
            os.write(master, b"\x1b[200~2h\x1b[201~\r")
            os.write(master, b"\x1b[200~Ghostty owned work claim\x1b[201~\r")
            until(b"y confirms")
            start = len(output)
            os.write(master, b"y")
            until(b"Save", start)
            time.sleep(0.3)
            start = len(output)
            os.write(master, b"ff")
            until(b"demo:terminal", start)
            # The owner also appears in the post-save Actionable response;
            # let the queued Mine read finish before opening its first row.
            time.sleep(0.3)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"issue-demo-1", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            os.write(master, b"a2\x1b[200~Ghostty released work\x1b[201~\r")
            until(b"y confirms")
            start = len(output)
            os.write(master, b"y")
            time.sleep(0.4)
            start = len(output)
            os.write(master, b"\r\x1b[F")
            until(b"issue-demo-2", start)
            os.write(master, b"\x1b")
            time.sleep(0.15)
            start = len(output)
            os.write(master, b"\x7f")
            until(b"demo:dispatcher", start)
            os.write(master, b"q")
        elif exit_mode == "actions":
            os.write(master, b"a")
            until(b"Reviewed")
            os.write(master, b"1")
            until(b"Actual")
            os.write(master, b"\x1b[200~Ghostty-TUI-check\x1b[201~\r")
            until(b"confirms")
            os.write(master, b"y")
            # Screen diffs can reuse individual letters from "Sending";
            # the subsequent fresh-detail check verifies the saved value.
            until(b"Save")
            # Confirmed writes trigger an explicit read of the simulated store.
            # Wait for that queued read before opening the fresh row details.
            time.sleep(0.4)
            os.write(master, b"\r")
            until(b"Ghostty-TUI-check")
            os.write(master, b"\x1b")
            time.sleep(0.15)
            os.write(master, b"q")
        elif exit_mode == "keyboard":
            os.write(master, b"5")
            until(b"callback")
            os.write(master, b"3")
            until(b"INV-2026-1042")
            os.write(master, b"\x1b[200~q\x1b[201~")
            time.sleep(0.15)
            require(proc.poll() is None, "bracketed paste triggered the quit shortcut")
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 10, 40, 0, 0))
            os.kill(proc.pid, signal.SIGWINCH)
            until(b"Resize")
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
            os.kill(proc.pid, signal.SIGWINCH)
            until(b"INV-2026-1042")
            os.write(master, b"q")
        elif exit_mode == "ctrl-c":
            os.write(master, b"\x03")
        else:
            os.kill(proc.pid, signal.SIGTERM)
        require(proc.wait(timeout=5) == (1 if setup else 0), "console exit was not clean")
        while select.select([master], [], [], 0.1)[0]:
            output.extend(os.read(master, 65536))
        require(termios.tcgetattr(slave) == original, "terminal flags were not restored")
        for sequence in [b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?2004h", b"\x1b[?2004l"]:
            require(sequence in output, "missing terminal lifecycle sequence " + repr(sequence))
        print("PASS Ghostty TERM PTY:", exit_mode)
    except BaseException:
        with tempfile.NamedTemporaryFile(prefix="freight-tui-pty-failure-", suffix=".txt", delete=False) as trace:
            trace.write(output)
            print("PTY failure trace:", trace.name)
        raise
    finally:
        if proc and proc.poll() is None:
            proc.kill()
            proc.wait()
        os.close(master)
        os.close(slave)


def hangup(binary, graphics, leader):
    """An idle console must exit promptly when its terminal hangs up.

    crossterm retries end-of-file inside its input poll, so without the
    watchdog the console spun at full CPU and ignored SIGTERM/SIGHUP.
    """
    master, slave = os.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))

    def attach():
        os.setsid()
        if leader:
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    env = dict(os.environ, TERM="xterm-ghostty", COLORTERM="truecolor")
    proc = subprocess.Popen([str(binary), "--demo", "--graphics", graphics], stdin=slave, stdout=slave,
                            stderr=slave, env=env, preexec_fn=attach)
    os.close(slave)
    output = bytearray()
    try:
        deadline = time.monotonic() + 10
        while b"FRT-10421" not in output and time.monotonic() < deadline:
            require(proc.poll() is None, "console exited before rendering")
            if select.select([master], [], [], 0.1)[0]:
                chunk = os.read(master, 65536)
                output.extend(chunk)
                if b"\x1b[6n" in chunk:
                    os.write(master, b"\x1b[1;1R")
        require(b"FRT-10421" in output, "console did not render before hangup")
        time.sleep(0.5)
    finally:
        os.close(master)
    try:
        status = proc.wait(timeout=4)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        raise AssertionError("console kept running after terminal hangup: " + graphics)
    require(status == 1, "hung-up console did not exit cleanly: %s status %s" % (graphics, status))
    print("PASS terminal hangup exit:", graphics, "session leader" if leader else "without SIGHUP")


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/freight-tui").resolve()
    for graphics in ["auto", "kitty", "kitty-tmux", "off"]:
        for leader in [True, False]:
            hangup(binary, graphics, leader)
    for exit_mode in ["keyboard", "compact", "navigation", "inspection", "search", "related", "queues", "finance", "palette", "operations", "read-refresh", "page-positions", "parties", "work-actions", "actions", "setup", "setup-sigterm", "ctrl-c", "sigterm"]:
        exercise(binary, exit_mode)
    snapshot = subprocess.run([str(binary), "--demo", "--snapshot"], capture_output=True, timeout=10)
    require(snapshot.returncode == 0 and b"SYNTHETIC DEMO" in snapshot.stdout and b"FRT-10421" in snapshot.stdout,
            "plain snapshot failed")
    require(b"\x1b" not in snapshot.stdout, "plain snapshot contains terminal escapes")
    piped = subprocess.run([str(binary), "--demo"], capture_output=True, timeout=10)
    require(piped.returncode == 1 and b"needs a terminal" in piped.stderr, "non-TTY guard failed")
    print("PASS plain snapshot and non-TTY guard")


if __name__ == "__main__":
    main()
