#!/usr/bin/env python3
"""Qualify the retained HTTP workload with opt-in read phase and procfs samples.

Phase tracing adds overhead. Compare runs of this driver, retaining unfavorable
observations. The ordinary API does not emit these records at its default level.
"""
import importlib.util
import json
import math

from runtime_contracts import ROOT, require

spec = importlib.util.spec_from_file_location(
    "profiled_capacity", ROOT / "scripts/profile-service-reads.py"
)
profiled = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profiled)


class PipelineCapacity(profiled.ProfiledCapacity):
    def __init__(self, args):
        super().__init__(args)
        name = "scripts/profile-read-pipeline.py"
        self.harness[name] = profiled.capacity.digest((ROOT / name).read_bytes())
        self.report["read_phase_instrumentation"] = {
            "target": "stateset_freight::read_profile=debug",
            "scope": "projected workspace/operations reads; excludes other routes and writes",
            "overhead": "phase clocks, UUID correlation, JSON trace formatting and local log writes",
            "timing": "wall time; projection includes consumed snapshot disposal; not CPU time",
        }

    def spawn(self, name, config):
        if name == "freight":
            config = dict(config, RUST_LOG="stateset_freight::read_profile=debug")
        return super().spawn(name, config)

    def stop(self, name):
        super().stop(name)
        if name != "freight":
            return
        raw = (self.directory / "freight.log").read_bytes()
        records = []
        for line in raw.decode().splitlines():
            if "freight_read_profile=" in line:
                # Extract the JSON object only; the tracing formatter may add
                # color reset sequences to its prefix/suffix.
                start = line.index("freight_read_profile=") + len("freight_read_profile=")
                record, _ = json.JSONDecoder().raw_decode(line[start:])
                records.append(record)
        reads = {}
        for record in records:
            stages = reads.setdefault(record["read_id"], {})
            require(record["stage"] not in stages, "Duplicate phase in one read")
            require(math.isfinite(record["elapsed_ms"]) and record["elapsed_ms"] >= 0,
                    "Invalid phase duration")
            stages[record["stage"]] = record
        required = {"admission", "snapshot", "decode_queue", "decode", "commit",
                    "projection", "encoding", "response_value_drop", "complete"}
        require(all(required <= set(stages) for stages in reads.values()),
                "Projected read did not complete every required phase")
        require(len(reads) == 5 * self.args.requests * len(self.report["workloads"]),
                "Read phase coverage does not match the retained HTTP requests")
        groups = {}
        for record in records:
            groups.setdefault((record["tenant_id"], record["stage"]), []).append(record["elapsed_ms"])
        summary = []
        for (tenant, stage), values in sorted(groups.items()):
            values.sort()
            summary.append({"tenant_id": tenant, "stage": stage, "samples": len(values),
                            "p50_ms": values[math.ceil(len(values) * .5) - 1],
                            "p95_ms": values[math.ceil(len(values) * .95) - 1],
                            "max_ms": max(values)})
        self.report["read_pipeline"] = {
            "completed_reads": len(reads), "phase_records": records, "summary": summary,
            "log_sha256": profiled.capacity.digest(raw),
            "reads_changing_worker_between_decode_and_projection": sum(
                stages["decode"]["thread_id"] != stages["projection"]["thread_id"]
                for stages in reads.values()),
            "observed_blocking_job_counts": sorted({1 + int("projection_queue" in stages)
                                                     for stages in reads.values()}),
        }


if __name__ == "__main__":
    profiled.capacity.Capacity = PipelineCapacity
    raise SystemExit(profiled.capacity.main())
