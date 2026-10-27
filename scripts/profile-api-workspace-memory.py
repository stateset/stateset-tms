#!/usr/bin/env python3
"""Observe process mappings and existing phase logs on the financial workload.

Only the owned API PID1 is inspected. Extra procfs reads and debug logs change
measurement overhead; these are attribution observations, not new capacity.
"""

import datetime as dt
import importlib.util
from pathlib import Path
import re
import threading
import time

spec = importlib.util.spec_from_file_location(
    "workspace_memory_base",
    Path(__file__).with_name("qualify-api-sustained-envelope.py"),
)
workload = importlib.util.module_from_spec(spec)
spec.loader.exec_module(workload)
base, require = workload.base, workload.require

PHASE_LOG = (
    "error,stateset_freight::read_profile=debug,stateset_freight::write_profile=debug"
)
MAP = re.compile(
    r"^([0-9a-f]+)-([0-9a-f]+) ([rwxps-]{4}) [0-9a-f]+ [0-9a-f]+:[0-9a-f]+ \d+(?:\s+(.*))?$"
)
STATUS_FIELDS = {"VmRSS", "VmHWM", "RssAnon", "RssFile", "RssShmem", "VmSize"}
MAPPING_FIELDS = {"Rss", "Anonymous", "Private_Dirty", "Private_Clean", "Swap"}


def kb(text):
    fields = text.split()
    require(
        len(fields) == 2 and fields[0].isdigit() and fields[1] == "kB",
        "Missing/ambiguous procfs bytes",
    )
    return int(fields[0]) * 1024


def parse_process(text):
    sections = {}
    for item in text.split("@@"):
        if item:
            name, sep, data = item.partition("\n")
            require(sep and name not in sections, "Invalid process framing")
            sections[name] = data
    require(
        set(sections) == {"exe", "status", "smaps", "tasks"}, "Missing process source"
    )
    require(
        sections["exe"].strip() == "/usr/local/bin/stateset-freight",
        "Unexpected PID1 executable",
    )
    status = {}
    for line in sections["status"].splitlines():
        key, sep, value = line.partition(":")
        if key in STATUS_FIELDS | {"Pid", "Threads"}:
            require(sep and key not in status, "Duplicate process status")
            if key in STATUS_FIELDS:
                status[key] = kb(value)
            else:
                require(value.strip().isdigit(), "Invalid PID/thread count")
                status[key] = int(value)
    require(
        set(status) == STATUS_FIELDS | {"Pid", "Threads"}
        and status["Pid"] == 1
        and status["Threads"] > 0,
        "Incomplete/incorrect PID1 status",
    )
    mappings = []
    current = None
    for line in sections["smaps"].splitlines():
        match = MAP.fullmatch(line)
        if match:
            if current is not None:
                require(MAPPING_FIELDS.issubset(current), "Incomplete mapping counters")
            current = {
                "start": int(match[1], 16),
                "end": int(match[2], 16),
                "permissions": match[3],
                "path": match[4] or "",
            }
            require(current["end"] > current["start"], "Invalid mapping range")
            mappings.append(current)
        else:
            key, sep, value = line.partition(":")
            if key in MAPPING_FIELDS:
                require(
                    current is not None and key not in current and sep,
                    "Ambiguous mapping source",
                )
                current[key] = kb(value)
    require(
        mappings and current is not None and MAPPING_FIELDS.issubset(current),
        "Missing mapping observations",
    )
    tids = []
    for line in sections["tasks"].splitlines():
        fields = line.split()
        require(len(fields) == 1 and fields[0].isdigit(), "Invalid task identity")
        tids.append(int(fields[0]))
    require(
        tids and len(tids) == len(set(tids)) and 1 in tids,
        "Missing/duplicate PID1 tasks",
    )
    # Status, smaps and task enumeration are sequential; task churn can change their counts.
    anonymous = [
        v for v in mappings if v["path"] == "" and v["permissions"].startswith("rw")
    ]
    heap = [v for v in mappings if v["path"] == "[heap]"]
    return {
        "status": status,
        "observed_tids": tids,
        "mapping_count": len(mappings),
        "anonymous_rw_mapping_count": len(anonymous),
        "anonymous_rw_rss_bytes": sum(v["Rss"] for v in anonymous),
        "heap_rss_bytes": sum(v["Rss"] for v in heap),
        "mapping_anonymous_bytes": sum(v["Anonymous"] for v in mappings),
        "mapping_swap_bytes": sum(v["Swap"] for v in mappings),
        "anonymous_rw_mappings": anonymous,
        "heap_mappings": heap,
    }


class WorkspaceMemory(workload.SustainedEnvelope):
    def __init__(self, args):
        super().__init__(args)
        require(
            self.plan.get("memory_attribution")
            == {
                "process": "owned API PID1 only",
                "phase_log": PHASE_LOG,
                "interval_seconds": 0.5,
                "allocator_changed": False,
            },
            "Declare instrumentation before running",
        )
        self.report["scope"] = __doc__
        self.report["harness"][str(Path(__file__).resolve().relative_to(base.ROOT))] = (
            base.sha(__file__)
        )
        self.process_samples = []
        self.process_stop = threading.Event()
        self.process_ready = threading.Event()
        self.process_thread = None
        self.report["process_memory_attribution"] = {
            "plan": self.plan["memory_attribution"],
            "scope": "Anonymous resident mappings and thread IDs, not live malloc bytes or allocation stack ownership. Additional sampling/debug overhead; no latency comparison or causality claim.",
        }

    def command(self, command, *, label, **kwargs):
        if label == "actual capped API image":
            require(
                command.count("RUST_LOG=error") == 1, "Base logging contract changed"
            )
            command = [
                "RUST_LOG=" + PHASE_LOG if value == "RUST_LOG=error" else value
                for value in command
            ]
        return super().command(command, label=label, **kwargs)

    def setup(self):
        super().setup()
        actual = dict(
            value.split("=", 1) for value in self.own(self.api)["Config"]["Env"]
        )
        require(actual["RUST_LOG"] == PHASE_LOG, "Actual phase logging differs")
        require(
            not actual.get("MALLOC_ARENA_MAX") and not actual.get("GLIBC_TUNABLES"),
            "Attribution must retain default allocator",
        )
        self.report["process_memory_attribution"]["effective_environment"] = {
            "RUST_LOG": actual["RUST_LOG"]
        }
        self.process_thread = threading.Thread(target=self.observe_process, daemon=True)
        self.process_thread.start()
        require(self.process_ready.wait(15), "No process observation")
        require(
            self.process_samples and "observer_error" not in self.process_samples[0],
            "Initial process observation failed",
        )

    def observe_process(self):
        script = 'printf "@@exe\\n"; readlink /proc/1/exe; printf "@@status\\n"; cat /proc/1/status; printf "@@smaps\\n"; cat /proc/1/smaps; printf "@@tasks\\n"; for process_task in /proc/1/task/[0-9]*; do printf "%s\\n" "${process_task##*/}"; done'
        while not self.process_stop.is_set():
            sample = {
                "at": time.monotonic(),
                "utc": dt.datetime.now(dt.timezone.utc).isoformat(),
            }
            try:
                state = self.own(self.api)["State"]
                if not state["Running"]:
                    sample["terminal"] = {
                        key: state[key] for key in ["Running", "OOMKilled", "ExitCode"]
                    }
                    self.process_samples.append(sample)
                    self.process_ready.set()
                    break
                text = self.command(
                    ["docker", "exec", self.api, "sh", "-c", script],
                    label="owned PID1 process mappings",
                    timeout=10,
                )
                sample.update(parse_process(text))
                path = self.directory / (
                    "process-source-%04d.txt" % len(self.process_samples)
                )
                with path.open("x") as out:
                    out.write(text + "\n")
                sample["raw_source"] = {"path": str(path), "sha256": base.sha(path)}
            except Exception as error:
                state = self.own(self.api)["State"]
                if not state["Running"]:
                    sample["terminal"] = {
                        key: state[key] for key in ["Running", "OOMKilled", "ExitCode"]
                    }
                else:
                    sample["observer_error"] = self.safe(
                        type(error).__name__ + ": " + str(error)
                    )
            self.process_samples.append(sample)
            self.process_ready.set()
            self.process_stop.wait(0.5)

    def finish(self):
        self.process_stop.set()
        if self.process_thread:
            self.process_thread.join(25)
            require(not self.process_thread.is_alive(), "Process sampler did not stop")
        path = self.directory / "process-memory.json"
        with path.open("x") as out:
            out.write(
                base.json.dumps({"samples": self.process_samples}, indent=2) + "\n"
            )
        self.report["process_memory_attribution"]["samples"] = {
            "path": str(path),
            "sha256": base.sha(path),
            "count": len(self.process_samples),
            "observer_errors": sum("observer_error" in x for x in self.process_samples),
        }
        if not self.process_samples or any(
            "observer_error" in x for x in self.process_samples
        ):
            self.report.update(result="failed", process_observation_failed=True)
        super().finish()


if __name__ == "__main__":
    raise SystemExit(base.main(WorkspaceMemory))
