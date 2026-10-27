#!/usr/bin/env python3
"""Paired original-history HTTP observations with cold-build and write costs.

Run before and after against separate disposable PostgreSQL16 clusters. Both
variants must use the same fixed reference fixtures, budgets and harness. This
finite sequential workload establishes neither capacity nor an SLA.
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
from urllib.parse import urlencode

from runtime_contracts import ROOT, fingerprint, literal, require

spec = importlib.util.spec_from_file_location(
    "deferred_history", ROOT / "scripts/profile-deferred-history.py"
)
deferred = importlib.util.module_from_spec(spec)
spec.loader.exec_module(deferred)
history = deferred.history


class IndexedHistory(deferred.DeferredHistory):
    def __init__(self, args):
        super().__init__(args)
        self.inventory = json.loads(args.inventory.read_text())
        self.harness["scripts/profile-indexed-history.py"] = history.sha(
            Path(__file__).read_bytes()
        )
        self.report.update(
            scope=__doc__,
            variant=args.variant,
            inventory={
                "path": str(args.inventory),
                "sha256": history.sha(args.inventory.read_bytes()),
            },
        )
        self.report["settings"].update(
            response_modes=None,
            timed_cases=[
                "global_events",
                "shipment_events",
                "shipment_gates",
                "shipment_appointments",
            ],
            normalization="None for original history responses; complete workspace removes only operations.generated_at.",
            cold_read="First gates page after fixture insertion; first events page after a real location command.",
        )

    def build(self):
        if self.args.variant == "after":
            super().build()
        else:
            binary = self.args.inventory.parent / "stateset-freight-release"
            require(
                history.sha(binary.read_bytes())
                == self.inventory["release_api_sha256"],
                "Baseline executable changed",
            )
            for relative, expected in self.inventory["source_files_sha256"].items():
                require(
                    history.sha(
                        (self.args.inventory.parent / "source" / relative).read_bytes()
                    )
                    == expected,
                    "Baseline source copy changed",
                )
            self.binaries = {"freight": binary}
            self.generator = ROOT / "target/release/examples/workspace_history_fixture"
            self.report["builds"] = {
                "freight": {
                    "source": self.inventory["core"],
                    "binary_sha256": history.sha(binary.read_bytes()),
                },
                "fixture": {"binary_sha256": history.sha(self.generator.read_bytes())},
            }
        require(
            history.sha((ROOT / "examples/workspace_history_fixture.rs").read_bytes())
            == self.inventory["source_files_sha256"][
                "examples/workspace_history_fixture.rs"
            ],
            "Fixture source changed",
        )

    def history_path(self, kind, load=None, cursor=None):
        q = {"kind": kind, "limit": 25}
        if load is not None:
            q["load_id"] = load
        if cursor is not None:
            q["cursor"] = cursor
        return "/v1/history?" + urlencode(q)

    def snapshot(self):
        # Inspect the same physical identity and complete logical source hash
        # without the older observer's second data::text allocation for length.
        # This runs outside all HTTP timing/process-memory samples.
        return self.rows(
            "freight_runtime",
            "SELECT tenant_id,xmin::text AS revision,updated_at,md5(data::text) AS data_md5 FROM freight_workspaces ORDER BY tenant_id",
        )

    def recorded(self, name, path):
        sample, raw = self.observed(path)
        sample["archive"] = self.archive(name, raw)
        return sample, json.loads(raw)

    def checks(self):
        self.start_api()
        self.stop("freight")
        for index, (count, depth) in enumerate(self.report["settings"]["workloads"]):
            self.sql(
                "freight_runtime",
                "TRUNCATE freight_load_search,freight_load_catalog,freight_workspaces,freight_document_content CASCADE",
            )
            prior = self.reference["workloads"][index]
            raw = history.subprocess.check_output(
                [
                    str(self.generator),
                    str(count),
                    f"workspace-{count}-{depth}",
                    self.clock,
                    str(depth),
                ],
                cwd=self.directory,
            )
            require(
                history.sha(raw) == prior["fixture_sha256"], "Fixture bytes changed"
            )
            workspace = json.loads(raw)["workspace"]
            workspace["_freight_format"] = 18
            prefix = f"{count}-{depth}"
            w = {
                "shipments": count,
                "history_depth": depth,
                "fixture_sha256": history.sha(raw),
                "fixture_archive": self.archive(prefix + "-fixture", raw),
                "cases": {},
            }
            self.report["workloads"].append(w)
            self.command(
                [
                    "psql",
                    f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime",
                    "-X",
                    "-v",
                    "ON_ERROR_STOP=1",
                ],
                label="owned dense insertion",
                input="INSERT INTO freight_workspaces(tenant_id,data) VALUES('large',"
                + literal(json.dumps(workspace))
                + "::jsonb);",
            )
            before = self.snapshot()
            self.start_api()
            first, last = workspace["loads"][0], workspace["loads"][-1]
            w["cold_initial"], cold_page = self.recorded(
                prefix + "-cold", self.history_path("gates", first["id"])
            )
            cold_expected = [
                {"position": i + 1, "record": row}
                for i, row in reversed(list(enumerate(workspace["stop_events"])))
                if row["load_id"] == first["id"]
            ]
            require(
                cold_page["items"] == cold_expected[:25]
                and cold_page["total"] == len(cold_expected),
                "Cold original page changed",
            )
            for name, kind, load in [
                ("global_events", "events", None),
                ("shipment_events", "events", first["id"]),
                ("shipment_gates", "gates", first["id"]),
                ("shipment_appointments", "appointments", last["id"]),
            ]:
                field = {
                    "events": "events",
                    "gates": "stop_events",
                    "appointments": "appointments",
                }[kind]
                identity = "entity_id" if kind == "events" else "load_id"
                expected = [
                    {"position": i + 1, "record": row}
                    for i, row in reversed(list(enumerate(workspace[field])))
                    if load is None or row[identity] == load
                ]
                samples = []
                case = {"samples": samples}
                w["cases"][name] = case
                for n in range(self.args.samples + 1):
                    sample, page = self.recorded(
                        f"{prefix}-{name}-{n}", self.history_path(kind, load)
                    )
                    require(
                        page["items"] == expected[:25]
                        and page["total"] == len(expected)
                        and page["captured_through"] == len(workspace[field]),
                        "Original page changed",
                    )
                    require(
                        page["tenant_id"] == "large"
                        and page["access"]
                        == {"subject": "user:capacity", "role": "operations"}
                        and not page["has_new_records"],
                        "Owner/window changed",
                    )
                    if n == 0:
                        case["warmup"] = sample
                        case["page"] = page
                    else:
                        require(
                            page == case["page"],
                            "Read response changed between samples",
                        )
                        samples.append(sample)
                    self.save()
                times = [s["response_ms"] for s in samples]
                case["latency_ms"] = {
                    "min": min(times),
                    "median": statistics.median(times),
                    "max": max(times),
                }
                print(
                    f"READ {prefix} {name} median {statistics.median(times):.1f}ms",
                    flush=True,
                )
            w["original_history"] = self.original_pages(workspace, prefix)
            sample, full_raw = self.observed("/v1/workspace")
            w["complete_response"] = {
                **sample,
                "archive": self.archive(prefix + "-complete", full_raw),
            }
            normalized, _ = self.validate(json.loads(full_raw), workspace)
            require(
                normalized == prior["normalized_response_sha256"],
                "Complete legacy workspace changed",
            )
            require(before == self.snapshot(), "Read rewrote authoritative source")
            w["unchanged_physical_source"] = before
            body = {
                "action": "location",
                "expected_version": first["version"],
                "location": "Owned indexed history observation",
            }
            path = "/v1/loads/" + first["id"] + "/commands"
            key = "owned-indexed-history-location"
            w["native_write"], response = self.request(
                ("location", path, history.TOKEN, body, key)
            )
            w["native_write"]["archive"] = self.archive(prefix + "-write", response)
            require(
                w["native_write"]["status"] == 200, "Native location command failed"
            )
            after_write = self.snapshot()
            replay, replay_raw = self.request(
                ("location_replay", path, history.TOKEN, body, key)
            )
            w["native_replay"] = {
                **replay,
                "archive": self.archive(prefix + "-replay", replay_raw),
            }
            original_result, replay_result = json.loads(response), json.loads(replay_raw)
            require(
                replay["status"] == 200
                and original_result["replayed"] is False
                and replay_result == {"data": original_result["data"], "replayed": True}
                and self.snapshot() == after_write,
                "Lost-response replay changed result/source",
            )
            current = self.rows(
                "freight_runtime",
                "SELECT data FROM freight_workspaces WHERE tenant_id='large'",
            )[0]["data"]
            w["source_after_write"] = self.archive(
                prefix + "-source-after-write", json.dumps(current).encode()
            )
            w["physical_source_after_write"] = after_write
            for field in ["appointments", "stop_events", "tenders"]:
                require(
                    current[field] == workspace[field],
                    "Write changed retained physical facts",
                )
            require(
                current["events"][: len(workspace["events"])] == workspace["events"],
                "Write replaced original events",
            )
            w["cold_after_write"], page = self.recorded(
                prefix + "-cold-after-write", self.history_path("events", first["id"])
            )
            expected = [
                {"position": i + 1, "record": row}
                for i, row in reversed(list(enumerate(current["events"])))
                if row["entity_id"] == first["id"]
            ]
            require(
                page["items"] == expected[:25] and page["total"] == len(expected),
                "Dirty history failed to rebuild exact originals",
            )
            cursor = w["cases"]["global_events"]["page"]["next_cursor"]
            old_sample, old_page = self.recorded(
                prefix + "-old-event-cursor", self.history_path("events", cursor=cursor)
            )
            w["old_event_cursor"] = old_sample
            require(
                old_page["items"]
                == [
                    {"position": i + 1, "record": row}
                    for i, row in reversed(list(enumerate(workspace["events"])))
                ][25:50]
                and old_page["has_new_records"]
                and old_page["captured_through"] == len(workspace["events"]),
                "Open append window changed",
            )
            stop_cursor = w["cases"]["shipment_gates"]["page"]["next_cursor"]
            if stop_cursor:
                denied, _ = self.request(
                    (
                        "old_stop_cursor",
                        self.history_path("gates", first["id"], stop_cursor),
                        history.TOKEN,
                        None,
                        None,
                    )
                )
                require(denied["status"] == 409, "Old stop version was accepted")
                w["old_stop_cursor_status"] = 409
            require(
                after_write == self.snapshot(),
                "Replay/history reads changed authoritative source",
            )
            if self.args.variant == "after":
                state = self.rows(
                    "freight_runtime",
                    "SELECT s.ready,s.source_revision,w.history_revision FROM freight_workspaces w JOIN freight_history_state s USING(tenant_id) WHERE w.tenant_id='large'",
                )[0]
                require(
                    state["ready"]
                    and state["source_revision"] == state["history_revision"],
                    "Stale index after native write",
                )
                w["index_after_write"] = state
                w["index_bytes"] = self.rows(
                    "freight_runtime",
                    "SELECT relname,pg_total_relation_size(oid) AS bytes FROM pg_class WHERE relname IN ('freight_history_state','freight_history_scopes','freight_history_loads','freight_history_collections') ORDER BY relname",
                )
            self.stop("freight")
            self.save()
        self.report["qualification"] = {
            "functional_contracts": "pass",
            "capacity_sla": "not established",
            "limits": "Sequential shared-host observations, fixed before-then-after order, no concurrent offered load. Cold index rebuild locks the tenant; JSONB decompression and per-scope ordinal processing remain.",
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--variant", choices=["before", "after"], required=True)
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument(
        "--reference",
        type=Path,
        default=ROOT / "docs/evidence/workspace-history-after.json",
    )
    parser.add_argument("--samples", type=int, choices=[3, 5, 7, 9], default=7)
    parser.add_argument("--postgres-image", default="postgres:16")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Preserve the existing report; choose a new output path")
    run = IndexedHistory(args)

    def interrupted(_signum, _frame):
        raise KeyboardInterrupt()

    signal.signal(signal.SIGTERM, interrupted)
    try:
        run.build()
        run.postgres()
        run.checks()
        require(
            run.source == fingerprint(ROOT), "Application changed during measurement"
        )
        require(
            all(
                history.sha((ROOT / p).read_bytes()) == h
                for p, h in run.harness.items()
            ),
            "Harness changed during measurement",
        )
        run.report["result"] = "measured"
    except BaseException as error:
        run.report.update(
            result="failed", error=run.safe(f"{type(error).__name__}: {error}")
        )
        print(run.report["error"], file=sys.stderr, flush=True)
    finally:
        try:
            run.cleanup()
        except BaseException as error:
            run.report.update(result="failed", cleanup_error=run.safe(str(error)))
        d = run.report.get("postgres_diagnostics", {})
        if d.get("cgroup_version") == 2:
            kills = d.get("memory_events", {}).get("oom_kill")
        else:
            counters = dict(
                line.split() for line in d.get("memory.oom_control", "").splitlines()
            )
            kills = int(counters["oom_kill"]) if "oom_kill" in counters else None
        if (
            kills != 0
            or d.get("state", {}).get("OOMKilled") is not False
            or run.report.get("diagnostic_error")
        ):
            run.report["resource_failure"] = "Missing/OOM database diagnostics"
            run.report.setdefault("error", "Missing/OOM database diagnostics")
            run.report["result"] = "failed"
        elif run.report["result"] == "measured":
            run.report["qualification"]["no_database_oom_kills"] = True
        run.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        run.save()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("x") as stream:
            stream.write((run.directory / "report.json").read_text())
    return 0 if run.report["result"] == "measured" else 1


if __name__ == "__main__":
    sys.exit(main())
