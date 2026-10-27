#!/usr/bin/env python3
"""Compare exact codec outputs and counterbalanced CLI CPU/RSS observations.

These component measurements do not establish API latency, deployment capacity,
or production memory bounds. Every output, timing and unfavorable result stays
in a fresh artifact directory. Inputs and executables must remain unchanged.
"""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time


def binding(path):
    path = path.resolve()
    return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--input", type=Path, action="append", required=True)
    parser.add_argument("--rounds", type=int, default=2, choices=range(1, 11))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(not args.output.exists(), "Select a fresh report path; preserve prior evidence")
    directory = Path(tempfile.mkdtemp(prefix="freight-codec-comparison-"))
    harness_archive = directory / "harness.py"
    harness_archive.write_bytes(Path(__file__).read_bytes())
    binaries = {"baseline": args.baseline.resolve(), "candidate": args.candidate.resolve()}
    report = {
        "result": "running", "scope": __doc__,
        "started_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "harness": binding(Path(__file__)),
        "harness_archive": binding(harness_archive),
        "binaries": {name: binding(path) for name, path in binaries.items()},
        "inputs": [binding(path) for path in args.input],
        "artifacts": str(directory), "rounds": args.rounds,
        "host_at_start": {"logical_cpus": os.cpu_count(), "load_average": os.getloadavg()},
        "order_per_round": ["baseline", "candidate", "candidate", "baseline"],
        "samples": [], "comparisons": [],
    }

    def execute(variant, mode, source, label, measured=True):
        output = directory / (label + ".json")
        stderr = directory / (label + ".stderr")
        timing = directory / (label + ".time")
        command = [str(binaries[variant]), mode]
        if mode == "pack":
            command.extend(["64", "65536"])
        if measured:
            command = ["/usr/bin/time", "-f", "%U %S %M", "-o", str(timing), *command]
        started = time.monotonic()
        with source.open("rb") as inp, output.open("xb") as out, stderr.open("xb") as err:
            completed = subprocess.run(command, stdin=inp, stdout=out, stderr=err,
                                       env={**os.environ, "LC_ALL": "C"})
        require(completed.returncode == 0, f"Codec failed: inspect {stderr}")
        sample = {"variant": variant, "mode": mode, "wall_seconds": time.monotonic() - started,
                  "output": binding(output), "stderr": binding(stderr)}
        if measured:
            user, system, rss = timing.read_text().split()
            sample.update(user_cpu_seconds=float(user), system_cpu_seconds=float(system),
                          maximum_rss_bytes=int(rss) * 1024, timing=binding(timing))
        return output, sample

    try:
        for index, source in enumerate(args.input):
            source = source.resolve()
            frame, _ = execute("baseline", "pack", source, f"{index}-reference-frame", False)
            restored, _ = execute("baseline", "unpack", frame, f"{index}-reference-original", False)
            expected = {"pack": binding(frame)["sha256"], "unpack": binding(restored)["sha256"]}
            for round_number in range(args.rounds):
                for order, variant in enumerate(report["order_per_round"]):
                    for mode, data in [("pack", source), ("unpack", frame)]:
                        label = f"{index}-{round_number}-{order}-{variant}-{mode}"
                        _, sample = execute(variant, mode, data, label)
                        sample.update(input=index, round=round_number, order=order)
                        report["samples"].append(sample)
                        require(sample["output"]["sha256"] == expected[mode], "Exact codec output changed: " + label)
            for mode in ["pack", "unpack"]:
                medians = {}
                for variant in binaries:
                    samples = [s for s in report["samples"] if s["input"] == index
                               and s["variant"] == variant and s["mode"] == mode]
                    medians[variant] = {field: statistics.median(s[field] for s in samples)
                                        for field in ["user_cpu_seconds", "system_cpu_seconds",
                                                      "maximum_rss_bytes", "wall_seconds"]}
                report["comparisons"].append({"input": index, "mode": mode, "medians": medians,
                    "user_cpu_reduction_fraction": (1 - medians["candidate"]["user_cpu_seconds"] /
                        medians["baseline"]["user_cpu_seconds"])
                        if medians["baseline"]["user_cpu_seconds"] else None})
            print("VERIFIED input", index, "exact outputs and retained resource observations", flush=True)
        require(report["binaries"] == {name: binding(path) for name, path in binaries.items()}, "Executable changed")
        require(report["inputs"] == [binding(path) for path in args.input], "Input changed")
        require(report["harness"] == binding(Path(__file__)), "Harness changed")
        report["result"] = "pass"
    except Exception as error:
        report.update(result="failed", error=f"{type(error).__name__}: {error}")
        print(report["error"], flush=True)
    finally:
        report["host_at_finish"] = {"logical_cpus": os.cpu_count(), "load_average": os.getloadavg()}
        report["finished_utc"] = dt.datetime.now(dt.timezone.utc).isoformat()
        with args.output.open("x") as output:
            output.write(json.dumps(report, indent=2) + "\n")
        print("REPORT", args.output, flush=True)
    return 0 if report["result"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
