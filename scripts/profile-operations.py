#!/usr/bin/env python3
"""Profile owned synthetic operations workloads with the deployment build profile."""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess

from runtime_contracts import fingerprint

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--iterations", type=int, default=7, choices=range(3, 101, 2), help="odd sample count (3–99)")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; preserve the original evidence and select a new path")
    source = fingerprint(ROOT)
    runner_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    fixture_hash = hashlib.sha256((ROOT / "examples/operations_profile.rs").read_bytes()).hexdigest()
    host = {
        "platform": platform.platform(),
        "cpu_model": next((line.partition(":")[2].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown"),
        "logical_cpus": os.cpu_count(),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
    }
    baseline = json.loads(args.baseline.read_text()) if args.baseline else None
    if baseline is not None:
        assert baseline["schema"] == 1 and baseline["optimized"]
        assert baseline["provenance"]["fixture_sha256"] == fixture_hash, "fixture generator changed"
        assert baseline["iterations"] == args.iterations, "sample counts differ"
        for name, value in host.items():
            assert baseline["provenance"][name] == value, f"comparison environment changed: {name}"
    command = ["cargo", "run", "--offline", "--locked", "--release", "--example", "operations_profile", "--", str(args.iterations)]
    environment = dict(os.environ, RUSTC_WRAPPER="", CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="2", CARGO_TARGET_DIR=str(ROOT / "target"))
    started = dt.datetime.now(dt.timezone.utc).isoformat()
    result = subprocess.run(command, cwd=ROOT, env=environment, text=True, stdout=subprocess.PIPE, check=True)
    report = json.loads(result.stdout)
    if source != fingerprint(ROOT):
        raise RuntimeError("application source changed during profiling; do not use this run")
    if fixture_hash != hashlib.sha256((ROOT / "examples/operations_profile.rs").read_bytes()).hexdigest():
        raise RuntimeError("fixture changed during profiling; do not use this run")
    if runner_hash != hashlib.sha256(Path(__file__).read_bytes()).hexdigest():
        raise RuntimeError("runner changed during profiling; do not use this run")
    assert report["optimized"], "profiling requires an optimized release build"
    report["provenance"] = {
        "started_at": started,
        "finished_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "source": source,
        "fixture_sha256": fixture_hash,
        "runner_sha256": runner_hash,
        "command": command,
        **host,
        "binary_sha256": hashlib.sha256((ROOT / "target/release/examples/operations_profile").read_bytes()).hexdigest(),
    }
    if baseline is not None:
        assert baseline["schema"] == report["schema"] and baseline["optimized"]
        assert baseline["provenance"]["fixture_sha256"] == fixture_hash, "fixture generator changed"
        assert baseline["iterations"] == report["iterations"], "sample counts differ"
        assert len(baseline["results"]) == len(report["results"]), "workload count changed"
        comparisons = []
        for before, after in zip(baseline["results"], report["results"]):
            for key in ("loads", "notes_per_case", "cases", "history_events", "workspace_json_bytes", "fixture_sha256"):
                assert before[key] == after[key], f"workload changed: {key}"
            assert before["measurements"].keys() == after["measurements"].keys(), "measurements changed"
            times = {}
            for name, current in after["measurements"].items():
                prior = before["measurements"][name]
                assert current["result_sha256"] == prior["result_sha256"], f"projection changed: {name}"
                times[name] = {"before_median_ms": prior["median_ms"], "after_median_ms": current["median_ms"], "ratio": prior["median_ms"] / current["median_ms"]}
            comparisons.append({"loads": after["loads"], "notes_per_case": after["notes_per_case"], "measurements": times})
        report["comparison"] = {"baseline_source": baseline["provenance"]["source"], "baseline_report_sha256": hashlib.sha256(args.baseline.read_bytes()).hexdigest(), "identical_fixtures_and_projection_results": True, "results": comparisons}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as output:
        output.write(json.dumps(report, indent=2) + "\n")
    print(f"REPORT {args.output}")
    for row in report["results"]:
        print(row["loads"], "shipments;", row["notes_per_case"], "notes/case;", ", ".join(f'{name}={v["median_ms"]:.2f}ms' for name, v in row["measurements"].items()))


if __name__ == "__main__":
    main()
