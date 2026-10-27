#!/usr/bin/env python3
"""Observe real new-save/replay HTTP costs with exact retained-history checks.

Finite sequential synthetic requests, not a capacity SLA or a causal memory
estimate. Continuous kernel monitoring includes controller gaps and rejects OOM.
"""
import argparse
import datetime as dt
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess
import sys
import threading

from runtime_contracts import ROOT, fingerprint, literal, require

spec = importlib.util.spec_from_file_location("memory_profile", ROOT / "scripts/profile-history-memory.py")
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
sha = lambda raw: hashlib.sha256(raw).hexdigest()
TOKEN = base.indexed.history.TOKEN


class SharedSaves(base.HistoryMemory):
    def __init__(self, args):
        super().__init__(args)
        self.harness["scripts/profile-shared-saves.py"] = sha(Path(__file__).read_bytes())
        self.report.update(scope=__doc__, workloads=[], comparison_scope="Same fixed input executable/clock; no response-field normalization for exact receipt equality. Controller checks and sampling affect cadence.")
        self.report["settings"].update(workloads=[[100, 4], [1000, 4]], timed_samples=args.samples, warmups=0, request_types=["new_location", "original_replay"], concurrency=1)

    def build(self):
        retained = self.args.inventory.parent
        if self.args.generation == "before":
            binary = retained / "release"
            require(sha(binary.read_bytes()) == self.inventory["binaries"]["release"], "Preceding executable changed")
            for relative, expected in self.inventory["source_files_sha256"].items():
                require(sha((retained / "source" / relative).read_bytes()) == expected, "Preceding source changed")
            source = self.inventory["core"]
        else:
            command = ["cargo", "build", "--offline", "--locked", "--release", "--bin", "stateset-freight"]
            with (self.directory / "build.log").open("w") as log:
                subprocess.run(command, cwd=ROOT, env=dict(os.environ, RUSTC_WRAPPER="", CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="2", CARGO_TARGET_DIR=str(ROOT / "target")), stdout=log, stderr=subprocess.STDOUT, check=True)
            require(fingerprint(ROOT) == self.source, "Current source changed during build")
            binary = ROOT / "target/release/stateset-freight"
            source = self.source
        saved = self.directory / "stateset-freight"
        saved.write_bytes(binary.read_bytes())
        saved.chmod(0o755)
        self.binaries = {"freight": saved}
        self.generator = retained / "fixture"
        require(sha(self.generator.read_bytes()) == self.inventory["binaries"]["fixture"], "Fixture executable changed")
        self.report["builds"] = {"freight": {"source": source, "binary_sha256": sha(saved.read_bytes()), "path": str(saved)}, "fixture": {"binary_sha256": sha(self.generator.read_bytes()), "path": str(self.generator)}}

    def observe(self, kind, identity, body, key):
        stop, samples = threading.Event(), []
        def monitor():
            while not stop.is_set():
                try:
                    samples.append(base.indexed.history.profiled.process_sample(self.processes["freight"].pid))
                except Exception as error:
                    samples.append({"error": str(error)})
                stop.wait(0.05)
        thread = threading.Thread(target=monitor, daemon=True)
        thread.start()
        try:
            sample, raw = self.request((kind, "/v1/loads/" + identity + "/commands", TOKEN, body, key))
        finally:
            stop.set()
            thread.join(10)
        require(not thread.is_alive() and samples and all("error" not in p for p in samples), "API process sampling failed")
        require(sample["status"] == 200 and sample["error"] is None, "Save failed: " + raw.decode(errors="replace")[:1000])
        return {**sample, "process_samples": samples}, raw

    def source_state(self, name):
        raw = self.sql("freight_runtime", "SELECT data::text FROM freight_workspaces WHERE tenant_id='large'").encode()
        return json.loads(raw), self.archive(name, raw)

    def checks(self):
        self.start_api()
        self.stop("freight")
        version = int(self.sql("freight_runtime", "SELECT version FROM freight_storage_format"))
        for count, depth in self.report["settings"]["workloads"]:
            self.sql("freight_runtime", "TRUNCATE freight_workspaces CASCADE")
            prefix = f"shared-save-{count}-{depth}"
            raw = subprocess.check_output([str(self.generator), str(count), prefix, self.clock, str(depth)], cwd=self.directory)
            fixture = json.loads(raw)
            workspace = fixture["workspace"]
            workspace["_freight_format"] = version
            self.command(["psql", f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime", "-X", "-v", "ON_ERROR_STOP=1"], label="owned fixed shared-save fixture", input="INSERT INTO freight_workspaces(tenant_id,data) VALUES('large'," + literal(json.dumps(workspace)) + "::jsonb);")
            output = {"shipments": count, "history_depth": depth, "fixture_sha256": sha(raw), "fixture_archive": self.archive(prefix + "-fixture", raw), "counts": fixture["counts"], "samples": []}
            self.report["workloads"].append(output)
            self.start_api()
            identity = workspace["loads"][0]["id"]
            for index in range(self.args.samples):
                before, archive = self.source_state(prefix + f"-before-{index}")
                current = next(load for load in before["loads"] if load["id"] == identity)
                body = {"action": "location", "expected_version": current["version"], "location": f"Synthetic observed location {index}"}
                key = prefix + f"-original-{index}"
                sample, raw = self.observe("new_location", identity, body, key)
                record = {"key": key, "request": body, "source_before": archive, "new_location": {**sample, "archive": self.archive(prefix + f"-new-{index}", raw)}, "verification": "pending"}
                output["samples"].append(record)
                self.save()
                original = json.loads(raw)
                require(original.get("replayed") is not True, "New key replayed another decision")
                require(original["data"]["version"] == current["version"] + 1 and original["data"]["location"] == body["location"], "New observation did not apply exactly")
                after, after_archive = self.source_state(prefix + f"-after-{index}")
                require(set(after) == set(before), "Source root fields changed")
                for field in set(before) - {"loads", "events", "receipts", "operations_cases"}:
                    require(after[field] == before[field], "Unrelated original field changed: " + field)
                for old in before["loads"]:
                    current_load = next(load for load in after["loads"] if load["id"] == old["id"])
                    allowed = {"version", "location", "updated_at", "last_tracking_at"} if old["id"] == identity else set()
                    require(set(current_load) == set(old) and all(current_load[field] == value for field, value in old.items() if field not in allowed), "Unrelated shipment facts changed")
                require(all(after["receipts"][key] == value for key, value in before["receipts"].items()), "An earlier original receipt changed")
                require(after["events"][:len(before["events"])] == before["events"], "Original audit prefix changed")
                require(after["receipts"][key]["response"] == original["data"], "Original receipt differs from HTTP result")
                physical = self.snapshot()
                replay_sample, replay_raw = self.observe("original_replay", identity, body, key)
                record.update(source_after=after_archive, original_replay={**replay_sample, "archive": self.archive(prefix + f"-replay-{index}", replay_raw)})
                self.save()
                require(json.loads(replay_raw) == {"data": original["data"], "replayed": True}, "Retry changed the original result")
                require(self.snapshot() == physical, "Retry rewrote the authoritative source")
                record.update(replay_preserves_physical_source=True, verification="pass")
                self.save()
            self.stop("freight")
            output["summary"] = {kind: {"samples_ms": [s[kind]["response_ms"] for s in output["samples"]], "median_ms": statistics.median(s[kind]["response_ms"] for s in output["samples"]), "peak_sampled_api_rss_bytes": max(p["status"]["VmRSS"] for s in output["samples"] for p in s[kind]["process_samples"])} for kind in ["new_location", "original_replay"]}
            print("OBSERVED", count, output["summary"], flush=True)
            self.save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument("--generation", choices=["before", "after"], required=True)
    parser.add_argument("--reference", type=Path, default=ROOT / "docs/evidence/workspace-history-after.json")
    parser.add_argument("--samples", type=int, choices=[3, 5, 7, 9], default=5)
    parser.add_argument("--postgres-image", default="postgres:16")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Preserve existing evidence and choose a new output path")
    args.variant = "after"
    run = SharedSaves(args)
    def interrupt(_signal, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupt)
    try:
        run.build()
        run.postgres()
        run.prepare_sampling()
        run.checks()
        require(fingerprint(ROOT) == run.source, "Source changed during observations")
        require(all(sha((ROOT / p).read_bytes()) == h for p, h in run.harness.items()), "Harness changed")
        run.report["result"] = "measured"
    except BaseException as error:
        run.report.update(result="failed", error=run.safe(f"{type(error).__name__}: {error}"))
        print(run.report["error"], file=sys.stderr, flush=True)
    finally:
        try:
            run.stop_sampling()
        except BaseException as error:
            run.report.update(result="failed", sampling_error=run.safe(str(error)))
        run.trace_enabled = False
        try:
            run.cleanup()
        except BaseException as error:
            run.report.update(result="failed", cleanup_error=run.safe(str(error)))
        diagnostic = run.report.get("postgres_diagnostics", {})
        if diagnostic.get("cgroup_version") == 2:
            kills = diagnostic.get("memory_events", {}).get("oom_kill")
        else:
            counters = dict(line.split() for line in diagnostic.get("memory.oom_control", "").splitlines())
            kills = int(counters["oom_kill"]) if "oom_kill" in counters else None
        run.report["no_oom_kills"] = kills == 0 and diagnostic.get("state", {}).get("OOMKilled") is False
        if not run.report["no_oom_kills"] or run.report.get("diagnostic_error"):
            run.report["result"] = "failed"
            run.report["resource_error"] = "Database OOM or missing final diagnostics"
            run.report.setdefault("error", run.report["resource_error"])
        run.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        run.save()
        with args.output.open("x") as stream:
            stream.write((run.directory / "report.json").read_text())
    return 0 if run.report["result"] == "measured" else 1


if __name__ == "__main__":
    sys.exit(main())
