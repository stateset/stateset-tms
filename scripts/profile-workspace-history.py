#!/usr/bin/env python3
"""Compare actual full-workspace HTTP reads with retained typed stop history."""
import argparse
import datetime as dt
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import signal
import statistics
import subprocess
import sys
import threading
import time

from runtime_contracts import ROOT, Runtime, fingerprint, literal, require

spec = importlib.util.spec_from_file_location("profiled_reads", ROOT / "scripts/profile-service-reads.py")
profiled = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profiled)
TOKEN = profiled.capacity.LARGE


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def stable(value):
    return sha(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())


class WorkspaceHistory(profiled.ProfiledCapacity):
    def __init__(self, args):
        Runtime.__init__(self, args)
        self.source = fingerprint(ROOT)
        self.baseline = json.loads(args.baseline.read_text()) if args.baseline else None
        self.clock = self.baseline["fixture_clock"] if self.baseline else dt.datetime.now(dt.timezone.utc).isoformat()
        self.harness = {p: sha((ROOT / p).read_bytes()) for p in (
            "scripts/profile-workspace-history.py", "examples/workspace_history_fixture.rs",
            "scripts/runtime_contracts.py", "scripts/qualify-service-capacity.py", "scripts/profile-service-reads.py",
        )}
        self.secrets += [TOKEN]
        self.report.update(schema=1, scope="Actual owned loopback full-workspace HTTP/PostgreSQL reads; typed synthetic corrected gates and appointment history; no capacity SLA", fixture_clock=self.clock,
            settings={"workloads": [[100, 4], [1000, 4], [1000, 16]], "timed_samples": args.samples, "warmups": 1, "concurrency": 1,
                "api_async_threads": 2, "api_cpu_quota": "not imposed", "api_memory_quota": "not imposed", "postgres_cpu_quota": 2, "postgres_memory_mib": 768,
                "normalization": "Remove only operations.generated_at after validating its timestamp. All other decoded response fields must match."},
            host={"platform": platform.platform(), "logical_cpus": os.cpu_count(), "cpu_model": next((line.partition(":")[2].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown"), "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip()},
            harness=self.harness, workloads=[])
        if self.baseline:
            for key in ["settings", "host", "harness"]:
                require(self.baseline[key] == self.report[key], key + " changed")

    def build(self):
        env = dict(os.environ, RUSTC_WRAPPER="", CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="2", CARGO_TARGET_DIR=str(ROOT / "target"))
        with (self.directory / "build.log").open("w") as log:
            subprocess.run(["cargo", "build", "--offline", "--locked", "--release", "--bin", "stateset-freight", "--example", "workspace_history_fixture"], cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        require(self.source == fingerprint(ROOT), "Application changed during build")
        self.binaries = {"freight": ROOT / "target/release/stateset-freight"}
        self.generator = ROOT / "target/release/examples/workspace_history_fixture"
        self.report["builds"] = {"freight": {"source": self.source, "binary_sha256": sha(self.binaries["freight"].read_bytes())}, "fixture": {"binary_sha256": sha(self.generator.read_bytes())}}

    def validate(self, response, workspace):
        require(len(response["loads"]) == len(workspace["loads"]), "Shipments missing")
        for key in ["appointments", "stop_events", "tenders", "carrier_submissions", "events", "stop_services", "movement_corrections"]:
            require(response[key] == workspace[key], "Retained records changed: " + key)
        superseded = {g["supersedes_id"] for g in workspace["stop_events"] if g["supersedes_id"]}
        gates, appointments = {}, {}
        for gate in workspace["stop_events"]:
            if gate["id"] not in superseded:
                gates.setdefault((gate["load_id"], gate["tender_id"]), []).append(gate)
        for appointment in workspace["appointments"]:
            if appointment["status"] == "active":
                appointments.setdefault((appointment["load_id"], appointment["stop"]), appointment)
        for load in response["loads"]:
            current = gates.get((load["id"], load["tender_id"]), [])
            require(load["execution_timeline"]["consistent"] is True and load["route_attention"] is None, "Chronology changed")
            require(load["execution_timeline"]["record_count"] == len(current) + 1, "Current timeline scope changed")
            for stop in ["pickup", "delivery"]:
                selected = load["stop_execution"][stop]
                require(selected["appointment"] == appointments.get((load["id"], stop)), "First active window changed")
                for kind in ["arrival", "departure"]:
                    expected = next((g for g in current if g["stop"] == stop and g["kind"] == kind), None)
                    require(selected[kind] == expected, "Gate identity/tender/correction selection changed")
                require(not selected["dwell_open"], "Fixture must not have volatile open dwell")
        generated = response["operations"].pop("generated_at")
        dt.datetime.fromisoformat(re.sub(r"(\.\d{6})\d+", r"\1", generated).replace("Z", "+00:00"))
        return stable(response), generated

    def checks(self):
        self.start_api()
        self.stop("freight")
        version = int(self.sql("freight_runtime", "SELECT version FROM freight_storage_format"))
        for index, (count, depth) in enumerate(self.report["settings"]["workloads"]):
            self.sql("freight_runtime", "TRUNCATE freight_load_search,freight_load_catalog,freight_workspaces,freight_document_content CASCADE")
            raw = subprocess.check_output([str(self.generator), str(count), f"workspace-{count}-{depth}", self.clock, str(depth)], cwd=self.directory)
            fixture = json.loads(raw)
            path = self.directory / f"fixture-{count}-{depth}.json.gz"
            path.write_bytes(gzip.compress(raw, compresslevel=1, mtime=0))
            workload = {"shipments": count, "history_depth": depth, "fixture_sha256": sha(raw), "counts": fixture["counts"], "fixture_archive": {"path": str(path), "sha256": sha(path.read_bytes())}, "samples": []}
            if self.baseline:
                prior = self.baseline["workloads"][index]
                for key in ["shipments", "history_depth", "fixture_sha256", "counts"]:
                    require(workload[key] == prior[key], "Fixture changed: " + key)
            workspace = fixture["workspace"]
            workspace["_freight_format"] = version
            self.command(["psql", f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime", "-X", "-v", "ON_ERROR_STOP=1"], label="owned history fixture insert",
                input="INSERT INTO freight_workspaces(tenant_id,data) VALUES('large'," + literal(json.dumps(workspace)) + "::jsonb);", env={"PATH": os.defpath, "LC_ALL": "C"})
            before = self.snapshot()
            self.start_api()
            self.report["workloads"].append(workload)
            expected = None
            for n in range(self.args.samples + 1):
                stop = threading.Event()
                observations = []
                def monitor():
                    while not stop.is_set():
                        try:
                            observations.append(profiled.process_sample(self.processes["freight"].pid))
                        except Exception as error:
                            observations.append({"error": type(error).__name__})
                        stop.wait(0.2)
                thread = threading.Thread(target=monitor, daemon=True)
                thread.start()
                try:
                    sample, body = self.request(("workspace", "/v1/workspace", TOKEN, None, None))
                finally:
                    stop.set()
                    thread.join(10)
                require(not thread.is_alive() and observations and all("error" not in p for p in observations), "Process sampling failed")
                require(sample["status"] == 200 and sample["error"] is None, "Workspace read failed")
                response_path = self.directory / f"response-{count}-{depth}-{n}.json.gz"
                response_path.write_bytes(gzip.compress(body, compresslevel=1, mtime=0))
                workload["latest_attempt"] = {**sample, "archive": {"path": str(response_path), "sha256": sha(response_path.read_bytes())}}
                self.save()
                output_hash, generated = self.validate(json.loads(body), workspace)
                expected = expected or output_hash
                require(output_hash == expected, "Response changed between samples")
                if self.baseline:
                    require(output_hash == prior["normalized_response_sha256"], "Before/after workspace response changed")
                record = {**sample, "normalized_response_sha256": output_hash, "generated_at": generated, "process_samples": observations,
                    "raw_response_sha256": sha(body), "archive": {"path": str(response_path), "sha256": sha(response_path.read_bytes())}}
                if n == 0:
                    workload["warmup"] = record
                else:
                    workload["samples"].append(record)
                print(f"READ {count}/{depth} sample {n}: {sample['response_ms']:.1f}ms", flush=True)
                self.save()
            workload["normalized_response_sha256"] = expected
            timings = [s["response_ms"] for s in workload["samples"]]
            workload["latency_ms"] = {"min": min(timings), "median": statistics.median(timings), "max": max(timings)}
            after = self.snapshot()
            require(before == after, "Read changed physical workspace revision or retained bytes")
            workload["unchanged_physical_snapshot"] = {"before": before, "after": after}
            self.stop("freight")
            self.save()
        self.report["qualification"] = {"functional_contracts": "pass", "capacity_sla": "not established", "normalization": "Only operations.generated_at omitted; complete retained raw responses archived."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--samples", type=int, choices=[3, 5, 7, 9], default=7)
    parser.add_argument("--postgres-image", default="postgres:16")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("preserve the existing report; choose a new output path")
    run = WorkspaceHistory(args)
    def interrupt(_signum, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupt)
    try:
        run.build()
        run.postgres()
        run.checks()
        require(run.source == fingerprint(ROOT), "Application changed during measurement")
        require(all(sha((ROOT / p).read_bytes()) == h for p, h in run.harness.items()), "Harness changed during measurement")
        run.report["result"] = "measured"
    except BaseException as error:
        run.report.update(result="failed", error=run.safe(f"{type(error).__name__}: {error}"))
        print(run.report["error"], file=sys.stderr, flush=True)
    finally:
        try:
            run.cleanup()
        except BaseException as error:
            run.report.update(result="failed", cleanup_error=run.safe(str(error)))
        diagnostics = run.report.get("postgres_diagnostics", {})
        if run.report["result"] == "measured":
            if diagnostics.get("cgroup_version") == 2:
                kills = diagnostics.get("memory_events", {}).get("oom_kill")
            else:
                counters = dict(line.split() for line in diagnostics.get("memory.oom_control", "").splitlines())
                kills = int(counters["oom_kill"]) if "oom_kill" in counters else None
            if kills != 0 or diagnostics.get("state", {}).get("OOMKilled") is not False or run.report.get("diagnostic_error"):
                run.report.update(result="failed", error="PostgreSQL memory diagnostics missing or OOM kill observed")
            else:
                run.report["qualification"]["no_database_oom_kills"] = True
        run.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        run.save()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("x") as stream:
            stream.write((run.directory / "report.json").read_text())
    return 0 if run.report["result"] == "measured" else 1


if __name__ == "__main__":
    sys.exit(main())
