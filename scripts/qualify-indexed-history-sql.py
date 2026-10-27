#!/usr/bin/env python3
"""Check history migration and exact page SQL in an owned standalone database.

This provides SQL evidence when listening sockets are unavailable. It does not
qualify SQLx transport, concurrent transactions, HTTP latency or service recovery.
Every run creates a new temporary cluster and retains its SQL and logs.
"""

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from runtime_contracts import ROOT


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--postgres-bin", type=Path, default=Path("/usr/lib/postgresql/12/bin")
    )
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Preserve the existing report; choose a new output path")
    root = ROOT
    bin_dir = args.postgres_bin
    out = Path(tempfile.mkdtemp(prefix="freight-indexed-history-sql-offline-"))
    data_dir = out / "data"
    sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
    source = {
        str(p.relative_to(root)): sha(p)
        for p in sorted((root / "migrations").glob("*.sql"))
    }
    source["src/history_page.sql"] = sha(root / "src/history_page.sql")
    source["scripts/qualify-indexed-history-sql.py"] = sha(Path(__file__))
    source["scripts/runtime_contracts.py"] = sha(root / "scripts/runtime_contracts.py")
    report = {
        "result": "running",
        "scope": "Supplemental real SQL assertions in an owned standalone PostgreSQL backend. No sockets, HTTP, SQLx transport, concurrent readers or resource-budget qualification.",
        "source": source,
        "checks": [],
        "started_at": dt.datetime.now(dt.timezone.utc).isoformat(),
    }

    def chunk(sql):
        # Single-user -j terminates commands on a blank line, including blank lines
        # inside dollar-quoted bodies. Strip only empty lines from these fixed inputs.
        return "\n".join(line for line in sql.splitlines() if line.strip()) + "\n\n"

    def literal(v):
        return (
            "'"
            + json.dumps(v, separators=(",", ":"), ensure_ascii=False).replace(
                "'", "''"
            )
            + "'::jsonb"
        )

    work = {
        "_freight_format": 18,
        "loads": [
            {
                "id": "load_1",
                "version": 7,
                "status": "delivered",
                "customer_id": "customer_1",
            }
        ],
        "events": [],
        "appointments": [],
        "stop_events": [],
    }
    for field in ["events", "appointments", "stop_events"]:
        for i in range(311):
            row = {
                "id": field + "-original-" + str(i),
                "entity_id": "load_1" if i % 3 else "other",
                "load_id": "load_1" if i % 3 else "other",
                "original": {
                    "ordinal": i,
                    "unicode": "\u6771\u4eac",
                    "quote": "a'b",
                    "opaque": [None, False, {"x": i}],
                },
            }
            work[field].append(row)
    work["events"].append(
        {
            "id": "oversized-identity-" * 700,
            "entity_id": "unqueryable-scope-" * 30,
            "details": {"original": "retained complete"},
        }
    )
    commands = [
        chunk("SET log_min_messages=notice;"),
        chunk(
            "BEGIN;\n"
            + "\n".join(
                p.read_text()
                for p in sorted((root / "migrations").glob("*.sql"))
                if int(p.name[:3]) < 24
            )
            + "\nCOMMIT;"
        ),
    ]
    commands.append(
        chunk(
            f"INSERT INTO freight_workspaces(tenant_id,data) VALUES('owned',{literal(work)});"
        )
    )
    commands.append(
        chunk(
            "CREATE TEMP TABLE original_source AS SELECT tenant_id,xmin::text stamp,updated_at,md5(data::text) checksum FROM freight_workspaces; PREPARE owned_old_writer(jsonb) AS UPDATE freight_workspaces SET data=$1 WHERE tenant_id='owned';"
        )
    )
    commands.append(
        chunk(
            "BEGIN;\n"
            + (root / "migrations/024_indexed_history.sql").read_text()
            + "\nCOMMIT;"
        )
    )
    commands.append(
        chunk(
            "BEGIN;\n"
            + (root / "migrations/025_tender_deadlines.sql").read_text()
            + "\nCOMMIT;"
        )
    )
    checks = []

    def check(label, body):
        checks.append(label)
        commands.append(
            chunk(
                "DO $check$ BEGIN "
                + body
                + " RAISE NOTICE 'FREIGHT_CHECK_OK: "
                + label
                + "'; END $check$;"
            )
        )

    check(
        "backfill preserves source JSON timestamps and physical row",
        "IF EXISTS(SELECT 1 FROM freight_workspaces w JOIN original_source o USING(tenant_id) WHERE w.xmin::text<>o.stamp OR w.updated_at<>o.updated_at OR md5(w.data::text)<>o.checksum) THEN RAISE EXCEPTION 'source changed by backfill'; END IF;",
    )
    page_sql = (root / "src/history_page.sql").read_text()
    commands.append(
        chunk(
            'CREATE FUNCTION owned_page(text,text,text,bigint,bigint,bigint,text) RETURNS TABLE(marker_valid boolean,valid boolean,load_count bigint,load_version text,source_length bigint,upper_bound bigint,anchor text,total bigint,newer boolean,"position" bigint,record text) LANGUAGE sql AS $page$\n'
            + page_sql
            + "\n$page$;"
        )
    )
    # A PL/pgSQL reader consumes the exact product SELECT and compares every
    # requested original JSON value and ordinal, with real lookahead pagination.
    commands.append(
        chunk(
            """CREATE FUNCTION owned_verify(p_tenant text) RETURNS void LANGUAGE plpgsql AS $verify$
    DECLARE k text; scope text; expected jsonb; actual jsonb; before_pos bigint; next_before bigint; observed bigint; expected_count bigint; source_count bigint; tail_hash text; meta record;
    BEGIN
     FOREACH k IN ARRAY ARRAY['events','appointments','stop_events'] LOOP
      FOREACH scope IN ARRAY ARRAY[NULL::text,'load_1'] LOOP
       IF k<>'events' AND scope IS NULL THEN CONTINUE; END IF;
       SELECT jsonb_array_length(data->k),encode(sha256(convert_to(data->k->(jsonb_array_length(data->k)-1)->>'id','UTF8')),'hex') INTO source_count,tail_hash FROM freight_workspaces WHERE tenant_id=p_tenant;
       SELECT count(*) INTO expected_count FROM freight_workspaces w, jsonb_array_elements(w.data->k) v WHERE w.tenant_id=p_tenant AND (scope IS NULL OR v->>CASE WHEN k='events' THEN 'entity_id' ELSE 'load_id' END=scope);
       before_pos:=NULL; observed:=0;
       LOOP
        SELECT * INTO meta FROM owned_page(p_tenant,k,scope,NULL,before_pos,18,'18') LIMIT 1;
        IF NOT meta.marker_valid OR NOT meta.valid OR meta.load_count<>1 OR meta.source_length<>source_count OR meta.upper_bound<>source_count OR meta.anchor IS DISTINCT FROM tail_hash OR meta.total<>expected_count OR meta.newer THEN RAISE EXCEPTION 'wrong metadata %, %',k,scope; END IF;
        SELECT coalesce(jsonb_agg(jsonb_build_object('position',position,'record',record::jsonb) ORDER BY position DESC),'[]'::jsonb),min(position) INTO actual,next_before FROM (SELECT position,record FROM owned_page(p_tenant,k,scope,NULL,before_pos,18,'18') WHERE position IS NOT NULL ORDER BY position DESC LIMIT 17) p;
        SELECT coalesce(jsonb_agg(jsonb_build_object('position',ordinal,'record',value) ORDER BY ordinal DESC),'[]'::jsonb) INTO expected FROM (SELECT ordinal,value FROM freight_workspaces w,jsonb_array_elements(w.data->k) WITH ORDINALITY v(value,ordinal) WHERE w.tenant_id=p_tenant AND (before_pos IS NULL OR ordinal<before_pos) AND (scope IS NULL OR value->>CASE WHEN k='events' THEN 'entity_id' ELSE 'load_id' END=scope) ORDER BY ordinal DESC LIMIT 17) original;
        IF actual IS DISTINCT FROM expected THEN RAISE EXCEPTION 'original page changed %, %, %',k,scope,before_pos; END IF;
        observed:=observed+jsonb_array_length(actual);
        EXIT WHEN jsonb_array_length(actual)<17;
        IF before_pos IS NOT NULL AND next_before>=before_pos THEN RAISE EXCEPTION 'cursor did not advance'; END IF;
        before_pos:=next_before;
       END LOOP;
       IF observed<>expected_count THEN RAISE EXCEPTION 'missing original records'; END IF;
      END LOOP;
     END LOOP;
    END $verify$;"""
        )
    )
    check(
        "every selected original across global and scoped multi-page histories",
        "PERFORM owned_verify('owned');",
    )
    commands.append(
        chunk(
            f"INSERT INTO freight_workspaces(tenant_id,data) VALUES('cold',{literal(work)});"
        )
    )
    check(
        "committed insertion invalidates without eagerly building",
        "IF NOT EXISTS(SELECT 1 FROM freight_history_state WHERE tenant_id='cold' AND NOT ready) OR EXISTS(SELECT 1 FROM freight_history_scopes WHERE tenant_id='cold') THEN RAISE EXCEPTION 'cold state not invalid'; END IF;",
    )
    commands.append(
        chunk(
            "INSERT INTO original_source SELECT tenant_id,xmin::text,updated_at,md5(data::text) FROM freight_workspaces WHERE tenant_id='cold'; SELECT freight_ensure_history('cold');"
        )
    )
    check(
        "cold read rebuilds all original histories without source rewrite",
        "PERFORM owned_verify('cold'); IF EXISTS(SELECT 1 FROM freight_workspaces w JOIN original_source o USING(tenant_id) WHERE w.xmin::text<>o.stamp OR w.updated_at<>o.updated_at OR md5(w.data::text)<>o.checksum) THEN RAISE EXCEPTION 'source changed by read'; END IF;",
    )
    changed = json.loads(json.dumps(work))
    changed["events"].pop()
    commands.append(chunk(f"EXECUTE owned_old_writer({literal(changed)});"))
    check(
        "already prepared preceding writer invalidates at commit",
        "IF NOT EXISTS(SELECT 1 FROM freight_workspaces w JOIN freight_history_state s USING(tenant_id) WHERE w.tenant_id='owned' AND w.history_revision=1 AND NOT s.ready) THEN RAISE EXCEPTION 'old writer did not invalidate'; END IF;",
    )
    commands.append(chunk("SELECT freight_ensure_history('owned');"))
    check(
        "old writer data rebuilds exact source and portable revision",
        "PERFORM owned_verify('owned'); IF NOT EXISTS(SELECT 1 FROM freight_workspaces w JOIN freight_history_state s USING(tenant_id) WHERE w.tenant_id='owned' AND w.history_revision=s.source_revision AND s.ready) THEN RAISE EXCEPTION 'stale revision'; END IF;",
    )
    commands.append(
        chunk(
            "BEGIN; UPDATE freight_workspaces SET data=data WHERE tenant_id='owned'; UPDATE freight_workspaces SET data=data WHERE tenant_id='owned'; COMMIT;"
        )
    )
    check(
        "multiple updates leave final revision invalidated",
        "IF NOT EXISTS(SELECT 1 FROM freight_workspaces w JOIN freight_history_state s USING(tenant_id) WHERE w.tenant_id='owned' AND w.history_revision=3 AND NOT s.ready) THEN RAISE EXCEPTION 'wrong final revision'; END IF;",
    )
    commands.append(
        chunk(
            "SELECT freight_ensure_history('owned'); DELETE FROM freight_history_scopes WHERE tenant_id='owned' AND kind='events' AND scope_id='other';"
        )
    )
    check(
        "deleted scope cannot look like complete empty history",
        "IF EXISTS(SELECT 1 FROM owned_page('owned','events','load_1',NULL,NULL,18,'18') WHERE valid) THEN RAISE EXCEPTION 'damaged index accepted'; END IF;",
    )
    commands.append(chunk("SELECT freight_rebuild_history('owned');"))
    check(
        "explicit repair restores all original pages", "PERFORM owned_verify('owned');"
    )
    check(
        "unsorted duplicate empty zero and null ordinal arrays rejected",
        "DECLARE invalid bigint[]; BEGIN FOREACH invalid SLICE 1 IN ARRAY ARRAY[ARRAY[2,1],ARRAY[1,1],ARRAY[0,2],ARRAY[1,NULL]] LOOP BEGIN INSERT INTO freight_history_scopes VALUES('owned','events','bad',invalid); RAISE EXCEPTION 'invalid ordinals accepted'; EXCEPTION WHEN check_violation THEN NULL; END; END LOOP; BEGIN INSERT INTO freight_history_scopes VALUES('owned','events','bad',ARRAY[]::bigint[]); RAISE EXCEPTION 'empty ordinals accepted'; EXCEPTION WHEN check_violation THEN NULL; END; END;",
    )
    # Verify exact canonical plans in all cache modes, with unrelated scopes.
    extra = [
        {"id": f"plan-event-{i}", "entity_id": f"plan-scope-{i:05}", "original": i}
        for i in range(1800)
    ]
    commands.append(
        chunk(
            "UPDATE freight_workspaces SET data=jsonb_set(data,'{events}',(data->'events')||"
            + literal(extra)
            + ") WHERE tenant_id='owned'; SELECT freight_ensure_history('owned'); ANALYZE freight_history_scopes; ANALYZE freight_history_loads;"
        )
    )
    commands.append(
        chunk(
            "PREPARE owned_history(text,text,text,bigint,bigint,bigint,text) AS\n"
            + page_sql
            + ";"
        )
    )
    for mode in ["force_generic_plan", "force_custom_plan", "auto"]:
        commands.append(chunk(f"SET plan_cache_mode={mode};"))
        if mode == "auto":
            commands.append(
                chunk(
                    "EXECUTE owned_history('owned','events','load_1',NULL,NULL,18,'18');\n"
                    * 6
                )
            )
        # Explain via dynamic command retains exact actual prepared product statement.
        check(
            "exact page scope index " + mode,
            "DECLARE p json; scans jsonb; BEGIN EXECUTE 'EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE owned_history(''owned'',''events'',''load_1'',NULL,NULL,18,''18'')' INTO p; WITH RECURSIVE nodes(v) AS (SELECT p::jsonb->0->'Plan' UNION ALL SELECT child FROM nodes,jsonb_array_elements(coalesce(v->'Plans','[]'::jsonb)) child) SELECT jsonb_agg(v) INTO scans FROM nodes WHERE v->>'Relation Name'='freight_history_scopes'; IF scans IS NULL OR EXISTS(SELECT 1 FROM jsonb_array_elements(scans) n WHERE n->>'Node Type' NOT IN ('Index Scan','Index Only Scan') OR n->>'Index Cond' NOT LIKE '%tenant_id%' OR n->>'Index Cond' NOT LIKE '%kind%' OR n->>'Index Cond' NOT LIKE '%scope_id%' OR (n->>'Actual Rows')::numeric>1) THEN RAISE EXCEPTION 'unbounded scope plan %',p; END IF; END;",
        )
    commands.append(
        chunk(
            "TRUNCATE freight_load_search,freight_load_catalog,freight_workspaces,freight_document_content CASCADE;"
        )
    )
    check(
        "owned fixture reset cascades new history children",
        "IF EXISTS(SELECT 1 FROM freight_history_state) OR EXISTS(SELECT 1 FROM freight_history_scopes) THEN RAISE EXCEPTION 'history children not reset'; END IF;",
    )
    commands.append(chunk("SELECT 'FREIGHT_ALL_SQL_CHECKS_FINISHED';"))
    try:
        init = subprocess.run(
            [
                str(bin_dir / "initdb"),
                "-D",
                str(data_dir),
                "-A",
                "trust",
                "--no-locale",
                "-E",
                "UTF8",
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        (out / "initdb.log").write_text(init.stdout)
        assert init.returncode == 0, init.stdout
        sql = "".join(commands)
        (out / "probe.sql").write_text(sql)
        p = subprocess.run(
            [
                str(bin_dir / "postgres"),
                "--single",
                "-j",
                "-D",
                str(data_dir),
                "postgres",
            ],
            input=sql,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        (out / "postgres.log").write_text(p.stdout)
        passed = re.findall(r"NOTICE:  FREIGHT_CHECK_OK: ([^\n]+)", p.stdout)
        assert (
            p.returncode == 0
            and "ERROR:" not in p.stdout
            and "FATAL:" not in p.stdout
            and "PANIC:" not in p.stdout
        ), p.stdout[-4000:]
        assert passed == checks, (passed, checks)
        assert "FREIGHT_ALL_SQL_CHECKS_FINISHED" in p.stdout, "missing end marker"
        assert all(sha(root / p) == h for p, h in source.items()), "source drift"
        report.update(
            result="pass",
            checks=passed,
            postgres_version=subprocess.check_output(
                [str(bin_dir / "postgres"), "--version"], text=True
            ).strip(),
        )
    except BaseException as e:
        report.update(result="failed", error=str(e))
        print(str(e)[-3000:])
    finally:
        report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        report["artifacts"] = {
            p.name: {"path": str(p), "sha256": sha(p)}
            for p in out.iterdir()
            if p.is_file()
        }
        (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        with args.output.open("x") as stream:
            stream.write((out / "report.json").read_text())
        print(
            json.dumps(
                {
                    "report": str(args.output),
                    "artifacts": str(out),
                    "result": report["result"],
                    "checks": len(report["checks"]),
                }
            )
        )
    return 0 if report["result"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
