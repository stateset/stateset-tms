#!/usr/bin/env python3
"""Exercise the real PostgreSQL catalog without requiring a listening socket.

Owns a fresh temporary cluster. This qualifies SQL/projections and query plans,
not sqlx transport, concurrent sessions, browser behavior or service latency.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

from runtime_contracts import ROOT, fingerprint, literal, require


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--postgres-bin", type=Path, default=Path("/usr/lib/postgresql/12/bin"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--size", type=int, default=1000)
    parser.add_argument("--compare-maintenance", action="store_true", help="Retain alternating migration-021/current observations for single, bulk, reordered and history-only writes")
    args = parser.parse_args()
    require(not args.output.exists(), "Choose a new evidence output; retained reports must not be overwritten")
    require(1 <= args.size <= 5000, "Fixture size outside supported range")
    source = fingerprint(ROOT)
    directory = Path(tempfile.mkdtemp(prefix="stateset-freight-catalog-sql-"))
    cluster = directory / "postgres"
    fixture_binary = ROOT / "target/debug/examples/service_capacity_fixture"
    fixture = subprocess.check_output([str(fixture_binary), str(args.size), "catalog-owned", "2026-10-16T12:00:00Z"])
    workspace = json.loads(fixture)["workspace"]
    workspace["_freight_format"] = 17
    (directory / "fixture.json").write_bytes(fixture)
    log = subprocess.run([str(args.postgres_bin / "initdb"), "-D", str(cluster), "-A", "trust", "--no-locale"], text=True, capture_output=True)
    (directory / "initdb.log").write_text(log.stdout + log.stderr)
    require(log.returncode == 0, "Owned cluster initialization failed")
    phases = []

    def sql(name, text):
        # -j treats a blank line as the end of a batch, including inside a
        # dollar-quoted function. Preserve line comments but omit blank lines.
        text = "\n".join(line for line in text.splitlines() if line.strip()) + "\n"
        (directory / (name + ".sql")).write_text(text)
        result = subprocess.run([str(args.postgres_bin / "postgres"), "--single", "-D", str(cluster), "-j", "postgres"], input=text, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        (directory / (name + ".log")).write_text(result.stdout)
        # Standalone PostgreSQL can exit zero even after a failed SQL command.
        require(result.returncode == 0 and not re.search(r"\b(ERROR|FATAL|PANIC):", result.stdout), name + " SQL failed; inspect " + str(directory))
        phases.append({"name": name, "result": "pass", "sql_sha256": digest(text.encode()), "log_sha256": digest(result.stdout.encode())})
        return result.stdout

    migrations = sorted((ROOT / "migrations").glob("*.sql"))
    require(len(migrations) == 25, "Review qualification for the new migration inventory")
    sql("previous-schema", "BEGIN;\n" + "\n".join(p.read_text() for p in migrations[:20]) + "\nCOMMIT;")
    sql("original-records", "INSERT INTO freight_workspaces(tenant_id,data,updated_at) VALUES('owned'," + literal(json.dumps(workspace)) + "::jsonb,'2026-01-01T00:00:00Z');\n"
        "INSERT INTO freight_document_content VALUES('owned','original-proof',decode('255044462d312e340a6f726967696e616c','hex'));\n"
        "CREATE TABLE original_workspace AS SELECT data,updated_at,xmin::text AS revision,ctid::text AS position FROM freight_workspaces WHERE tenant_id='owned';")
    sql("catalog-upgrade", "BEGIN;\n" + migrations[20].read_text() + "\nCOMMIT;")
    sql("backfill-retention", f"""
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM original_workspace o JOIN freight_workspaces w ON w.tenant_id='owned'
        WHERE o.data=w.data AND o.updated_at=w.updated_at AND o.revision=w.xmin::text AND o.position=w.ctid::text)
    THEN RAISE EXCEPTION 'Backfill changed original physical records'; END IF;
    IF (SELECT count(*) FROM freight_load_search WHERE tenant_id='owned') <> {args.size}
    THEN RAISE EXCEPTION 'Backfill omitted shipment rows'; END IF;
    IF NOT (SELECT valid FROM freight_load_catalog WHERE tenant_id='owned')
    THEN RAISE EXCEPTION 'Backfill marked valid records invalid'; END IF;
    IF (SELECT encode(content,'hex') FROM freight_document_content WHERE tenant_id='owned') <> '255044462d312e340a6f726967696e616c'
    THEN RAISE EXCEPTION 'Backfill changed original file'; END IF;
END $$;
""")
    sql("incremental-upgrade", "CREATE TABLE before_incremental_upgrade AS SELECT tenant_id,load_id,xmin::text AS revision,ctid::text AS position,summary FROM freight_load_search;\nBEGIN;\n" + migrations[21].read_text() + "\nCOMMIT;")
    sql("incremental-upgrade-retention", """
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM before_incremental_upgrade b FULL JOIN freight_load_search s USING(tenant_id,load_id)
        WHERE b.load_id IS NULL OR s.load_id IS NULL OR b.revision<>s.xmin::text OR b.position<>s.ctid::text OR b.summary<>s.summary)
    THEN RAISE EXCEPTION 'Incremental migration rewrote the existing catalog'; END IF;
    IF NOT EXISTS (SELECT 1 FROM original_workspace o JOIN freight_workspaces w ON w.tenant_id='owned'
        WHERE o.data=w.data AND o.updated_at=w.updated_at AND o.revision=w.xmin::text AND o.position=w.ctid::text)
    THEN RAISE EXCEPTION 'Incremental migration rewrote original source records'; END IF;
END $$;
""")
    sql("transactional-updates", """
CREATE TABLE original_projection AS SELECT load_id,xmin::text AS revision,summary FROM freight_load_search;
BEGIN;
UPDATE freight_workspaces SET data=jsonb_set(data,'{loads,0,location}','"Literal 100%_ AUSTIN"') WHERE tenant_id='owned';
DO $$ BEGIN
    IF (SELECT summary->>'location' FROM freight_load_search WHERE load_id='catalog-owned-00000') <> 'Literal 100%_ AUSTIN'
    THEN RAISE EXCEPTION 'Source and index not atomic'; END IF;
    IF EXISTS (SELECT 1 FROM freight_load_search s JOIN original_projection o USING(load_id)
        WHERE s.load_id<>'catalog-owned-00000' AND (s.summary<>o.summary OR s.xmin::text<>o.revision))
    THEN RAISE EXCEPTION 'Unrelated index rows were rewritten'; END IF;
END $$;
COMMIT;
BEGIN;
UPDATE freight_workspaces SET data=jsonb_set(data,'{loads,0,location}','"Rolled back"') WHERE tenant_id='owned';
ROLLBACK;
DO $$ BEGIN
    IF (SELECT summary->>'location' FROM freight_load_search WHERE load_id='catalog-owned-00000') <> 'Literal 100%_ AUSTIN'
    THEN RAISE EXCEPTION 'Index survived rolled back source'; END IF;
    IF (SELECT count(*) FROM freight_load_search WHERE strpos(search_text,'100%_ austin')>0) <> 1
    THEN RAISE EXCEPTION 'Literal search mismatch'; END IF;
END $$;
CREATE TABLE committed_projection AS SELECT load_id,xmin::text AS revision,summary FROM freight_load_search;
UPDATE freight_workspaces SET data=jsonb_set(data,'{events}',data->'events') WHERE tenant_id='owned';
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM freight_load_search s JOIN committed_projection o USING(load_id)
        WHERE s.summary<>o.summary OR s.xmin::text<>o.revision)
    THEN RAISE EXCEPTION 'History-only write touched catalog rows'; END IF;
END $$;
""")
    page_query = (ROOT / "src/load_catalog_page.sql").read_text().strip().rstrip(";")
    prepared = "PREPARE catalog(text,text,text,text,text,text,bigint) AS " + page_query + ";\n"
    observed = sql("bounded-query-plan", "ANALYZE freight_load_search; ANALYZE freight_load_catalog;\n" + prepared + "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE catalog('owned','',NULL,NULL,NULL,'',26);\n"
        "SELECT jsonb_build_object('full_snapshot_bytes',(SELECT octet_length(data::text) FROM freight_workspaces WHERE tenant_id='owned'),"
        "'catalog_rows',(SELECT count(*) FROM freight_load_search WHERE tenant_id='owned'),"
        "'page_rows',(SELECT count(*) FROM (SELECT summary FROM freight_load_search WHERE tenant_id='owned' ORDER BY load_id LIMIT 25) p),"
        "'page_summary_bytes',(SELECT sum(octet_length(summary::text)) FROM (SELECT summary FROM freight_load_search WHERE tenant_id='owned' ORDER BY load_id LIMIT 25) p)) AS report;")
    match = re.search(r'QUERY PLAN = "(.*?)"\s*\(typeid', observed, re.S)
    require(match is not None, "Query plan output missing")
    plan = json.loads(match.group(1))
    require('freight_workspaces' not in json.dumps(plan), "Catalog query accessed whole-workspace history")
    if args.size >= 1000:
        require('freight_load_search_pkey' in json.dumps(plan), "Default keyset page did not use its tenant/id index")
    require(plan[0]["Plan"]["Actual Rows"] <= 26, "Query returned more than limit+one")
    match = re.search(r'report = "(.*?)"\s*\(typeid', observed, re.S)
    require(match is not None, "Payload observations missing")
    payload = json.loads(match.group(1))
    require(payload["page_rows"] <= 25, "Page limit not retained")
    write_plans = []
    # Observe maintenance cost as well as the smaller read payload. Each pair
    # changes the same source value and rolls back; alternate order, retaining
    # every sample. These are SQL observations, not HTTP or deployment timings.
    for trial in range(3):
        order = [False,True] if trial % 2 == 0 else [True,False]
        for enabled in order:
            label = "enabled" if enabled else "disabled"
            setup = "" if enabled else "ALTER TABLE freight_workspaces DISABLE TRIGGER freight_load_catalog_refresh;\n"
            output = sql(f"write-{trial}-{label}", "BEGIN;\n" + setup +
                "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) UPDATE freight_workspaces SET data=jsonb_set(data,'{loads,0,location}','\"Maintenance observation\"') WHERE tenant_id='owned';\nROLLBACK;")
            match = re.search(r'QUERY PLAN = "(.*?)"\s*\(typeid', output, re.S)
            require(match is not None, "Write observation missing")
            write_plans.append({"trial":trial,"catalog_maintenance":label,"plan":json.loads(match.group(1))})
    comparison = []
    if args.compare_maintenance:
        # Restore the exact released function bodies inside each preceding
        # observation, then roll back both definitions and the source write.
        # The current schema/source/fixture and update value otherwise match.
        released = migrations[20].read_text()
        preceding = released[released.index("CREATE FUNCTION freight_project_load_catalog"):released.index("CREATE TRIGGER freight_load_catalog_refresh")].replace("CREATE FUNCTION", "CREATE OR REPLACE FUNCTION")
        writes = {
            "single": "jsonb_set(data,'{loads,0,location}','\"Maintenance comparison\"')",
            "reorder": "jsonb_set(data,'{loads}',(SELECT jsonb_agg(item ORDER BY position DESC) FROM jsonb_array_elements(data->'loads') WITH ORDINALITY f(item,position)))",
            "bulk": "jsonb_set(data,'{loads}',(SELECT jsonb_agg(jsonb_set(item,'{location}','\"Maintenance comparison\"') ORDER BY position) FROM jsonb_array_elements(data->'loads') WITH ORDINALITY f(item,position)))",
            "history_only": "jsonb_set(data,'{events,0,detail}','\"Maintenance comparison\"')",
        }
        for workload, expression in writes.items():
            for trial in range(5):
                for version in (["preceding", "current"] if trial % 2 == 0 else ["current", "preceding"]):
                    setup = preceding if version == "preceding" else ""
                    statement = "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) UPDATE freight_workspaces SET data=" + expression + " WHERE tenant_id='owned';"
                    output = sql(f"compare-{workload}-{trial}-{version}", "BEGIN;\n" + setup + "\n" + statement + "\nROLLBACK;")
                    match = re.search(r'QUERY PLAN = "(.*?)"\s*\(typeid', output, re.S)
                    require(match is not None, "Comparison observation missing")
                    comparison.append({"workload": workload, "trial": trial, "version": version, "plan": json.loads(match.group(1))})
        # Keep instrumented nested-statement profiles separate from the timing
        # comparison above: auto_explain itself adds measurement overhead.
        for version in ("preceding", "current"):
            setup = preceding if version == "preceding" else ""
            sql("profile-" + version, "LOAD 'auto_explain'; SET auto_explain.log_min_duration=0; SET auto_explain.log_analyze=true; SET auto_explain.log_nested_statements=true;\nBEGIN;\n" + setup + "\nUPDATE freight_workspaces SET data=" + writes["single"] + " WHERE tenant_id='owned';\nROLLBACK;")
    sql("damage-repair-and-isolation", """
BEGIN;
UPDATE freight_workspaces SET data=jsonb_set(data,'{loads}','{}') WHERE tenant_id='owned';
DO $$ BEGIN
    IF (SELECT valid FROM freight_load_catalog WHERE tenant_id='owned') OR EXISTS(SELECT 1 FROM freight_load_search WHERE tenant_id='owned')
    THEN RAISE EXCEPTION 'Malformed collection became a successful partial catalog'; END IF;
END $$;
ROLLBACK;
BEGIN;
UPDATE freight_workspaces SET data=jsonb_set(data,'{loads}',jsonb_build_array(data->'loads'->0,data->'loads'->0)) WHERE tenant_id='owned';
DO $$ BEGIN
    IF (SELECT valid FROM freight_load_catalog WHERE tenant_id='owned')
    THEN RAISE EXCEPTION 'Duplicate shipment identity was accepted'; END IF;
END $$;
ROLLBACK;
INSERT INTO freight_workspaces(tenant_id,data) SELECT 'other',data FROM freight_workspaces WHERE tenant_id='owned';
DO $$ BEGIN
    IF (SELECT count(*) FROM freight_load_search WHERE tenant_id='other') <> (SELECT count(*) FROM freight_load_search WHERE tenant_id='owned')
    THEN RAISE EXCEPTION 'Tenant projection mismatch'; END IF;
END $$;
DELETE FROM freight_workspaces WHERE tenant_id='other';
DO $$ BEGIN
    IF EXISTS(SELECT 1 FROM freight_load_search WHERE tenant_id='other') OR EXISTS(SELECT 1 FROM freight_load_catalog WHERE tenant_id='other')
    THEN RAISE EXCEPTION 'Tenant deletion retained orphan catalog rows'; END IF;
    IF NOT (SELECT valid FROM freight_load_catalog WHERE tenant_id='owned')
    THEN RAISE EXCEPTION 'Rolled back damage did not restore projection validity'; END IF;
END $$;
""")
    sql("save-resolution-format-upgrade", "CREATE TABLE pre_resolution AS SELECT data,updated_at FROM freight_workspaces WHERE tenant_id='owned';\nBEGIN;\n" + migrations[22].read_text() + "\nCOMMIT;\n" + "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pre_resolution old JOIN freight_workspaces current ON current.tenant_id='owned' WHERE old.data=jsonb_set(current.data,'{_freight_format}','17'::jsonb) AND old.updated_at=current.updated_at) THEN RAISE EXCEPTION 'Save-resolution upgrade changed original business records'; END IF; IF (SELECT version FROM freight_storage_format WHERE singleton)<>18 THEN RAISE EXCEPTION 'Save-resolution format did not advance'; END IF; END $$;\n")
    sql("history-index-upgrade", "CREATE TABLE pre_history_index AS SELECT data,updated_at,xmin::text AS revision FROM freight_workspaces WHERE tenant_id='owned';\nBEGIN;\n" + migrations[23].read_text() + "\nCOMMIT;\n" + "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pre_history_index old JOIN freight_workspaces current ON current.tenant_id='owned' WHERE old.data=current.data AND old.updated_at=current.updated_at AND old.revision=current.xmin::text) THEN RAISE EXCEPTION 'History index changed original physical business records'; END IF; IF NOT EXISTS (SELECT 1 FROM freight_workspaces w JOIN freight_history_state s USING(tenant_id) WHERE w.tenant_id='owned' AND s.ready AND s.source_revision=w.history_revision) THEN RAISE EXCEPTION 'History index was not backfilled'; END IF; END $$;\n")
    sql("tender-deadline-upgrade", "CREATE TABLE pre_tender_deadline AS SELECT data,updated_at,xmin::text AS revision FROM freight_workspaces WHERE tenant_id='owned';\nBEGIN;\n" + migrations[24].read_text() + "\nCOMMIT;\n" + "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pre_tender_deadline old JOIN freight_workspaces current ON current.tenant_id='owned' WHERE old.data=current.data AND old.updated_at=current.updated_at AND old.revision=current.xmin::text) THEN RAISE EXCEPTION 'Deadline backfill changed original source'; END IF; IF NOT EXISTS(SELECT 1 FROM freight_tender_deadlines WHERE tenant_id='owned' AND ready) THEN RAISE EXCEPTION 'Deadline backfill missing'; END IF; END $$;")
    require(fingerprint(ROOT) == source, "Source changed during qualification")
    report = {"result": "pass", "scope": "owned standalone PostgreSQL SQL qualification; no listening sockets, sqlx transport, concurrent sessions, browser or service latency claim",
              "source": source, "artifacts": str(directory), "phases": phases,
              "fixture": {"sha256": digest(fixture), "binary_sha256": digest(fixture_binary.read_bytes()), "generator_sha256": digest((ROOT / 'examples/service_capacity_fixture.rs').read_bytes())},
              "query_sha256": digest((ROOT / 'src/load_catalog_page.sql').read_bytes()), "query_plan": plan, "payload_observation": payload,"write_observations":write_plans,
              "maintenance_comparison": comparison,
              "harness_sha256": digest(Path(__file__).read_bytes()), "runtime_harness_sha256":digest((ROOT/'scripts/runtime_contracts.py').read_bytes()), "postgres_version": subprocess.check_output([str(args.postgres_bin/'postgres'),'--version'],text=True).strip(),
              "limitations": ["Synthetic fixture and PostgreSQL standalone execution on a shared host; no controlled deployment throughput claim", "Payload sizes exclude HTTP envelope and do not measure request latency", "Substring search still scans candidate rows; catalog maintenance scans both load arrays on changed loads", "Preceding comparison restores exact migration-021 function bodies in the current schema, not an earlier executable", "Production backups, concurrent transport, browser tests and actual EDI/Sync recovery need separate qualification"]}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps({"result":"pass","output":str(args.output),"payload_observation":payload}))


if __name__ == "__main__":
    main()
