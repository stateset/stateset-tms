#!/usr/bin/env python3
"""Attribute owned PostgreSQL memory across the exact history HTTP workload.

Kernel cgroup and process-status samples are taken without additional SQL
connections. Sampling changes request cadence; these are attribution observations,
not a latency comparison, allocator profile, capacity claim or production probe.
"""

import argparse
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time

from runtime_contracts import ROOT, fingerprint, require

spec = importlib.util.spec_from_file_location(
    "indexed_history", ROOT / "scripts/profile-indexed-history.py"
)
indexed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(indexed)


class HistoryMemory(indexed.IndexedHistory):
    def __init__(self, args):
        super().__init__(args)
        self.trace_enabled = False
        self.phases = []
        self.global_samples = []
        self.active_phase = None
        self.sample_lock = threading.Lock()
        self.monitor_stop = threading.Event()
        self.monitor_thread = None
        self.harness["scripts/profile-history-memory.py"] = indexed.history.sha(
            Path(__file__).read_bytes()
        )
        self.report.update(
            scope=__doc__,
            memory_phases=self.phases,
            continuous_memory_samples=self.global_samples,
            application_generation=args.generation,
            attribution={
                "sampling_interval_seconds": 0.2,
                "postgres_memory_mib": 768,
                "sources": [
                    "owned container cgroup memory counters",
                    "owned postmaster/child Linux procfs status",
                ],
                "limits": "Extra Docker exec calls and gaps alter cadence. RSS includes shared pages; do not sum process RSS as private memory. Cgroup file/shmem counters overlap.",
            },
        )

    def build(self):
        baseline = self.args.inventory.parent
        if self.args.generation == "before":
            self.binaries = {"freight": baseline / "release"}
            api_source = self.inventory["core"]
            for relative, expected in self.inventory["source_files_sha256"].items():
                require(
                    indexed.history.sha((baseline / "source" / relative).read_bytes())
                    == expected,
                    "Baseline source changed",
                )
            require(
                indexed.history.sha(self.binaries["freight"].read_bytes())
                == self.inventory["binaries"]["release"],
                "Baseline API changed",
            )
        else:
            super().build()
            api_source = self.source
        self.generator = baseline / "fixture"
        self.report["builds"] = {
            "freight": {
                "source": api_source,
                "binary_sha256": indexed.history.sha(
                    self.binaries["freight"].read_bytes()
                ),
            },
            "fixture": {
                "binary_sha256": indexed.history.sha(self.generator.read_bytes())
            },
        }
        require(
            self.report["builds"]["fixture"]["binary_sha256"]
            == self.inventory["binaries"]["fixture"],
            "Fixture executable changed",
        )

    def docker_read(self, paths):
        result = subprocess.run(
            [
                "docker",
                "--host",
                "unix:///var/run/docker.sock",
                "exec",
                self.container,
                "cat",
                *paths,
            ],
            env={"PATH": os.defpath, "LC_ALL": "C"},
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=10,
        )
        require(
            result.returncode == 0, "Owned cgroup read failed: " + result.stderr[-500:]
        )
        return result.stdout.splitlines()

    def prepare_sampling(self):
        state = json.loads(
            self.command(
                ["docker", "inspect", "--format", "{{json .State}}", self.container],
                label="owned postmaster identity",
            )
        )
        self.postmaster = int(state["Pid"])
        require(
            self.postmaster > 1 and Path(f"/proc/{self.postmaster}/status").exists(),
            "Owned postmaster not visible",
        )
        # Current qualified runtime uses v1; preserve both layouts explicitly.
        probe = subprocess.run(
            [
                "docker",
                "--host",
                "unix:///var/run/docker.sock",
                "exec",
                self.container,
                "cat",
                "/sys/fs/cgroup/memory.current",
            ],
            env={"PATH": os.defpath, "LC_ALL": "C"},
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.cgroup_version = 2 if probe.returncode == 0 else 1
        self.report["attribution"]["cgroup_version"] = self.cgroup_version
        self.trace_enabled = True

        def monitor():
            while not self.monitor_stop.wait(0.2):
                try:
                    sample = self.sample()
                except Exception as error:
                    sample = {"error": str(error)}
                with self.sample_lock:
                    phase = self.active_phase
                    self.global_samples.append(
                        {
                            "phase": phase["name"] if phase else "between_requests",
                            **sample,
                        }
                    )
                    if phase is not None:
                        phase["samples"].append(sample)

        self.monitor_thread = threading.Thread(target=monitor, daemon=True)
        self.monitor_thread.start()

    def stop_sampling(self):
        self.monitor_stop.set()
        if self.monitor_thread is not None:
            self.monitor_thread.join(15)
            require(
                not self.monitor_thread.is_alive(),
                "Global memory sampler did not finish",
            )
        require(
            all("error" not in v for v in self.global_samples),
            "Global memory sampling failed",
        )

    def sample(self):
        started = time.monotonic()
        if self.cgroup_version == 1:
            lines = self.docker_read(
                [
                    "/sys/fs/cgroup/memory/" + name
                    for name in [
                        "memory.usage_in_bytes",
                        "memory.max_usage_in_bytes",
                        "memory.failcnt",
                        "memory.limit_in_bytes",
                        "memory.stat",
                        "memory.oom_control",
                    ]
                ]
            )
            values = dict((k, int(v)) for k, v in (line.split() for line in lines[4:]))
            memory = {
                "usage_bytes": int(lines[0]),
                "historical_peak_bytes": int(lines[1]),
                "limit_hits": int(lines[2]),
                "limit_bytes": int(lines[3]),
                "stat": values,
            }
        else:
            lines = self.docker_read(
                [
                    "/sys/fs/cgroup/" + name
                    for name in [
                        "memory.current",
                        "memory.max",
                        "memory.stat",
                        "memory.events",
                    ]
                ]
            )
            values = dict((k, int(v)) for k, v in (line.split() for line in lines[2:]))
            memory = {
                "usage_bytes": int(lines[0]),
                "limit_bytes": int(lines[1]),
                "limit_hits": values["max"],
                "stat": values,
            }
        root = Path(f"/proc/{self.postmaster}")
        children = [
            int(v)
            for v in (root / "task" / str(self.postmaster) / "children")
            .read_text()
            .split()
        ]
        processes = []
        for pid in [self.postmaster, *children]:
            try:
                status = {}
                for line in Path(f"/proc/{pid}/status").read_text().splitlines():
                    key, _, value = line.partition(":")
                    if key in ["VmRSS", "RssAnon", "RssFile", "RssShmem", "VmHWM"]:
                        status[key] = int(value.split()[0]) * 1024
                    elif key == "NSpid":
                        status["namespace_pids"] = [int(v) for v in value.split()]
                processes.append(
                    {
                        "host_pid": pid,
                        "name": Path(f"/proc/{pid}/comm").read_text().strip(),
                        "process_title": Path(f"/proc/{pid}/cmdline")
                        .read_bytes()
                        .replace(b"\0", b" ")
                        .decode(errors="replace")[:240],
                        "status": status,
                    }
                )
            except FileNotFoundError:
                # A backend may exit between child enumeration and status read.
                continue
        return {
            "at_monotonic": started,
            "sampling_ms": (time.monotonic() - started) * 1000,
            "memory": memory,
            "processes": processes,
        }

    def phase(self, name, action, **extra):
        if not self.trace_enabled:
            return action()
        phase = {"name": name, **extra, "samples": [self.sample()]}
        self.phases.append(phase)
        with self.sample_lock:
            self.active_phase = phase
        try:
            result = action()
            phase["result"] = "completed"
            return result
        except BaseException as error:
            phase.update(result="failed", error=self.safe(str(error)))
            raise
        finally:
            with self.sample_lock:
                self.active_phase = None
            phase["samples"].append(self.sample())
            require(
                all("error" not in s for s in phase["samples"]),
                "Memory sampling failed",
            )
            phase["peak_sampled_usage_bytes"] = max(
                s["memory"]["usage_bytes"] for s in phase["samples"]
            )
            phase["new_limit_hits"] = (
                phase["samples"][-1]["memory"]["limit_hits"]
                - phase["samples"][0]["memory"]["limit_hits"]
            )
            self.save()

    def request(self, spec, queued=None):
        return self.phase(
            "http:" + spec[0],
            lambda: super(HistoryMemory, self).request(spec, queued),
            path=spec[1],
        )

    def command(self, command, *, label, input=None, env=None, timeout=600):
        name = "command:" + label
        if command[0] == "psql" and "-c" in command:
            sql = command[command.index("-c") + 1]
            if "md5(data::text)" in sql:
                name = "observer:complete_source_hash"
            elif "SELECT data FROM freight_workspaces" in sql:
                name = "observer:complete_source_json"
        return self.phase(
            name,
            lambda: super(HistoryMemory, self).command(
                command, label=label, input=input, env=env, timeout=timeout
            ),
        )

    def stop(self, name):
        return self.phase("stop:" + name, lambda: super(HistoryMemory, self).stop(name))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--generation", choices=["before", "after"], default="after")
    parser.add_argument(
        "--inventory",
        type=Path,
        default=Path("/tmp/freight-history-memory-baseline/inventory.json"),
    )
    parser.add_argument(
        "--reference",
        type=Path,
        default=ROOT / "docs/evidence/workspace-history-after.json",
    )
    parser.add_argument("--samples", type=int, choices=[3, 5, 7, 9], default=7)
    parser.add_argument("--postgres-image", default="postgres:16")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Preserve previous evidence; choose a new output path")
    args.variant = "after"
    run = HistoryMemory(args)

    def interrupted(_signum, _frame):
        raise KeyboardInterrupt()

    signal.signal(signal.SIGTERM, interrupted)
    try:
        run.build()
        run.postgres()
        run.prepare_sampling()
        run.checks()
        require(
            fingerprint(ROOT) == run.source, "Application changed during attribution"
        )
        require(
            all(
                indexed.history.sha((ROOT / p).read_bytes()) == h
                for p, h in run.harness.items()
            ),
            "Harness changed",
        )
        run.report["result"] = "measured"
    except BaseException as error:
        run.report.update(
            result="failed", error=run.safe(f"{type(error).__name__}: {error}")
        )
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
        diagnostics = run.report.get("postgres_diagnostics", {})
        if diagnostics.get("cgroup_version") == 2:
            kills = diagnostics.get("memory_events", {}).get("oom_kill")
        else:
            counters = dict(
                line.split()
                for line in diagnostics.get("memory.oom_control", "").splitlines()
            )
            kills = int(counters["oom_kill"]) if "oom_kill" in counters else None
        run.report["no_oom_kills"] = (
            kills == 0 and diagnostics.get("state", {}).get("OOMKilled") is False
        )
        if not run.report["no_oom_kills"] or run.report.get("diagnostic_error"):
            run.report["result"] = "failed"
            run.report.setdefault(
                "error",
                "Database OOM observed or missing diagnostics; foreground success does not qualify background work",
            )
        run.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        run.save()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("x") as stream:
            stream.write((run.directory / "report.json").read_text())
        print("REPORT", run.directory / "report.json", flush=True)
    return 0 if run.report["result"] == "measured" else 1


if __name__ == "__main__":
    sys.exit(main())
