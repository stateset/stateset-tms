#!/usr/bin/env python3
"""Probe record storage with archived, actual Freight before/after outcomes.

This is an isolated storage experiment, not an application implementation,
format upgrade, capacity qualification, or end-to-end latency comparison.
Both layouts omit production derived-index/queue/format triggers. Native
business application, delta preparation and input ingestion are outside the
SQL execution observations. Existing application qualification is unchanged.
"""
import argparse
import datetime as dt
from decimal import Decimal
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import sys
import threading
import time

from runtime_contracts import ROOT, Runtime, fingerprint, literal, require

spec = importlib.util.spec_from_file_location("memory_probe", ROOT / "scripts/profile-history-memory.py")
memory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(memory)
sha = lambda raw: hashlib.sha256(raw).hexdigest()


def decode(raw):
    return json.loads(raw, parse_float=Decimal)


def encode_decimal(value):
    """Exact fallback for PostgreSQL numeric values outside native float precision."""
    if isinstance(value, Decimal):
        require(value.is_finite(), "Non-finite JSON number")
        return str(value)
    if isinstance(value, dict):
        return "{" + ",".join(json.dumps(k, ensure_ascii=False) + ":" + encode_decimal(v)
                              for k, v in sorted(value.items())) + "}"
    if isinstance(value, list):
        return "[" + ",".join(map(encode_decimal, value)) + "]"
    return json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))


class DecimalEncountered(Exception):
    pass


def encode(value):
    """Use the native encoder when exact decimal handling is not needed."""
    def unsupported(item):
        if isinstance(item, Decimal):
            raise DecimalEncountered()
        raise TypeError("Unsupported JSON value")
    try:
        return json.dumps(value, default=unsupported, sort_keys=True,
                          ensure_ascii=False, allow_nan=False, separators=(",", ":"))
    except DecimalEncountered:
        return encode_decimal(value)


def kind(value):
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, (int, Decimal)):
        return "number"
    return {dict: "object", list: "array", str: "string"}[type(value)]


def split(value):
    roots, items = {}, {}
    for root, contents in value.items():
        shape = kind(contents)
        roots[root] = [shape, len(contents) if shape in ("array", "object") else 0,
                       None if shape in ("array", "object") else contents]
        if shape == "array":
            items.update({(root, n, ""): item for n, item in enumerate(contents, 1)})
        elif shape == "object":
            items.update({(root, 0, name): item for name, item in contents.items()})
    return roots, items


def assemble(roots, items):
    """Refuse incomplete, ambiguous or wrongly shaped input instead of dropping it."""
    grouped = {root: [] for root in roots}
    for (root, position, name), payload in items.items():
        require(root in grouped, "Fragment without a root")
        grouped[root].append((position, name, payload))
    result = {}
    for root, (shape, size, scalar) in roots.items():
        rows = sorted(grouped[root])
        require(size >= 0, "Invalid size")
        if shape == "array":
            require(scalar is None and len(rows) == size and
                    all(p == n and name == "" for n, (p, name, _) in enumerate(rows, 1)),
                    "Missing, duplicate or invalid array ordinal")
            result[root] = [v for _, _, v in rows]
        elif shape == "object":
            require(scalar is None and len(rows) == size and all(p == 0 for p, _, _ in rows),
                    "Missing or invalid object member")
            result[root] = {name: v for _, name, v in rows}
            require(len(result[root]) == size, "Duplicate object key")
        else:
            require(size == 0 and not rows and kind(scalar) == shape, "Invalid scalar")
            result[root] = scalar
    return result


class Probe(memory.HistoryMemory):
    def __init__(self, args):
        # Reuse owned-runtime setup, continuous kernel sampling and final OOM
        # diagnostics; no API build, sibling process or historical controller.
        Runtime.__init__(self, args)
        self.source = fingerprint(ROOT)
        self.trace_enabled = False
        self.phases, self.global_samples = [], []
        self.active_phase = None
        self.sample_lock = threading.Lock()
        self.monitor_stop = threading.Event()
        self.monitor_thread = None
        paths = ["scripts/profile-storage-records.py", "scripts/storage-records-prototype.sql",
                 "scripts/profile-history-memory.py", "scripts/runtime_contracts.py",
                 "scripts/qualify-service-capacity.py", "scripts/profile-service-reads.py"]
        self.harness = {p: sha((ROOT / p).read_bytes()) for p in paths}
        self.report.update(scope=__doc__, source=self.source, harness=self.harness,
                           settings={"postgres_cpu_quota": 2, "postgres_memory_mib": 768,
                                     "application_build": "none; production source unchanged",
                                     "snapshot_isolation": "repeatable read",
                                     "write_lock": "one tenant head; expected revision checked under lock"},
                           attribution={"sampling_interval_seconds": 0.2},
                           memory_phases=self.phases, continuous_memory_samples=self.global_samples,
                           workloads=[])

    def query(self, statement, label="prototype SQL"):
        return self.command(["psql", self.url, "-X", "-qAt", "-v", "ON_ERROR_STOP=1"],
                            label=label, input=statement, env={"PATH": os.defpath, "LC_ALL": "C"})

    def archived(self, record):
        path = Path(record["path"])
        require(sha(path.read_bytes()) == record["sha256"], "Changed compressed source")
        raw = gzip.decompress(path.read_bytes())
        require(sha(raw) == record["raw_sha256"], "Changed original source")
        return decode(raw)

    def capture(self, name, value):
        raw = encode(value).encode()
        path = self.directory / (name + ".json.gz")
        path.write_bytes(gzip.compress(raw, compresslevel=1, mtime=0))
        return {"path": str(path), "sha256": sha(path.read_bytes()), "raw_sha256": sha(raw)}

    def snapshot(self, tenant):
        owner = literal(tenant)
        lines = self.query("BEGIN ISOLATION LEVEL REPEATABLE READ;\n"
                           "SELECT jsonb_build_array('head',format,revision,root_names)::text FROM probe_heads WHERE tenant_id=" + owner + " FOR SHARE;\n"
                           "SELECT jsonb_build_array('root',root,kind,size,scalar,xmin::text,ctid::text)::text FROM probe_roots WHERE tenant_id=" + owner + " ORDER BY root;\n"
                           "SELECT jsonb_build_array('item',root,position,name,payload,xmin::text,ctid::text)::text FROM probe_items WHERE tenant_id=" + owner + " ORDER BY root,position,name;\nCOMMIT;",
                           "stream all original fragments")
        rows = [decode(line) for line in lines.splitlines()]
        heads = [r for r in rows if r[0] == "head"]
        require(len(heads) == 1 and heads[0][1] == 18, "Missing/incompatible header")
        roots, items, physical = {}, {}, {}
        for row in rows:
            if row[0] == "root":
                require(row[1] not in roots, "Duplicate root")
                roots[row[1]] = row[2:5]
                physical[(row[1],)] = row[5:]
            elif row[0] == "item":
                key = tuple(row[1:4])
                require(key not in items, "Duplicate fragment")
                items[key] = row[4]
                physical[key] = row[5:]
        names = heads[0][3]
        require(isinstance(names, list) and len(names) == len(set(names)) and
                set(names) == set(roots), "Missing or unexpected root descriptor")
        return assemble(roots, items), physical, heads[0][2]

    def seed(self, tenant, value):
        payload = literal(encode(value))
        self.query("INSERT INTO probe_aggregates(tenant_id,data) VALUES(" + literal(tenant) + "," + payload + "::jsonb);\nSELECT probe_backfill(" + literal(tenant) + ");", "one-time exact-source backfill")
        actual, _, revision = self.snapshot(tenant)
        require(encode(actual) == encode(value) and revision == 0, "Backfill lost original values")

    def delta(self, before, after):
        old_roots, old_items = split(before)
        roots, items = split(after)
        changed_roots = {k: v for k, v in roots.items() if k not in old_roots or encode(v) != encode(old_roots[k])}
        changed_items = {k: v for k, v in items.items() if k not in old_items or encode(v) != encode(old_items[k])}
        return changed_roots, changed_items, set(old_roots) - set(roots), set(old_items) - set(items)

    def records_statement(self, tenant, before, after, revision):
        roots, items, removed_roots, removed_items = self.delta(before, after)
        owner = literal(tenant)
        statements = []
        # The lock/revision decision precedes every write, not merely a final
        # conditional header UPDATE after independent fragments have changed.
        fence = "DO $$ BEGIN PERFORM 1 FROM probe_heads WHERE tenant_id=" + owner + " AND format=18 AND revision=" + str(revision) + " FOR UPDATE; IF NOT FOUND THEN RAISE EXCEPTION 'stale prototype revision'; END IF; END $$;\n"
        if roots:
            values = ",".join("(" + ",".join([owner, literal(root), literal(v[0]), str(v[1]),
                                             "NULL" if v[0] in ("array", "object") else literal(encode(v[2])) + "::jsonb"]) + ")" for root, v in roots.items())
            statements.append("INSERT INTO probe_roots VALUES " + values + " ON CONFLICT(tenant_id,root) DO UPDATE SET kind=EXCLUDED.kind,size=EXCLUDED.size,scalar=EXCLUDED.scalar")
        if removed_items:
            keys = ",".join("(" + literal(r) + "," + str(p) + "," + literal(n) + ")" for r, p, n in sorted(removed_items))
            statements.append("DELETE FROM probe_items WHERE tenant_id=" + owner + " AND (root,position,name) IN (" + keys + ")")
        if items:
            values = ",".join("(" + ",".join([owner, literal(r), str(p), literal(n), literal(encode(v)) + "::jsonb"]) + ")" for (r, p, n), v in items.items())
            statements.append("INSERT INTO probe_items VALUES " + values + " ON CONFLICT(tenant_id,root,position,name) DO UPDATE SET payload=EXCLUDED.payload")
        if removed_roots:
            statements.append("DELETE FROM probe_roots WHERE tenant_id=" + owner + " AND root IN (" + ",".join(map(literal, sorted(removed_roots))) + ")")
        names = "ARRAY[" + ",".join(map(literal, sorted(after))) + "]::text[]"
        statements.append("UPDATE probe_heads SET revision=revision+1,root_names=" + names + " WHERE tenant_id=" + owner)
        return fence, statements, {"changed_roots": len(roots), "changed_records": len(items),
                                   "deleted_roots": len(removed_roots), "deleted_records": len(removed_items),
                                   "record_payload_bytes": sum(len(encode(v).encode()) for v in items.values())}

    def execute(self, fence, statements, label):
        plans = []
        # One connection/transaction. EXPLAIN ANALYZE executes each statement
        # once; it is not followed by a second unmeasured application.
        sql = "BEGIN;\n" + fence + "\n".join("EXPLAIN(ANALYZE,WAL,BUFFERS,FORMAT JSON) " + s + ";" for s in statements) + "\nCOMMIT;"
        started = time.monotonic()
        raw = self.query(sql, label)
        elapsed = (time.monotonic() - started) * 1000
        parser, offset = json.JSONDecoder(parse_float=Decimal), 0
        while offset < len(raw):
            while offset < len(raw) and raw[offset].isspace():
                offset += 1
            if offset == len(raw):
                break
            plan, offset = parser.raw_decode(raw, offset)
            plans.append(plan[0])
        require(len(plans) == len(statements), "Missing executed SQL plans")
        return {"observed_command_ms": elapsed, "sql_execution_ms": float(sum(p["Execution Time"] for p in plans)),
                "statement_count": len(statements), "sql_bytes": len(sql.encode()),
                "plans": self.capture(label + "-plans", plans)}

    def footprint(self):
        rows = [decode(v) for v in self.query("SELECT jsonb_build_array(c.relname,pg_table_size(c.oid),pg_indexes_size(c.oid),pg_total_relation_size(c.oid))::text FROM pg_class c WHERE c.relname IN ('probe_aggregates','probe_heads','probe_roots','probe_items') ORDER BY c.relname;", "record actual relation sizes").splitlines()]
        sizes = {r[0]: {"table_bytes": r[1], "index_bytes": r[2], "total_bytes": r[3]} for r in rows}
        require(len(sizes) == 4, "Missing measured relations")
        return {"relations": sizes,
                "aggregate_total_bytes": sizes["probe_aggregates"]["total_bytes"],
                "records_total_bytes": sum(v["total_bytes"] for k, v in sizes.items() if k != "probe_aggregates"),
                "scope": "Physical tables, TOAST and indexes; post-write sizes can include dead versions and normal background maintenance."}

    def integrity_checks(self):
        tenant = "edge"
        value = {"_freight_format": 18, "events": [{"id": "duplicate", "value": None}, {"id": "duplicate", "value": True}],
                 "receipts": {"literal/\"雪{}": {"fingerprint": "original", "response": [None, True, 18446744073709551615, Decimal("1.000000000000000000123456789")]}},
                 "operations_cases": None, "operations_policy": {"future": ["retain"]}, "empty": []}
        require(encode(value) == encode_decimal(value), "Exact-decimal encoder differs")
        self.seed(tenant, value)
        actual, _, _ = self.snapshot(tenant)
        require(encode(actual) == encode(value), "Literal key, number or duplicate identity changed")
        after = decode(encode(value))
        after["operations_cases"] = {"unsupported": ["retain", None]}
        after["events"] = [value["events"][1], value["events"][0], {"id": "duplicate", "value": False}]
        after["new opaque root"] = {"literal/\"雪{}": [0, False, None]}
        del after["empty"]
        fence, statements, _ = self.records_statement(tenant, value, after, 0)
        self.execute(fence, statements, "edge-shape-and-order")
        changed, _, revision = self.snapshot(tenant)
        require(encode(changed) == encode(after) and revision == 1, "Shape/order/root transition lost originals")
        self.query("DELETE FROM probe_items WHERE tenant_id='edge' AND root='events' AND position=2;", "inject missing original fragment")
        try:
            self.snapshot(tenant)
        except (AssertionError, RuntimeError) as error:
            require("array ordinal" in str(error), "Unexpected fragment refusal")
        else:
            raise RuntimeError("Missing original fragment was silently accepted")
        self.query("INSERT INTO probe_items VALUES('edge','events',2,''," + literal(encode(after["events"][1])) + "::jsonb);", "restore original fragment")
        self.query("DELETE FROM probe_roots WHERE tenant_id='edge' AND root='operations_cases';", "inject missing root descriptor")
        try:
            self.snapshot(tenant)
        except (AssertionError, RuntimeError) as error:
            require("root descriptor" in str(error), "Unexpected root refusal")
        else:
            raise RuntimeError("Missing root descriptor was silently defaulted")
        # Isolation and malformed-layout refusal are storage-reader checks;
        # this generic representation is not permission to accept unknown
        # business roots through the production typed Workspace decoder.
        self.report["representation_checks"] = {"literal_unicode_keys": True, "exact_large_integer_and_decimal": True,
                                                "null_boolean_and_zero_distinct": True, "duplicate_ids_and_array_order": True,
                                                "opaque_shape_and_root_transitions": True, "missing_fragment_refused": True,
                                                "missing_root_refused": True}
        self.query("TRUNCATE probe_aggregates,probe_heads CASCADE;", "remove owned corruption fixtures")

    def checks(self):
        self.query((ROOT / "scripts/storage-records-prototype.sql").read_text(), "owned prototype schema")
        self.integrity_checks()
        native_path = self.args.native_report.resolve()
        native = json.loads(native_path.read_text())
        require(native["result"] == "measured" and native["no_oom_kills"], "Native source run was not qualified")
        require(native["builds"]["freight"]["source"] == self.source, "Native source is not current production")
        self.report["native_report"] = {"path": str(native_path), "sha256": sha(native_path.read_bytes())}
        for w in native["workloads"]:
            self.query("TRUNCATE probe_aggregates,probe_heads CASCADE;", "isolate next fixture relations")
            tenant = "probe-" + str(w["shipments"])
            initial = self.archived(w["samples"][0]["source_before"])
            self.seed(tenant, initial)
            roots, items = split(initial)
            output = {"shipments": w["shipments"], "history_depth": w["history_depth"],
                      "initial_roots": len(roots), "initial_records": len(items),
                      "root_bytes": {k: len(encode(v).encode()) for k, v in initial.items()},
                      "initial_storage": self.footprint(), "samples": []}
            self.report["workloads"].append(output)
            actual, physical, revision = self.snapshot(tenant)
            for n, sample in enumerate(w["samples"]):
                before, after = self.archived(sample["source_before"]), self.archived(sample["source_after"])
                require(encode(actual) == encode(before) and revision == n, "Native sequence/source diverged")
                fence, statements, counts = self.records_statement(tenant, before, after, n)
                aggregate = "UPDATE probe_aggregates SET data=" + literal(encode(after)) + "::jsonb,revision=revision+1 WHERE tenant_id=" + literal(tenant) + " AND revision=" + str(n)
                # Alternate primitive order; both are isolated representations
                # of the exact same previously committed native outcome.
                mutations = {}
                order = ["aggregate", "records"] if n % 2 == 0 else ["records", "aggregate"]
                for mode in order:
                    mutations[mode] = self.execute(fence if mode == "records" else "", statements if mode == "records" else [aggregate], tenant + "-" + str(n) + "-" + mode)
                actual, current, revision = self.snapshot(tenant)
                require(encode(actual) == encode(after) and revision == n + 1, "Record commit changed original content")
                changed_roots, changed_items, deleted_roots, deleted_items = self.delta(before, after)
                unchanged = set(physical) - {(r,) for r in changed_roots} - set(changed_items) - set(deleted_items) - {(r,) for r in deleted_roots}
                require(all(current[k] == physical[k] for k in unchanged), "Unrelated physical record rewritten")
                aggregate_source = decode(self.query("SELECT data::text FROM probe_aggregates WHERE tenant_id=" + literal(tenant), "verify aggregate original"))
                require(encode(aggregate_source) == encode(after), "Aggregate primitive diverged")
                receipt = self.query("BEGIN; SELECT format FROM probe_heads WHERE tenant_id=" + literal(tenant) + " FOR SHARE; SELECT payload::text FROM probe_items WHERE tenant_id=" + literal(tenant) + " AND root='receipts' AND position=0 AND name=" + literal(sample["key"]) + "; COMMIT;", "selected original receipt")
                lines = receipt.splitlines()
                require(lines[0] == "18" and len(lines) == 2 and encode(decode(lines[1])) == encode(after["receipts"][sample["key"]]), "Literal original receipt changed")
                replayed, replay_physical, replay_revision = self.snapshot(tenant)
                require(encode(replayed) == encode(after) and replay_physical == current and replay_revision == revision, "Original lookup rewrote source")
                entry = {**counts, "order": order, "mutations": mutations,
                         "unchanged_physical_rows_verified": len(unchanged), "exact_original_replay": True,
                         "complete_source": self.capture(tenant + "-" + str(n) + "-restored-source", actual),
                         "native_before": sample["source_before"], "native_after": sample["source_after"]}
                output["samples"].append(entry)
                physical = current
                print("VERIFIED", tenant, n, counts, flush=True)
                self.save()
            # A stale transaction must refuse every fragment, not leave a
            # partially applied change behind a rejected final revision bump.
            fence, statements, _ = self.records_statement(tenant, before, after, 0)
            try:
                self.execute(fence, statements, tenant + "-stale")
                raise AssertionError("Stale record mutation was accepted")
            except RuntimeError as error:
                require("stale prototype revision" in str(error), "Unexpected stale refusal")
            final, final_physical, final_revision = self.snapshot(tenant)
            require(encode(final) == encode(actual) and final_physical == physical and final_revision == revision, "Stale transaction left partial writes")
            output["stale_revision_atomic_refusal"] = True
            output["post_mutation_storage"] = self.footprint()
        self.step("actual archived outcomes reconstruct exactly with untouched original rows and read-only receipt lookup")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--native-report", type=Path, default=ROOT / "docs/evidence/prepared-write-after-confirmation.json")
    parser.add_argument("--postgres-image", default="postgres:16")
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Preserve evidence; select a new output path")
    run = Probe(args)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        run.postgres()
        run.prepare_sampling()
        run.sql("postgres", "CREATE DATABASE storage_probe")
        run.url = f"postgres://freight_runtime@127.0.0.1:{run.ports['postgres']}/storage_probe"
        run.checks()
        require(fingerprint(ROOT) == run.source, "Production source drift")
        require(all(sha((ROOT / p).read_bytes()) == h for p, h in run.harness.items()), "Probe source drift")
        run.report["result"] = "measured"
    except BaseException as error:
        run.report.update(result="failed", error=run.safe(f"{type(error).__name__}: {error}"))
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
        diagnostic = run.report.get("postgres_diagnostics", {})
        if diagnostic.get("cgroup_version") == 2:
            kills = diagnostic.get("memory_events", {}).get("oom_kill")
            hits = diagnostic.get("memory_events", {}).get("max")
        else:
            counters = dict(line.split() for line in diagnostic.get("memory.oom_control", "").splitlines())
            kills = int(counters["oom_kill"]) if "oom_kill" in counters else None
            hits = int(diagnostic["memory.failcnt"]) if "memory.failcnt" in diagnostic else None
        run.report["no_oom_or_limit_hits"] = kills == hits == 0 and diagnostic.get("state", {}).get("OOMKilled") is False
        if not run.report["no_oom_or_limit_hits"] or run.report.get("diagnostic_error"):
            run.report.update(result="failed", resource_error="OOM, limit hit or missing final diagnostics")
        run.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        run.save()
        with args.output.open("x") as stream:
            stream.write((run.directory / "report.json").read_text())
        print("REPORT", args.output, flush=True)
    return 0 if run.report["result"] == "measured" else 1


if __name__ == "__main__":
    sys.exit(main())
