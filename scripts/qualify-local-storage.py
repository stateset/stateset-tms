#!/usr/bin/env python3
"""Native HTTP/storage observations on an owned local schema.

Uses the supplied PostgreSQL installation without Docker. Each run creates a
unique schema, retains exact original snapshots and responses, and drops only
that schema. The harness imposes no resource quotas and does not qualify
sustained capacity or production RPO/RTO. PostgreSQL's actual version is recorded.
"""
import argparse
import datetime as dt
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import socket
import statistics
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request
import uuid

from runtime_contracts import literal, require

spec = importlib.util.spec_from_file_location("exact_originals", Path(__file__).with_name("profile-storage-records.py"))
exact = importlib.util.module_from_spec(spec)
spec.loader.exec_module(exact)


def binding(path):
    path = path.resolve()
    return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def cpu(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database-url", required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--codec", type=Path, required=True)
    parser.add_argument("--input", type=Path, action="append", required=True)
    parser.add_argument("--samples", type=int, choices=range(3, 21), default=5)
    parser.add_argument("--diagnostic", action="store_true", help="One candidate run per input; no speed comparison")
    parser.add_argument("--profile-writes", action="store_true", help="Enable write phase diagnostics; retain logging overhead")
    parser.add_argument("--invoice-review-projection", action="store_true", help="Validate and separately retain the candidate's additive invoice review controls while comparing unchanged business fields")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(not args.output.exists(), "Preserve prior reports; select a new output path")
    base = urllib.parse.urlsplit(args.database_url)
    require(base.scheme in {"postgres", "postgresql"} and base.hostname in {"localhost", "127.0.0.1", "::1"}, "Only an explicitly supplied local PostgreSQL database is supported")
    require(not urllib.parse.parse_qs(base.query).get("options"), "Supply a base database without search_path options")
    directory = Path(tempfile.mkdtemp(prefix="freight-local-storage-"))
    harness_archive = directory / "harness.py"
    harness_archive.write_bytes(Path(__file__).read_bytes())
    binaries = {"baseline": args.baseline.resolve(), "candidate": args.candidate.resolve()}
    report = {"result": "running", "scope": __doc__, "artifacts": str(directory),
              "started_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
              "harness": binding(Path(__file__)), "exact_json_helper": binding(Path(__file__).with_name("profile-storage-records.py")),
              "harness_archive": binding(harness_archive),
              "binaries": {k: binding(v) for k, v in binaries.items()}, "codec": binding(args.codec),
              "inputs": [binding(p) for p in args.input], "timed_samples": args.samples,
              "host_at_start": {"logical_cpus": os.cpu_count(), "load_average": os.getloadavg()},
              "order": ["candidate"] if args.diagnostic else ["baseline", "candidate", "candidate", "baseline"],
              "runtime_helpers": binding(Path(__file__).with_name("runtime_contracts.py")),
              "single_candidate_diagnostic": args.diagnostic, "write_phase_logging": args.profile_writes,
              "additive_invoice_projection_validation": args.invoice_review_projection, "runs": []}

    def sql(url, query):
        completed = subprocess.run(["psql", url, "-X", "-qAt", "-v", "ON_ERROR_STOP=1"],
                                   input=query.encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if completed.returncode:
            # CalledProcessError includes command arguments, including the
            # connection credential. Keep that URL out of the public report.
            archive("database-failure-" + uuid.uuid4().hex + ".stderr", completed.stderr)
            raise RuntimeError("Owned PostgreSQL command failed; inspect retained database diagnostics")
        return completed.stdout.decode().strip()

    def archive(name, raw):
        path = directory / name
        with path.open("xb") as out:
            out.write(raw)
        return binding(path)

    try:
        report["postgresql_version"] = sql(args.database_url, "SELECT version()")
        for input_index, source in enumerate(args.input):
            original = exact.decode(source.read_bytes())
            require(original.get("_freight_format") in {19, 20, 21}, "Supply a complete format-19, format-20 or format-21 workspace")
            identity = original["loads"][0]["id"]
            read_references = {}
            for order, variant in enumerate(report["order"]):
                prefix = f"{input_index}-{order}-{variant}"
                schema = "freight_profile_" + uuid.uuid4().hex
                sql(args.database_url, f"CREATE SCHEMA {schema}")
                url = urllib.parse.urlunsplit(base._replace(query=urllib.parse.urlencode([*urllib.parse.parse_qsl(base.query), ("options", f"-csearch_path={schema},pg_catalog")])))
                process = None
                token = secrets.token_urlsafe(40)
                run = {"input": input_index, "order": order, "variant": variant, "reads": {}, "saves": []}
                report["runs"].append(run)
                try:
                    with socket.socket() as listener:
                        listener.bind(("127.0.0.1", 0))
                        port = listener.getsockname()[1]
                    environment = {**os.environ, "DATABASE_URL": url, "FREIGHT_DEMO": "false",
                        "FREIGHT_API_KEYS_JSON": json.dumps({token: {"tenant_id": "large", "subject": "profile:operator", "role": "admin"}}), "FREIGHT_CLERK_JSON": "",
                        "FREIGHT_INTEGRATIONS_JSON": "{}", "FREIGHT_BIND_ADDR": f"127.0.0.1:{port}",
                        "TOKIO_WORKER_THREADS": "2", "FREIGHT_DATABASE_MAX_CONNECTIONS": "2",
                        "FREIGHT_READ_CONCURRENCY": "2", "RUST_LOG": "error,stateset_freight::write_profile=debug" if args.profile_writes else "error"}
                    with (directory / (prefix + ".log")).open("xb") as log:
                        process = subprocess.Popen([str(binaries[variant])], cwd=directory, env=environment, stdout=log, stderr=subprocess.STDOUT)
                    endpoint = f"http://127.0.0.1:{port}"

                    def request(path, name, body=None, key=None):
                        headers = {"Authorization": "Bearer " + token}
                        data = None
                        if body is not None:
                            headers.update({"Content-Type": "application/json", "Idempotency-Key": key})
                            data = json.dumps(body, separators=(",", ":")).encode()
                        before_cpu, started = cpu(process.pid), time.monotonic()
                        with urllib.request.urlopen(urllib.request.Request(endpoint + path, data=data, headers=headers), timeout=120) as response:
                            raw = response.read()
                            require(response.status == 200, "Unexpected HTTP status")
                        return {"wall_ms": (time.monotonic() - started) * 1000,
                                "api_cpu_seconds": cpu(process.pid) - before_cpu,
                                "response": archive(name, raw)}, raw

                    deadline = time.monotonic() + 60
                    while True:
                        require(process.poll() is None, "API exited; inspect " + prefix + ".log")
                        try:
                            with urllib.request.urlopen(endpoint + "/ready", timeout=1) as response:
                                if response.status == 200:
                                    break
                        except (OSError, TimeoutError):
                            require(time.monotonic() < deadline, "Readiness deadline exceeded")
                            time.sleep(0.05)
                    storage_format = int(sql(url, "SELECT version FROM freight_storage_format WHERE singleton"))
                    require(storage_format in {19, 20, 21}, "API initialized an unsupported storage format")
                    # Cross-release comparisons permit exactly the explicit
                    # compatibility marker change, with all other originals exact.
                    expected_original = {**original, "_freight_format": storage_format}
                    frame = exact.decode(subprocess.check_output([str(args.codec.resolve()), "pack", "64", "65536"], input=exact.encode(expected_original).encode()))
                    blocks = frame.pop("blocks")
                    frame["blocks"] = []
                    header = exact.encode({"_freight_format": storage_format, "frame": frame})
                    run["storage_format"] = storage_format
                    run["input_marker_changed_only"] = storage_format != original["_freight_format"]
                    statements = ["BEGIN", "INSERT INTO freight_workspaces(tenant_id,data) VALUES('large'," + literal(header) + "::jsonb)"]
                    for block in blocks:
                        statements.append("INSERT INTO freight_workspace_blocks(tenant_id,root,key,payload) VALUES('large'," + literal(block["root"]) + "," + literal(block["key"]) + "," + literal(exact.encode(block)) + "::jsonb)")
                    statements.extend(["UPDATE freight_workspaces SET data=" + literal(header) + "::jsonb WHERE tenant_id='large'", "COMMIT"])
                    sql(url, ";\n".join(statements) + ";")

                    def snapshot(label):
                        raw = sql(url, "SELECT freight_workspace_originals('large')::text").encode()
                        return exact.decode(raw), archive(label, raw)

                    def physical():
                        return sql(url, """SELECT jsonb_build_object(
                            'header',(SELECT jsonb_build_array(xmin::text,ctid::text,history_revision,md5(data::text)) FROM freight_workspaces WHERE tenant_id='large'),
                            'blocks',(SELECT jsonb_agg(jsonb_build_array(root,key,xmin::text,ctid::text,md5(payload::text)) ORDER BY root,key)
                                FROM freight_workspace_blocks WHERE tenant_id='large'))::text""")

                    stored, run["original"] = snapshot(prefix + "-original.json")
                    require(exact.encode(stored) == exact.encode(expected_original), "Seed changed exact original values beyond the validated format marker")
                    cases = {"workspace": "/v1/workspace", "deferred_workspace": "/v1/workspace?history=deferred",
                             "catalog": "/v1/loads?q=" + urllib.parse.quote(identity) + "&limit=25",
                             "history": "/v1/history?kind=events&limit=25"}
                    for kind, path in cases.items():
                        samples = []
                        for index in range(args.samples + 1):
                            sample, raw = request(path, prefix + f"-{kind}-{index}.json")
                            value = json.loads(raw)
                            if args.invoice_review_projection and variant == "candidate" and kind in {"workspace", "deferred_workspace"}:
                                for invoice in value["invoices"]:
                                    control = invoice.pop("approval_control")
                                    require(set(control) == {"required", "revision", "state", "entry_reviewed_by", "can_review_entry", "can_approve"}, "Unexpected invoice control fields")
                                    expected_state = "approved" if invoice["status"] == "approved" else "entry_review_required"
                                    require(control["required"] is True and control["can_review_entry"] == (invoice["status"] == "matched") and control["can_approve"] is False
                                            and control["entry_reviewed_by"] is None and control["state"] == expected_state
                                            and len(control["revision"]) == 64 and all(c in "0123456789abcdef" for c in control["revision"]), "Unexpected invoice review control for the owned named profiling credential")
                                sample["additive_invoice_controls_validated"] = len(value["invoices"])
                            if "operations" in value:
                                value["operations"].pop("generated_at")
                            # Cursor tokens retain source revisions per fresh schema;
                            # compare exact records and paging facts for this first page.
                            if kind == "history":
                                value.pop("next_cursor", None)
                            stable = hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
                            read_references.setdefault(kind, stable)
                            require(read_references[kind] == stable, "Read business output changed: " + kind)
                            sample["normalized_response_sha256"] = stable
                            if index:
                                samples.append(sample)
                            else:
                                run.setdefault("first_reads", {})[kind] = sample
                        run["reads"][kind] = {"samples": samples, "median_ms": statistics.median(s["wall_ms"] for s in samples),
                            "median_api_cpu_seconds": statistics.median(s["api_cpu_seconds"] for s in samples)}
                    unchanged, _ = snapshot(prefix + "-after-reads.json")
                    require(exact.encode(unchanged) == exact.encode(expected_original), "Reads changed stored originals")
                    for index in range(args.samples):
                        before, before_archive = snapshot(prefix + f"-before-save-{index}.json")
                        load = before["loads"][0]
                        body = {"action": "location", "expected_version": load["version"], "location": f"Synthetic observed location {index}"}
                        path = f"/v1/loads/{urllib.parse.quote(identity)}/commands"
                        key = f"local-storage-original-{index}"
                        sample, raw = request(path, prefix + f"-save-{index}.json", body, key)
                        result = json.loads(raw)
                        require(result["data"]["version"] == load["version"] + 1 and result["data"]["location"] == body["location"] and not result.get("replayed"), "Save result changed")
                        after, after_archive = snapshot(prefix + f"-after-save-{index}.json")
                        before_replay_physical = physical()
                        replay, replay_raw = request(path, prefix + f"-replay-{index}.json", body, key)
                        require(json.loads(replay_raw) == {**result, "replayed": True}, "Retry changed original response")
                        final, _ = snapshot(prefix + f"-after-replay-{index}.json")
                        require(exact.encode(final) == exact.encode(after), "Retry rewrote original business facts")
                        require(physical() == before_replay_physical, "Retry rewrote physical originals")
                        for name in ["appointments", "stop_events", "tenders", "carrier_submissions"]:
                            require(exact.encode(before[name]) == exact.encode(after[name]), "Save changed unrelated original " + name)
                        run["saves"].append({"new": sample, "replay": replay, "before": before_archive, "after": after_archive,
                                             "replay_preserves_physical_source": True})
                    process_status = Path(f"/proc/{process.pid}/status").read_text()
                    run["api_peak_rss_bytes"] = next(int(line.split()[1]) * 1024 for line in process_status.splitlines() if line.startswith("VmHWM:"))
                    run["status"] = "pass"
                    print("VERIFIED", prefix, "native reads, saves and exact retries", flush=True)
                finally:
                    if process is not None and process.poll() is None:
                        process.terminate()
                        try:
                            process.wait(timeout=35)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait()
                    sql(args.database_url, f"DROP SCHEMA {schema} CASCADE")
                    run["schema_removed"] = True
        require(report["binaries"] == {k: binding(v) for k, v in binaries.items()}, "Executable drift")
        require(report["inputs"] == [binding(p) for p in args.input], "Input drift")
        require(report["harness"] == binding(Path(__file__)), "Harness drift")
        require(report["codec"] == binding(args.codec), "Codec drift")
        require(report["exact_json_helper"] == binding(Path(__file__).with_name("profile-storage-records.py")), "Exact JSON helper drift")
        require(report["runtime_helpers"] == binding(Path(__file__).with_name("runtime_contracts.py")), "Runtime helper drift")
        report["result"] = "pass"
    except Exception as error:
        report.update(result="failed", error=f"{type(error).__name__}: {error}")
        print(report["error"], flush=True)
    finally:
        report["host_at_finish"] = {"logical_cpus": os.cpu_count(), "load_average": os.getloadavg()}
        report["finished_utc"] = dt.datetime.now(dt.timezone.utc).isoformat()
        with args.output.open("x") as out:
            out.write(json.dumps(report, indent=2) + "\n")
        print("REPORT", args.output, flush=True)
    return 0 if report["result"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
