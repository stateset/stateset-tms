#!/usr/bin/env python3
"""Measure complete versus deferred workspace reads and verify original pages.

Uses the retained dense-history fixtures and complete-response hashes. Each
response mode runs in a fresh API process. Results are observations, not an SLA.
"""
import argparse
import datetime as dt
import gzip
import importlib.util
import json
from pathlib import Path
import signal
import statistics
import sys
import threading
from urllib.parse import urlencode

from runtime_contracts import ROOT, fingerprint, literal, require

spec = importlib.util.spec_from_file_location("workspace_history", ROOT / "scripts/profile-workspace-history.py")
history = importlib.util.module_from_spec(spec)
spec.loader.exec_module(history)


class DeferredHistory(history.WorkspaceHistory):
    def __init__(self, args):
        reference = json.loads(args.reference.read_text())
        require(reference["result"] == "measured", "Reference is not qualified")
        args.baseline = None
        super().__init__(args)
        self.reference = reference
        self.clock = reference["fixture_clock"]
        self.harness["scripts/profile-deferred-history.py"] = history.sha(Path(__file__).read_bytes())
        self.report.update(scope="Owned actual HTTP/PostgreSQL complete/deferred workspace and original history reads; no capacity SLA", fixture_clock=self.clock,
            reference={"path": str(args.reference), "sha256": history.sha(args.reference.read_bytes())},
            process_sampling={"interval_seconds": 0.2, "source": "Own API Linux procfs", "scope": "Sampled RSS/high water and thread CPU, not allocation attribution"})
        self.report["settings"].update(response_modes=["complete", "deferred"], fresh_api_per_mode=True,
            history_validation="All appointment/gate/event pages for first and last shipment; first two global event pages. Page limit100.")

    def archive(self, name, raw):
        path = self.directory / (name + ".json.gz")
        path.write_bytes(gzip.compress(raw, compresslevel=1, mtime=0))
        return {"path": str(path), "sha256": history.sha(path.read_bytes()), "raw_sha256": history.sha(raw)}

    def observed(self, path):
        stop, observations = threading.Event(), []
        def monitor():
            while not stop.is_set():
                try:
                    observations.append(history.profiled.process_sample(self.processes["freight"].pid))
                except Exception as error:
                    observations.append({"error": type(error).__name__})
                stop.wait(0.2)
        thread = threading.Thread(target=monitor, daemon=True)
        thread.start()
        try:
            sample, raw = self.request(("workspace", path, history.TOKEN, None, None))
        finally:
            stop.set()
            thread.join(10)
        require(not thread.is_alive() and observations and all("error" not in p for p in observations), "Process sampling failed")
        require(sample["status"] == 200 and sample["error"] is None, "Workspace read failed")
        return {**sample, "process_samples": observations}, raw

    def original_pages(self, workspace, prefix):
        results = []
        scopes = [(kind, load["id"]) for load in [workspace["loads"][0], workspace["loads"][-1]] for kind in ["events", "appointments", "gates"]]
        scopes.append(("events", None))
        for scope_index, (kind, load_id) in enumerate(scopes):
            field = {"events": "events", "appointments": "appointments", "gates": "stop_events"}[kind]
            identity = "entity_id" if kind == "events" else "load_id"
            expected = [{"position": i + 1, "record": row} for i, row in reversed(list(enumerate(workspace[field]))) if load_id is None or row[identity] == load_id]
            cursor, collected, pages = None, [], []
            while True:
                query = {"kind": kind, "limit": 100}
                if load_id is not None:
                    query["load_id"] = load_id
                if cursor:
                    query["cursor"] = cursor
                sample, raw = self.request(("history", "/v1/history?" + urlencode(query), history.TOKEN, None, None))
                archive = self.archive(f"{prefix}-history-{scope_index}-{len(pages)}", raw)
                require(sample["status"] == 200 and sample["error"] is None, "History read failed")
                page = json.loads(raw)
                require(page["tenant_id"] == "large" and page["access"] == {"subject": "user:capacity", "role": "operations"}, "History owner changed")
                require(page["kind"] == kind and page["load_id"] == load_id and page["total"] == len(expected) and page["has_new_records"] is False, "History scope changed")
                require(page["captured_through"] == len(workspace[field]) and len(page["items"]) <= 100, "History window changed")
                collected.extend(page["items"])
                require(collected == expected[:len(collected)], "Original history/order changed")
                pages.append({**sample, "archive": archive, "records": len(page["items"])})
                cursor = page["next_cursor"]
                if not cursor or load_id is None and len(pages) == 2:
                    break
                require(len(pages) <= len(expected) + 1, "History cursor failed to advance")
            if load_id is not None:
                require(collected == expected and cursor is None, "History originals missing")
            results.append({"kind": kind, "load_id": load_id, "scope": "complete selected history" if load_id else "first two global pages", "expected_total": len(expected), "verified_records": len(collected), "pages": pages})
        return results

    def checks(self):
        self.start_api()
        self.stop("freight")
        version = int(self.sql("freight_runtime", "SELECT version FROM freight_storage_format"))
        for index, (count, depth) in enumerate(self.report["settings"]["workloads"]):
            self.sql("freight_runtime", "TRUNCATE freight_load_search,freight_load_catalog,freight_workspaces,freight_document_content CASCADE")
            raw = history.subprocess.check_output([str(self.generator), str(count), f"workspace-{count}-{depth}", self.clock, str(depth)], cwd=self.directory)
            fixture = json.loads(raw)
            prior = self.reference["workloads"][index]
            workload = {"shipments": count, "history_depth": depth, "fixture_sha256": history.sha(raw), "counts": fixture["counts"], "fixture_archive": self.archive(f"fixture-{count}-{depth}", raw), "modes": {}}
            for key in ["shipments", "history_depth", "fixture_sha256", "counts"]:
                require(workload[key] == prior[key], "Reference fixture changed: " + key)
            workspace = fixture["workspace"]
            workspace["_freight_format"] = version
            self.command(["psql", f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime", "-X", "-v", "ON_ERROR_STOP=1"], label="owned dense fixture insert",
                input="INSERT INTO freight_workspaces(tenant_id,data) VALUES('large'," + literal(json.dumps(workspace)) + "::jsonb);")
            before = self.snapshot()
            self.report["workloads"].append(workload)
            complete = None
            for mode in ["complete", "deferred"]:
                self.start_api()
                records = []
                output = {"samples": records}
                workload["modes"][mode] = output
                for n in range(self.args.samples + 1):
                    sample, body = self.observed("/v1/workspace" + ("?history=deferred" if mode == "deferred" else ""))
                    sample["archive"] = self.archive(f"response-{count}-{depth}-{mode}-{n}", body)
                    self.save()
                    value = json.loads(body)
                    if mode == "complete":
                        normalized, generated = self.validate(value, workspace)
                        require(normalized == prior["normalized_response_sha256"], "Legacy complete response changed")
                        complete = value
                    else:
                        require(all(key not in value for key in ["events", "appointments", "stop_events"]), "Compact response returned full histories")
                        require(value.pop("history") == {"mode": "deferred", "events": len(workspace["events"]), "appointments": len(workspace["appointments"]), "gates": len(workspace["stop_events"])}, "History counts changed")
                        generated = value["operations"].pop("generated_at")
                        dt.datetime.fromisoformat(history.re.sub(r"(\.\d{6})\d+", r"\1", generated).replace("Z", "+00:00"))
                        retained = {k: v for k, v in complete.items() if k not in {"events", "appointments", "stop_events"}}
                        require(value == retained, "Current operating/finance/safety data changed")
                        normalized = history.stable(value)
                    sample.update(normalized_response_sha256=normalized, generated_at=generated)
                    if n == 0:
                        output["warmup"] = sample
                    else:
                        records.append(sample)
                    print(f"READ {count}/{depth} {mode} sample{n}: {sample['response_ms']:.1f}ms {sample['bytes']}bytes", flush=True)
                    self.save()
                times = [s["response_ms"] for s in records]
                output["latency_ms"] = {"min": min(times), "median": statistics.median(times), "max": max(times)}
                output["peak_sampled_rss_bytes"] = max(p["status"]["VmRSS"] for s in [output["warmup"], *records] for p in s["process_samples"])
                output["response_bytes"] = sorted({s["bytes"] for s in records})
                self.stop("freight")
            # Original-page checks are outside both modes' timing/memory samples.
            self.start_api()
            workload["original_history"] = self.original_pages(workspace, f"{count}-{depth}")
            self.stop("freight")
            after = self.snapshot()
            require(before == after, "Reads changed physical retained state")
            workload["unchanged_physical_snapshot"] = {"before": before, "after": after}
            self.save()
        self.report["qualification"] = {"functional_contracts": "pass", "capacity_sla": "not established", "limits": "Whole aggregate JSONB still transferred for workspace and scanned for histories. Record-count limits do not bound per-record bytes. No concurrent offered-load or dense-browser rendering qualification."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--reference", type=Path, default=ROOT / "docs/evidence/workspace-history-after.json")
    parser.add_argument("--samples", type=int, choices=[3, 5, 7, 9], default=7)
    parser.add_argument("--postgres-image", default="postgres:16")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Preserve the existing report; choose a new output path")
    run = DeferredHistory(args)
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    try:
        run.build()
        run.postgres()
        run.checks()
        require(run.source == fingerprint(ROOT), "Application changed during measurement")
        require(all(history.sha((ROOT / p).read_bytes()) == h for p, h in run.harness.items()), "Harness changed during measurement")
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
                run.report.update(result="failed", error="Missing/OOM database diagnostics")
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
