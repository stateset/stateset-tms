#!/usr/bin/env python3
"""Run the retained HTTP workload with Linux process/thread memory observations.

Adds sampling overhead; compare runs of this driver with one another. No allocator
settings, workload inputs, server budgets or validation rules are changed.
"""
import importlib.util
import os
from pathlib import Path
import threading
import time

from runtime_contracts import ROOT, require

spec = importlib.util.spec_from_file_location(
    "service_capacity", ROOT / "scripts/qualify-service-capacity.py"
)
capacity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capacity)


def process_sample(pid):
    base = Path(f"/proc/{pid}")
    status = {}
    for line in (base / "status").read_text().splitlines():
        name, _, value = line.partition(":")
        if name in {"Threads", "VmRSS", "VmHWM", "RssAnon", "RssFile", "VmSize"}:
            status[name] = int(value.split()[0]) * (1024 if "kB" in value else 1)
    rollup = {}
    for line in (base / "smaps_rollup").read_text().splitlines():
        name, _, value = line.partition(":")
        if name in {"Rss", "Pss", "Anonymous", "Private_Dirty", "Private_Clean", "Swap"}:
            rollup[name + "_bytes"] = int(value.split()[0]) * 1024
    threads = []
    for task in sorted((base / "task").iterdir()):
        try:
            fields = (task / "stat").read_text().rsplit(")", 1)[1].split()
            threads.append({
                "tid": int(task.name),
                "name": (task / "comm").read_text().strip(),
                "cpu_seconds": (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK"),
            })
        except FileNotFoundError:
            # Threads can exit between listing and observation.
            continue
    return {"at": time.monotonic(), "status": status, "memory": rollup,
            "mapping_count": len((base / "maps").read_text().splitlines()), "threads": threads}


class ProfiledCapacity(capacity.Capacity):
    def __init__(self, args):
        super().__init__(args)
        path = "scripts/profile-service-reads.py"
        self.harness[path] = capacity.digest((ROOT / path).read_bytes())
        self.report["process_sampling"] = {
            "interval_seconds": 0.2,
            "source": "Linux procfs status/smaps_rollup/maps/task; own API process only",
            "scope": "resident/high-water/anonymous memory and thread CPU; no allocation attribution",
            "sampling_overhead": "additional to the base workload; not measured separately",
        }

    def wave(self, name, specs, concurrency):
        stop = threading.Event()
        samples = []

        def observe():
            while not stop.is_set():
                try:
                    samples.append(process_sample(self.processes["freight"].pid))
                except (OSError, ValueError, IndexError) as error:
                    samples.append({"error": type(error).__name__})
                stop.wait(0.2)

        sampler = threading.Thread(target=observe, daemon=True)
        sampler.start()
        try:
            wave, results = super().wave(name, specs, concurrency)
        finally:
            stop.set()
            sampler.join(10)
        require(not sampler.is_alive(), "Process sampler did not stop")
        require(samples and all("error" not in s for s in samples), "Process sampling failed")
        wave["process_samples"] = samples
        wave["process_after"] = process_sample(self.processes["freight"].pid)
        return wave, results


if __name__ == "__main__":
    capacity.Capacity = ProfiledCapacity
    raise SystemExit(capacity.main())
