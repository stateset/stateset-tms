#!/usr/bin/env python3
"""Observe native storage reads, writes and upgrades on identical inputs.

Each invocation owns a fresh, bounded PostgreSQL cluster. This finite sequential
profile is not a sustained-capacity or deployment guarantee. Preserve every run,
including failures, and compare counterbalanced runs of this exact harness.
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
import time

from runtime_contracts import ROOT, fingerprint, literal, require

spec = importlib.util.spec_from_file_location("canonical_memory", ROOT / "scripts/profile-history-memory.py")
memory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(memory)
TOKEN = memory.indexed.history.TOKEN


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def cpu(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


class NativeStorage(memory.HistoryMemory):
    def __init__(self, args):
        super().__init__(args)
        self.harness["scripts/profile-canonical-storage.py"] = sha(Path(__file__))
        self.api_stop = threading.Event()
        self.api_monitor = None
        self.api_lock = threading.Lock()
        self.api_samples = []
        self.current_phase = "setup"
        self.report.update(scope=__doc__, api_samples=self.api_samples, workloads=[],
            fixture_clock=self.inventory["clock"],
            comparison_scope="Same archived fixture executable/clock and release binaries; all production triggers/workers enabled. Monitoring and SQL verification affect cadence. Shared host, no API quota.")
        self.clock = self.inventory["clock"]
        self.report["settings"].update(workloads=[[100, 4], [1000, 4]],
            timed_samples=args.samples, warmups=1, concurrency=1,
            startup="Format-18 initialization and fixture insertion before every workload. Timed startup targets each release's declared format: aggregate restart or canonical migration through health readiness.",
            native_requests=["workspace", "deferred_workspace", "shipment_catalog", "global_history", "shipment_history", "new_location", "original_replay"],
            process_sampling_seconds=0.05, controller_work="Outside HTTP latency; PostgreSQL monitoring includes all controller gaps.")

    def storage_format(self):
        value = self.inventory[self.args.generation].get("storage_format", 18 if self.args.generation == "before" else 19)
        require(value in {18, 19}, "Unsupported measured storage format")
        return value

    def verify_inputs(self):
        require(sha(self.args.inventory) == self.report["inventory"]["sha256"], "Input manifest changed")
        for info in [self.inventory["before"], self.inventory["after"], self.inventory.get("initializer", self.inventory["before"])]:
            require(fingerprint(Path(info["source_path"])) == info["source"], "Retained core changed")
            require(sha(Path(info["binary"])) == info["binary_sha256"], "Retained executable changed")
        require(fingerprint(ROOT) == self.inventory["after"]["source"], "Application core changed")
        fixture = self.inventory["fixture"]
        require(sha(Path(fixture["path"])) == fixture["sha256"], "Fixture executable changed")
        require(sha(ROOT / "examples/workspace_history_fixture.rs") == fixture["source_sha256"], "Fixture source changed")

    def build(self):
        self.verify_inputs()
        self.generator = Path(self.inventory["fixture"]["path"])
        self.saved_binaries = {}
        for generation, info in [("before", self.inventory["before"]), ("after", self.inventory["after"]), ("initializer", self.inventory.get("initializer", self.inventory["before"]))]:
            dest = self.directory / (generation + "-stateset-freight")
            dest.write_bytes(Path(info["binary"]).read_bytes())
            dest.chmod(0o755)
            self.saved_binaries[generation] = dest
        self.binaries = {"freight": self.saved_binaries[self.args.generation]}
        self.report["builds"] = {"freight": {**self.inventory[self.args.generation], "path": str(self.binaries["freight"]), "storage_format": self.storage_format()}, "initializer": self.inventory.get("initializer", self.inventory["before"]), "fixture": self.inventory["fixture"]}

    def prepare_sampling(self):
        super().prepare_sampling()

        def monitor():
            while not self.api_stop.is_set():
                with self.api_lock:
                    process = self.processes.get("freight")
                    if process is not None and process.poll() is None:
                        try:
                            value = memory.indexed.history.profiled.process_sample(process.pid)
                            self.api_samples.append({"phase": self.current_phase, "pid": process.pid, "cpu_seconds": cpu(process.pid), **value})
                        except FileNotFoundError:
                            # An actual startup failure remains a failed run.
                            if process.poll() is None:
                                self.api_samples.append({"error": "live API sampling failed"})
                        except Exception as error:
                            self.api_samples.append({"error": str(error)})
                self.api_stop.wait(0.05)

        self.api_monitor = threading.Thread(target=monitor, daemon=True)
        self.api_monitor.start()

    def stop(self, name):
        with self.api_lock:
            return super().stop(name)

    def stop_sampling(self):
        self.api_stop.set()
        if self.api_monitor is not None:
            self.api_monitor.join(15)
            require(not self.api_monitor.is_alive(), "API monitor did not finish")
        require(self.api_samples and all("error" not in p for p in self.api_samples), "Continuous API sampling failed")
        super().stop_sampling()

    def raw_source(self):
        expression = "freight_workspace_originals('large')" if self.storage_format() == 19 else "data"
        return self.sql("freight_runtime", f"SELECT ({expression})::text FROM freight_workspaces WHERE tenant_id='large'").encode()

    def snapshot(self):
        result = {"headers": self.rows("freight_runtime", "SELECT tenant_id,xmin::text,ctid::text,updated_at,history_revision,md5(data::text) AS hash FROM freight_workspaces ORDER BY tenant_id")}
        if self.storage_format() == 19:
            result["blocks"] = self.rows("freight_runtime", "SELECT tenant_id,root,key,xmin::text,ctid::text,md5(payload::text) AS hash FROM freight_workspace_blocks ORDER BY tenant_id,root,key")
            result["counts"] = self.rows("freight_runtime", "SELECT tenant_id,root,records,blocks,xmin::text,ctid::text FROM freight_block_counts ORDER BY tenant_id,root")
        return result

    def disk(self):
        tables = self.rows("freight_runtime", "SELECT c.relname AS name,pg_total_relation_size(c.oid) AS bytes FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind='r' ORDER BY c.relname")
        return {"tables_including_toast_and_indexes": tables, "total_bytes": sum(t["bytes"] for t in tables)}

    def observed_http(self, prefix, kind, path, body=None, key=None):
        self.current_phase = prefix
        pid = self.processes["freight"].pid
        before = cpu(pid)
        start_sample = len(self.api_samples)
        sample, raw = self.request((kind, path, TOKEN, body, key))
        sample["api_cpu_seconds"] = cpu(pid) - before
        sample["archive"] = self.archive(prefix, raw)
        sample["api_sample_range"] = [max(0, start_sample - 1), len(self.api_samples)]
        require(sample["status"] == 200 and sample["error"] is None, "HTTP failure: " + raw.decode(errors="replace")[:600])
        self.current_phase = "controller"
        return sample, json.loads(raw)

    def reads(self, w, workspace):
        prefix = w["prefix"]
        identity = workspace["loads"][0]["id"]
        cases = {
            "workspace": "/v1/workspace",
            "deferred_workspace": "/v1/workspace?history=deferred",
            "shipment_catalog": "/v1/loads?q=" + identity + "&limit=25",
            "global_history": self.history_path("events"),
            "shipment_history": self.history_path("events", identity),
        }
        before = self.snapshot()
        originals = self.raw_source()
        reference_complete = None
        for kind, path in cases.items():
            output = {"samples": []}
            w["reads"][kind] = output
            expected = None
            for i in range(self.args.samples + 1):
                sample, value = self.observed_http(f"{prefix}-{kind}-{i}", kind, path)
                if kind == "workspace":
                    stable, stamp = self.validate(value, workspace)
                    reference_complete = value
                    sample["generated_at"] = stamp
                elif kind == "deferred_workspace":
                    require(value.pop("history") == {"mode": "deferred", "events": len(workspace["events"]), "appointments": len(workspace["appointments"]), "gates": len(workspace["stop_events"])}, "Deferred counts changed")
                    stamp = value["operations"].pop("generated_at")
                    dt.datetime.fromisoformat(memory.indexed.history.re.sub(r"(\.\d{6})\d+", r"\1", stamp).replace("Z", "+00:00"))
                    require(value == {k: v for k, v in reference_complete.items() if k not in {"events", "appointments", "stop_events"}}, "Deferred operating facts changed")
                    stable = memory.indexed.history.stable(value)
                    sample["generated_at"] = stamp
                elif kind.endswith("history"):
                    expected_items = [{"position": j + 1, "record": row} for j, row in reversed(list(enumerate(workspace["events"]))) if kind == "global_history" or row["entity_id"] == identity]
                    require(value["items"] == expected_items[:25] and value["total"] == len(expected_items), "Selected original history changed")
                    require(value["captured_through"] == len(workspace["events"]) and value["has_new_records"] is False, "Captured history window changed")
                    stable = memory.indexed.history.stable(value)
                else:
                    require(len(value["loads"]) == 1 and value["loads"][0]["id"] == identity and value["loads"][0]["version"] == workspace["loads"][0]["version"], "Selected catalog shipment changed")
                    stable = memory.indexed.history.stable(value)
                expected = expected or stable
                require(stable == expected, "Repeated read changed business output")
                sample["normalized_response_sha256"] = stable
                if i == 0:
                    output["first"] = sample
                else:
                    output["samples"].append(sample)
                self.save()
            output["median_ms"] = statistics.median(s["response_ms"] for s in output["samples"])
            output["median_api_cpu_seconds"] = statistics.median(s["api_cpu_seconds"] for s in output["samples"])
            print("READ", prefix, kind, round(output["median_ms"], 2), flush=True)
        require(self.snapshot() == before and self.raw_source() == originals, "Reads rewrote original source")
        w["reads_preserve_physical_source"] = True

    def saves(self, w):
        prefix = w["prefix"]
        for i in range(self.args.samples):
            raw_before = self.raw_source()
            before = json.loads(raw_before)
            current = before["loads"][0]
            identity = current["id"]
            request = {"action": "location", "expected_version": current["version"], "location": f"Synthetic observed location {i}"}
            key = prefix + f"-original-{i}"
            prior_physical = self.snapshot()
            sample, result = self.observed_http(f"{prefix}-new-{i}", "new_location", f"/v1/loads/{identity}/commands", request, key)
            record = {"request": request, "key": key, "new_location": sample, "source_before": self.archive(f"{prefix}-source-before-{i}", raw_before)}
            w["saves"].append(record)
            self.save()
            require(result.get("replayed") is not True and result["data"]["version"] == current["version"] + 1 and result["data"]["location"] == request["location"], "New observation did not apply once")
            raw_after = self.raw_source()
            after = json.loads(raw_after)
            record["source_after"] = self.archive(f"{prefix}-source-after-{i}", raw_after)
            require(set(after) == set(before), "Business roots changed")
            for field in set(before) - {"loads", "events", "receipts", "operations_cases"}:
                require(after[field] == before[field], "Unrelated root changed: " + field)
            require(len(after["loads"]) == len(before["loads"]), "Shipment count changed")
            for old, new in zip(before["loads"], after["loads"]):
                allowed = {"version", "location", "updated_at", "last_tracking_at"} if old["id"] == identity else set()
                require(set(old) == set(new) and all(new[k] == v for k, v in old.items() if k not in allowed), "Unrelated shipment original changed")
            require(after["events"][:-1] == before["events"], "Original audit prefix changed")
            require(len(after["receipts"]) == len(before["receipts"]) + 1 and all(after["receipts"][k] == v for k, v in before["receipts"].items()), "Original receipt changed")
            require(after["receipts"][key]["response"] == result["data"], "Stored receipt differs from HTTP")
            physical = self.snapshot()
            if self.storage_format() == 19:
                previous = {(b["tenant_id"], b["root"], b["key"]): b for b in prior_physical["blocks"]}
                present = {(b["tenant_id"], b["root"], b["key"]): b for b in physical["blocks"]}
                changed = [k for k in set(previous) | set(present) if previous.get(k) != present.get(k)]
                require(0 < len(changed) < len(present), "Save rewrote all blocks or none")
                record["physical_delta"] = {"changed_blocks": len(changed), "total_blocks": len(present), "roots": sorted({k[1] for k in changed})}
            replay, replay_value = self.observed_http(f"{prefix}-replay-{i}", "original_replay", f"/v1/loads/{identity}/commands", request, key)
            record["original_replay"] = replay
            require(replay_value == {"data": result["data"], "replayed": True}, "Retry changed its original result")
            require(self.snapshot() == physical, "Exact replay rewrote physical source")
            record["verification"] = "pass"
            self.save()
        w["save_summary"] = {kind: {"median_ms": statistics.median(s[kind]["response_ms"] for s in w["saves"]), "median_api_cpu_seconds": statistics.median(s[kind]["api_cpu_seconds"] for s in w["saves"])} for kind in ["new_location", "original_replay"]}
        print("SAVES", prefix, w["save_summary"], flush=True)

    def checks(self):
        for count, depth in self.report["settings"]["workloads"]:
            self.sql("postgres", "DROP DATABASE freight_runtime WITH (FORCE)")
            self.sql("postgres", "CREATE DATABASE freight_runtime")
            self.binaries["freight"] = self.saved_binaries["initializer"]
            self.current_phase = "initialize-format-18"
            self.start_api()
            self.stop("freight")
            require(self.sql("freight_runtime", "SELECT version FROM freight_storage_format") == "18", "Predecessor initialization changed")
            prefix = f"native-storage-{count}-{depth}"
            fixture_raw = subprocess.check_output([str(self.generator), str(count), prefix, self.clock, str(depth)], cwd=self.directory)
            fixture = json.loads(fixture_raw)
            workspace = fixture["workspace"]
            workspace["_freight_format"] = 18
            self.command(["psql", f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime", "-X", "-v", "ON_ERROR_STOP=1"], label="owned format-18 history fixture", input="INSERT INTO freight_workspaces(tenant_id,data) VALUES('large'," + literal(json.dumps(workspace)) + "::jsonb);")
            initial = self.sql("freight_runtime", "SELECT data::text FROM freight_workspaces WHERE tenant_id='large'").encode()
            metadata = self.rows("freight_runtime", "SELECT updated_at,history_revision FROM freight_workspaces WHERE tenant_id='large'")
            w = {"prefix": prefix, "shipments": count, "history_depth": depth, "fixture_archive": self.archive(prefix + "-fixture", fixture_raw), "source_pre_startup": self.archive(prefix + "-source-pre-startup", initial), "disk_before_startup": self.disk(), "reads": {}, "saves": []}
            self.report["workloads"].append(w)
            self.binaries["freight"] = self.saved_binaries[self.args.generation]
            self.current_phase = prefix + "-startup"
            sample_start = len(self.api_samples)
            started = time.perf_counter()
            self.phase("native-startup:" + prefix, self.start_api)
            w["startup"] = {"readiness_seconds": time.perf_counter() - started, "api_cpu_seconds": cpu(self.processes["freight"].pid), "api_sample_range": [sample_start, len(self.api_samples)]}
            self.current_phase = "controller"
            migrated = self.raw_source()
            expected = initial.replace(b'"_freight_format": 18', b'"_freight_format": 19') if self.storage_format() == 19 else initial
            require(migrated == expected, "Native startup changed exact original JSONB beyond the format marker")
            require(self.rows("freight_runtime", "SELECT updated_at,history_revision FROM freight_workspaces WHERE tenant_id='large'") == metadata, "Startup changed portable revision/timestamp")
            require(self.sql("freight_runtime", "SELECT version FROM freight_storage_format") == str(self.storage_format()), "Registry not upgraded")
            w.update(source_post_startup=self.archive(prefix + "-source-post-startup", migrated), disk_after_startup=self.disk(), exact_startup_originals=True)
            self.save()
            workspace["_freight_format"] = self.storage_format()
            self.reads(w, workspace)
            self.saves(w)
            w["disk_after_requests"] = self.disk()
            self.stop("freight")
            w["result"] = "measured"
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
        parser.error("Preserve prior evidence and choose a new output")
    args.variant = "after"
    run = NativeStorage(args)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        run.build()
        run.postgres()
        run.prepare_sampling()
        run.checks()
        run.verify_inputs()
        require(all(sha(ROOT / p) == h for p, h in run.harness.items()), "Harness changed during observations")
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
        counters = dict(line.split() for line in diagnostic.get("memory.oom_control", "").splitlines())
        kills = diagnostic.get("memory_events", {}).get("oom_kill") if diagnostic.get("cgroup_version") == 2 else int(counters.get("oom_kill", "-1"))
        hits = diagnostic.get("memory_events", {}).get("max") if diagnostic.get("cgroup_version") == 2 else int(diagnostic.get("memory.failcnt", "-1"))
        require_no_oom = kills == 0 and hits == 0 and diagnostic.get("state", {}).get("OOMKilled") is False and not run.report.get("diagnostic_error")
        run.report["no_oom_or_limit_hits"] = require_no_oom
        if not require_no_oom:
            run.report.update(result="failed", resource_error="Database OOM, limit hit or missing final diagnostics")
        run.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        run.save()
        with args.output.open("x") as out:
            out.write((run.directory / "report.json").read_text())
        print("REPORT", args.output, "RESULT", run.report["result"], flush=True)
    return 0 if run.report["result"] == "measured" else 1


if __name__ == "__main__":
    sys.exit(main())
