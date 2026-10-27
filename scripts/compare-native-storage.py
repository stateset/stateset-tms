#!/usr/bin/env python3
"""Verify and summarize counterbalanced native storage observations.

Requires four complete runs, in before/after/after/before order. Rehashes raw
archives and checks original results. Failing runs remain evidence, but cannot
be summarized as a successful comparison.
"""
import argparse
import datetime as dt
import gzip
import hashlib
import json
import re
from pathlib import Path
import statistics
import sys

from runtime_contracts import ROOT, fingerprint, require


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runs", type=Path, nargs=4, required=True)
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument("--protected", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(not args.output.exists(), "Preserve prior comparison evidence")
    require(len({p.resolve() for p in args.runs}) == 4, "Four distinct run reports are required")
    inputs = json.loads(args.inventory.read_text())
    protected = json.loads(args.protected.read_text())
    require(sha(args.inventory) == protected["input_manifest_sha256"], "Input manifest changed")
    require(fingerprint(ROOT) == inputs["after"]["source"], "Application source changed")
    for path, expected in protected["qualified_sources"].items():
        require(sha(ROOT / path) == expected, "Qualified input changed: " + path)
    for path, expected in protected["historical_evidence"].items():
        require(sha(ROOT / path) == expected, "Historical evidence changed: " + path)
    require(sha(ROOT / protected["profile_driver"]["path"]) == protected["profile_driver"]["sha256"], "Profile driver changed")
    archive_count = 0

    def unpack(record):
        nonlocal archive_count
        path = Path(record["path"])
        require(sha(path) == record["sha256"], "Compressed archive changed")
        raw = gzip.decompress(path.read_bytes())
        require(hashlib.sha256(raw).hexdigest() == record["raw_sha256"], "Raw archive changed")
        archive_count += 1
        return raw, json.loads(raw)

    fixtures, reads, commands = {}, {}, {}
    summaries = []
    environment = None
    previous_finished = None
    for index, (generation, path) in enumerate(zip(["before", "after", "after", "before"], args.runs)):
        run = json.loads(path.read_text())
        started = dt.datetime.fromisoformat(run["started_at"])
        finished = dt.datetime.fromisoformat(run["finished_at"])
        require(finished > started and (previous_finished is None or started >= previous_finished), "Run order overlaps or timestamps are invalid")
        previous_finished = finished
        require(run["result"] == "measured" and run["no_oom_or_limit_hits"] is True, "Incomplete or resource-failed run: " + str(path))
        require(run["application_generation"] == generation, "Run order changed")
        build = run["builds"]["freight"]
        require(build["source"] == inputs[generation]["source"] and build["binary_sha256"] == inputs[generation]["binary_sha256"] and sha(Path(build["path"])) == build["binary_sha256"], "Run executable/source mismatch")
        require(run["builds"]["fixture"] == inputs["fixture"], "Fixture identity changed")
        if "initializer" in inputs:
            require(run["builds"]["initializer"] == inputs["initializer"], "Initializer identity changed")
        require(run["fixture_clock"] == inputs["clock"], "Fixture clock changed")
        observed_environment = {k: run[k] for k in ["settings", "host", "harness", "postgres", "reference", "attribution"]}
        environment = environment or observed_environment
        require(observed_environment == environment, "Host, resource budgets, fixture reference or harness differ")
        require(run["settings"]["timed_samples"] == 5 and run["settings"]["workloads"] == [[100, 4], [1000, 4]], "Workload changed")
        for rel, expected in run["harness"].items():
            require(sha(ROOT / rel) == expected, "Measured harness changed: " + rel)
        diagnostic = run["postgres_diagnostics"]
        require(diagnostic["state"]["OOMKilled"] is False, "Database OOM")
        if diagnostic["cgroup_version"] == 1:
            events = dict(line.split() for line in diagnostic["memory.oom_control"].splitlines())
            require(int(diagnostic["memory.failcnt"]) == 0 and int(events["oom_kill"]) == 0, "Final database memory limit/OOM counter changed")
        else:
            require(diagnostic["memory_events"]["max"] == 0 and diagnostic["memory_events"]["oom_kill"] == 0, "Final database memory limit/OOM counter changed")
        require(run["continuous_memory_samples"] and all("error" not in s and s["memory"]["limit_hits"] == 0 for s in run["continuous_memory_samples"]), "Incomplete continuous PostgreSQL monitoring")
        api = run["api_samples"]
        require(api and all("error" not in s for s in api), "Incomplete API monitoring")
        output = {"generation": generation, "run_index": index, "report": {"path": str(path), "sha256": sha(path)}, "source": build["source"], "api_peak_sampled_rss_bytes": max(s["status"]["VmRSS"] for s in api), "postgres_sampled_peak_bytes": max(s["memory"]["usage_bytes"] for s in run["continuous_memory_samples"]), "postgres_kernel_historical_peak_bytes": int(diagnostic["memory.max_usage_in_bytes"]) if diagnostic["cgroup_version"] == 1 else None, "workloads": []}
        require(len(run["workloads"]) == 2, "Missing workload")
        for w in run["workloads"]:
            scope = (w["shipments"], w["history_depth"])
            require(w["result"] == "measured" and w["exact_startup_originals"] and w["reads_preserve_physical_source"], "Incomplete workload")
            fixture_raw, fixture = unpack(w["fixture_archive"])
            fixture_sha = hashlib.sha256(fixture_raw).hexdigest()
            require(fixtures.setdefault(scope, fixture_sha) == fixture_sha, "Fixture bytes differ")
            initial_raw, initial = unpack(w["source_pre_startup"])
            require(initial_raw.count(b'"_freight_format": 18') == 1, "Ambiguous original marker")
            migrated_raw, migrated = unpack(w["source_post_startup"])
            storage_format = inputs[generation].get("storage_format", 18 if generation == "before" else 19)
            require(storage_format in {18, 19}, "Unsupported measured storage format")
            require(migrated_raw == (initial_raw.replace(b'"_freight_format": 18', b'"_freight_format": 19') if storage_format == 19 else initial_raw), "Migration changed original JSONB beyond marker")
            require(initial["loads"] == fixture["workspace"]["loads"], "Inserted fixture differs")
            metrics = {}
            require(len(w["reads"]) == 5, "Missing read case")
            for kind, case in w["reads"].items():
                require(len(case["samples"]) == 5, "Missing read samples")
                for sample in [case["first"], *case["samples"]]:
                    require(sample["status"] == 200 and sample["error"] is None, "Read failed")
                    _, value = unpack(sample["archive"])
                    if kind in {"workspace", "deferred_workspace"}:
                        stamp = value["operations"].pop("generated_at")
                        require(isinstance(stamp, str) and stamp.endswith("Z"), "Invalid volatile timestamp")
                        dt.datetime.fromisoformat(re.sub(r"(\.\d{6})\d+", lambda match: match.group(1), stamp).replace("Z", "+00:00"))
                        if kind == "deferred_workspace":
                            require(value.pop("history") == {"mode": "deferred", "events": len(migrated["events"]), "appointments": len(migrated["appointments"]), "gates": len(migrated["stop_events"])}, "Deferred counts changed")
                    stable = hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
                    require(stable == sample["normalized_response_sha256"], "Recorded read hash differs from archived result")
                    read_key = (*scope, kind)
                    require(reads.setdefault(read_key, stable) == stable, "Before/after read business output differs: " + str(read_key))
                metrics[kind] = {"first_ms": case["first"]["response_ms"], "median_ms": statistics.median(s["response_ms"] for s in case["samples"]), "median_api_cpu_seconds": statistics.median(s["api_cpu_seconds"] for s in case["samples"])}
            require(len(w["saves"]) == 5, "Missing new/replay pairs")
            for n, record in enumerate(w["saves"]):
                require(record["verification"] == "pass", "Unverified save")
                _, before = unpack(record["source_before"])
                _, after = unpack(record["source_after"])
                _, result = unpack(record["new_location"]["archive"])
                _, replay = unpack(record["original_replay"]["archive"])
                require(replay == {"data": result["data"], "replayed": True}, "Retry changed original result")
                require(result.get("replayed") is not True and result["data"]["version"] == record["request"]["expected_version"] + 1 and result["data"]["location"] == record["request"]["location"], "New observation changed")
                require(after["receipts"][record["key"]]["response"] == result["data"] and len(after["receipts"]) == len(before["receipts"]) + 1, "Stored receipt changed")
                require(all(after["receipts"][k] == v for k, v in before["receipts"].items()), "Prior receipt changed")
                require(after["events"][:-1] == before["events"] and set(after) == set(before), "Audit/root retention failed")
                for field in set(before) - {"loads", "events", "receipts", "operations_cases"}:
                    require(after[field] == before[field], "Unrelated business root changed")
                require(len(before["loads"]) == len(after["loads"]), "Shipment membership changed")
                for old, new in zip(before["loads"], after["loads"]):
                    allowed = {"version", "location", "updated_at", "last_tracking_at"} if old["id"] == result["data"]["id"] else set()
                    require(set(old) == set(new) and all(new[k] == v for k, v in old.items() if k not in allowed), "Unrelated shipment fact changed")
                comparable = dict(result["data"])
                for field in ["updated_at", "last_tracking_at"]:
                    stamp = comparable.pop(field)
                    require(isinstance(stamp, str) and stamp.endswith("Z"), "Invalid live tracking timestamp")
                command_key = (*scope, n)
                require(commands.setdefault(command_key, comparable) == comparable, "Before/after command outcome differs")
                if storage_format == 19:
                    require(0 < record["physical_delta"]["changed_blocks"] < record["physical_delta"]["total_blocks"], "Physical block delta absent")
            metrics.update(w["save_summary"])
            points = [s for s in api if s.get("phase", "").startswith(w["prefix"])]
            output["workloads"].append({"shipments": scope[0], "history_depth": scope[1], "startup": w["startup"], "metrics": metrics, "api_peak_sampled_during_requests_or_startup_bytes": max(s["status"]["VmRSS"] for s in points), "disk_before_startup_bytes": w["disk_before_startup"]["total_bytes"], "disk_after_startup_bytes": w["disk_after_startup"]["total_bytes"], "disk_after_requests_bytes": w["disk_after_requests"]["total_bytes"], "physical_deltas": [r.get("physical_delta") for r in w["saves"]]})
        summaries.append(output)
    report = {"result": "verified_observations", "recorded_utc": dt.datetime.now(dt.timezone.utc).isoformat(), "scope": "Four counterbalanced finite native HTTP/PostgreSQL runs, before/after/after/before; 100 and 1,000 shipments with depth-4 history. Five timed reads per case after one first observation, five new saves and five exact replays per workload. All production triggers/workers enabled. No capacity or deployment SLA.", "source": inputs["after"]["source"], "storage_formats": {k: inputs[k].get("storage_format", 18 if k == "before" else 19) for k in ["before", "after"]}, "runs": summaries, "archives_rehashed_and_parsed": archive_count, "distinct_input_fixtures": len(fixtures), "matched_read_business_outputs": len(reads), "matched_command_business_outcomes": len(commands), "historical_evidence_unchanged": len(protected["historical_evidence"]), "qualified_source_inputs_unchanged": len(protected["qualified_sources"]), "inventory": {"path": str(args.inventory), "sha256": sha(args.inventory)}, "protected": {"path": str(args.protected), "sha256": sha(args.protected)}, "comparator": {"path": str(Path(__file__).resolve()), "sha256": sha(Path(__file__))}, "limits": ["Finite sequential synthetic histories on a shared host; monitoring/controller work affects cadence and cache state. Not sustained arrivals, contention, deployment capacity or independent operator evidence.", "Startup includes process creation, native migration, worker startup and readiness polling. API CPU includes concurrent workers. This does not isolate codec or SQL cost.", "Disk includes all public tables, TOAST and indexes; live/dead tuple versions and background maintenance affect size. Histories are compressible. Both workloads share each bounded cluster, so its cumulative kernel memory peak is not attributable to one request or workload.", "API runs outside the PostgreSQL CPU/memory quota. Sampled RSS and process high water do not establish an allocation or embedded-process memory bound.", "Only operations.generated_at is removed from workspace comparisons. Only updated_at and last_tracking_at are removed from cross-run new-command results. Stored originals and each run's exact retries remain unnormalized."]}
    with args.output.open("x") as out:
        out.write(json.dumps(report, indent=2) + "\n")
    print("Verified", archive_count, "archives across four native runs; report", args.output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
