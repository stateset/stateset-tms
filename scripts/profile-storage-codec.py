#!/usr/bin/env python3
"""Measure the qualified native block codec without the PostgreSQL observer.

Runs /usr/bin/time against the exact release executable and archived originals
from a successful storage-blocks probe. These are CLI CPU/RSS observations, not
embedded API memory, complete typed Workspace decoding, capacity or an SLA.
"""
import argparse
import datetime as dt
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from runtime_contracts import ROOT, require

spec = importlib.util.spec_from_file_location("record_probe", ROOT / "scripts/profile-storage-records.py")
records = importlib.util.module_from_spec(spec)
spec.loader.exec_module(records)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def binding(path):
    return {"path": str(path.resolve()), "sha256": sha(path)}


def measure(binary, mode, span, source, target, directory, label):
    log = directory / (label + ".time")
    stderr = directory / (label + ".stderr")
    command = ["/usr/bin/time", "-v", "-o", str(log), str(binary), mode]
    if mode == "pack":
        command += [str(span), "65536"]
    with source.open("rb") as stdin, target.open("xb") as stdout, stderr.open("xb") as error:
        completed = subprocess.run(command, stdin=stdin, stdout=stdout, stderr=error,
                                   env={**os.environ, "LC_ALL": "C"})
    require(completed.returncode == 0, f"Native codec {mode} failed; inspect {stderr}")
    fields = dict(line.strip().split(": ", 1) for line in log.read_text().splitlines() if ": " in line)
    return {
        "mode": mode,
        "exit_code": completed.returncode,
        "user_cpu_seconds": float(fields["User time (seconds)"]),
        "system_cpu_seconds": float(fields["System time (seconds)"]),
        "maximum_rss_bytes": int(fields["Maximum resident set size (kbytes)"]) * 1024,
        "elapsed_wall_clock": fields["Elapsed (wall clock) time (h:mm:ss or m:ss)"],
        "input": binding(source), "output": binding(target),
        "time_log": binding(log), "stderr": binding(stderr),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--storage-report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repetitions", type=int, default=3, choices=range(1, 11))
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Preserve evidence; select a new output path")
    directory = Path(tempfile.mkdtemp(prefix="freight-storage-codec-resources-"))
    controller_paths = [Path(__file__).resolve(), ROOT / "scripts/profile-storage-records.py",
                        ROOT / "scripts/runtime_contracts.py"]
    report = {
        "result": "running", "scope": __doc__,
        "started_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "artifacts": str(directory), "repetitions": args.repetitions,
        "controllers": {str(p.relative_to(ROOT)): binding(p) for p in controller_paths},
        "results": [],
    }
    try:
        run = json.loads(args.storage_report.read_text())
        require(run["result"] == "measured" and run["no_oom_or_limit_hits"],
                "Storage probe must be complete and resource-qualified")
        report["storage_report"] = binding(args.storage_report)
        binary = Path(run["native_codec"]["path"])
        require(sha(binary) == run["native_codec"]["sha256"], "Native binary drift")
        report["native_codec"] = binding(binary)
        for workload in run["workloads"]:
            archive = workload["samples"][0]["native_before"]
            path = Path(archive["path"])
            require(sha(path) == archive["sha256"], "Archived source changed")
            raw = gzip.decompress(path.read_bytes())
            require(hashlib.sha256(raw).hexdigest() == archive["raw_sha256"], "Raw original changed")
            original = records.encode(records.decode(raw))
            shipments = workload["shipments"]
            source = directory / f"source-{shipments}.json"
            source.write_bytes(raw)
            for repetition in range(args.repetitions):
                for span in ([16, 64] if repetition % 2 == 0 else [64, 16]):
                    label = f"{shipments}-{span}-{repetition}"
                    frame = directory / (label + "-frame.json")
                    restored = directory / (label + "-restored.json")
                    for mode, inp, target in [("pack", source, frame), ("unpack", frame, restored)]:
                        result = measure(binary, mode, span, inp, target, directory, label + "-" + mode)
                        result.update(shipments=shipments, span=span, repetition=repetition)
                        report["results"].append(result)
                    require(records.encode(records.decode(restored.read_bytes())) == original,
                            "Native CLI resource observation changed originals")
                    print("MEASURED", shipments, span, repetition, flush=True)
        require(sha(binary) == report["native_codec"]["sha256"], "Final binary drift")
        require(all(sha(Path(v["path"])) == v["sha256"] for v in report["controllers"].values()),
                "Resource controller drift")
        require(sha(args.storage_report) == report["storage_report"]["sha256"], "Storage report drift")
        report["result"] = "pass"
    except Exception as error:
        report.update(result="failed", error=f"{type(error).__name__}: {error}")
        print(report["error"], file=sys.stderr, flush=True)
    finally:
        report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        with args.output.open("x") as output:
            output.write(json.dumps(report, indent=2) + "\n")
        print("REPORT", args.output, flush=True)
    return 0 if report["result"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
