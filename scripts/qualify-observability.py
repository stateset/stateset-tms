#!/usr/bin/env python3
"""Exercise an owned Freight process on loopback with synthetic, ephemeral keys."""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    require(args.binary.is_file(), "Build the API binary first")
    require(not args.output.exists(), "Choose a new evidence output path")
    binary = args.binary.resolve()
    directory = Path(tempfile.mkdtemp(prefix="freight-observability-"))
    (directory / ".env").write_text("# Owned empty configuration.\n")
    report = {
        "scope": "Owned loopback API process with memory/demo storage; not database, container or production qualification",
        "started_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "artifacts": str(directory),
        "checks": [],
        "result": "running",
    }
    token, admin = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
    # Do not inherit database credentials, identity configuration, integrations,
    # proxy settings or ambient dotenv files into a synthetic qualification.
    base = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "LANG": "C.UTF-8",
        "FREIGHT_DEMO": "true",
        "FREIGHT_API_KEYS_JSON": json.dumps({admin: "demo"}),
        "FREIGHT_INTEGRATIONS_JSON": "{}",
        "FREIGHT_SHUTDOWN_TIMEOUT_MS": "3000",
        "TOKIO_WORKER_THREADS": "2",
        "RUST_LOG": "stateset_freight=info,tower_http=info",
    }
    process = None
    log = None
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def check(name, condition):
        require(condition, name)
        report["checks"].append({"name": name, "result": "pass"})

    def start(name, metrics_token):
        nonlocal process, log
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        log = (directory / (name + ".log")).open("x")
        env = dict(base, FREIGHT_BIND_ADDR=f"127.0.0.1:{port}", FREIGHT_METRICS_TOKEN=metrics_token)
        process = subprocess.Popen([str(binary)], cwd=directory, env=env, stdout=log, stderr=subprocess.STDOUT)
        return f"http://127.0.0.1:{port}"

    def request(url, path, credential=None):
        headers = {"Authorization": f"Bearer {credential}"} if credential else {}
        req = urllib.request.Request(url + path, headers=headers)
        try:
            response = opener.open(req, timeout=2)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            return response.status, response.headers, response.read()

    def ready(url):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            require(process.poll() is None, "API exited before readiness")
            try:
                if request(url, "/ready")[0] == 200:
                    return
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(0.05)
        raise AssertionError("API readiness deadline exceeded")

    def stop():
        process.send_signal(signal.SIGTERM)
        check("SIGTERM drains the owned process with exit zero", process.wait(timeout=8) == 0)
        log.close()

    try:
        url = start("enabled", token)
        ready(url)
        for path in ["/live", "/ready", "/health"]:
            status, headers, body = request(url, path)
            check(path + " returns uncached generic service health", status == 200 and headers.get("Cache-Control") == "no-store" and json.loads(body) == {"status": "ok", "service": "stateset-freight"})
        for credential in [None, admin]:
            status, headers, body = request(url, "/metrics", credential)
            check("metrics refuses missing or business credentials", status == 401 and not body and headers.get("Cache-Control") == "no-store")
        check("monitor credential cannot authorize business access", request(url, "/v1/access", token)[0] == 401)
        check("business credential still authorizes access", request(url, "/v1/access", admin)[0] == 200)
        status, headers, body = request(url, "/metrics", token)
        text = body.decode()
        check("dedicated credential receives a Prometheus scrape", status == 200 and headers.get("Content-Type") == "text/plain; version=0.0.4; charset=utf-8")
        check("scrape counts the actual success and denial", all(f'freight_http_requests_total{{route="/v1/access",method="GET",outcome="{kind}"}} 1\n' in text for kind in ["2xx", "4xx"]))
        check("scrape excludes probes and secrets", not any(value in text for value in [token, admin, 'route="/live"', 'route="/ready"', 'route="/health"', 'route="/metrics"']))
        (directory / "metrics.prom").write_bytes(body)
        stop()

        url = start("disabled", "")
        ready(url)
        status, headers, _ = request(url, "/metrics", token)
        check("disabled metrics returns uncached 404", status == 404 and headers.get("Cache-Control") == "no-store")
        stop()

        for name, configured, message in [
            ("invalid", "invalid-private-token", "FREIGHT_METRICS_TOKEN must contain"),
            ("reused", admin, "FREIGHT_METRICS_TOKEN must be distinct"),
        ]:
            start(name, configured)
            status = process.wait(timeout=15)
            log.close()
            output = (directory / (name + ".log")).read_text()
            check(name + " metrics configuration fails startup without revealing credentials", status != 0 and message in output and configured not in output)
        report["result"] = "pass"
    except Exception as error:
        report["result"] = "failed"
        report["error"] = str(error).replace(token, "[redacted]").replace(admin, "[redacted]")
        raise
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        if log is not None:
            log.close()
        report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        report["artifact_sha256"] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in directory.iterdir() if p.is_file()}
        with args.output.open("x") as output:
            output.write(json.dumps(report, indent=2) + "\n")
    print(f"PASS {len(report['checks'])} actual-process observations; {args.output}")


if __name__ == "__main__":
    main()
