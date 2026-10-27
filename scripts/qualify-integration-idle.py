#!/usr/bin/env python3
"""Measure physical idle writes with configured integration workers and retained history.

Only owned loopback Freight processes and a labelled disposable PostgreSQL are used.
The upstream URL is an unused loopback endpoint; this experiment sends no freight.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import time

from runtime_contracts import ROOT, Runtime, fingerprint, literal, require

KEY = "owned-integration-idle-synthetic-credential"


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Idle(Runtime):
    def __init__(self, args):
        super().__init__(args)
        self.secrets.append(KEY)
        self.source = fingerprint(ROOT)
        self.harness = {
            name: sha(ROOT / name)
            for name in (
                "scripts/qualify-integration-idle.py",
                "scripts/runtime_contracts.py",
                "examples/service_capacity_fixture.rs",
            )
        }
        self.report.update(
            scope="owned configured-worker idle writes; not a capacity or latency SLA",
            callback_transport={
                "public_https_delivery": "not exercised",
                "local_test_relay": "not exercised; idle queues and unused upstream",
                "native_public_address_policy": "not exercised by idle experiment",
            },
            harness=self.harness,
            settings={
                "shipments": args.shipments,
                "idle_seconds": args.seconds,
                "api_async_threads": 2,
                "api_build_profile": "unoptimized debug",
                "api_resource_quota": "not imposed",
                "postgres_cpu_quota": 2,
                "postgres_memory_mib": 768,
                "postgres_data_tmpfs_mib": 512,
                "integration_workers": "enabled and tenant configured",
                "upstream": "unused loopback URL; no queued work or upstream messages",
            },
            observations=[],
        )
        for relative in self.harness:
            shutil.copyfile(ROOT / relative, self.directory / Path(relative).name)

    def build(self):
        baseline = json.loads(
            (ROOT / "docs/evidence/recovery-qualification.json").read_text()
        )["builds"]["freight"]
        require(
            fingerprint(self.args.baseline_source) == baseline["source"],
            "Baseline source differs from retained qualification",
        )
        env = dict(
            os.environ,
            RUSTC_WRAPPER="",
            CARGO_INCREMENTAL="0",
            CARGO_BUILD_JOBS="2",
            CARGO_TARGET_DIR=str(ROOT / "target"),
        )
        self.baseline_binary = self.directory / "baseline-freight"
        if self.args.baseline_binary:
            require(
                sha(self.args.baseline_binary) == baseline["binary_sha256"],
                "Baseline binary differs from retained qualification",
            )
            shutil.copy2(self.args.baseline_binary, self.baseline_binary)
            provenance = (
                "retained binary matching the recorded source/binary qualification"
            )
        else:
            # Different local packages with the same name/version must not
            # share mutable Cargo outputs. Copy dependency cache bytes only.
            baseline_target = self.directory / "baseline-target"

            def ignore_freight(_directory, names):
                return [
                    name
                    for name in names
                    if name in ("incremental", ".cargo-lock", "examples")
                    or name.startswith(
                        ("stateset-freight", "stateset_freight", "libstateset_freight")
                    )
                ]

            shutil.copytree(
                ROOT / "target/debug", baseline_target / "debug", ignore=ignore_freight
            )
            baseline_env = {**env, "CARGO_TARGET_DIR": str(baseline_target)}
            with (self.directory / "baseline-build.log").open("w") as log:
                subprocess.run(
                    [
                        "cargo",
                        "build",
                        "--offline",
                        "--locked",
                        "--manifest-path",
                        str(self.args.baseline_source.resolve() / "Cargo.toml"),
                        "--bin",
                        "stateset-freight",
                    ],
                    cwd=self.directory,
                    env=baseline_env,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    check=True,
                )
            require(
                fingerprint(self.args.baseline_source) == baseline["source"],
                "Baseline source changed during build",
            )
            shutil.copy2(
                baseline_target / "debug/stateset-freight", self.baseline_binary
            )
            shutil.rmtree(baseline_target)
            provenance = "built locked/offline from the exact reconstructed baseline source in this run"
        with (self.directory / "build.log").open("w") as log:
            subprocess.run(
                [
                    "cargo",
                    "build",
                    "--offline",
                    "--locked",
                    "--bin",
                    "stateset-freight",
                    "--example",
                    "service_capacity_fixture",
                ],
                cwd=ROOT,
                env=env,
                stdout=log,
                stderr=subprocess.STDOUT,
                check=True,
            )
        require(fingerprint(ROOT) == self.source, "Source changed during build")
        self.current_binary = self.directory / "current-freight"
        self.generator = self.directory / "fixture-generator"
        shutil.copy2(ROOT / "target/debug/stateset-freight", self.current_binary)
        shutil.copy2(
            ROOT / "target/debug/examples/service_capacity_fixture", self.generator
        )
        require(
            sha(self.current_binary) != sha(self.baseline_binary),
            "Different migration sources produced identical executables; build isolation failed",
        )
        self.report["builds"] = {
            "before": {
                "source": baseline["source"],
                "binary_sha256": sha(self.baseline_binary),
                "provenance": provenance,
            },
            "after": {"source": self.source, "binary_sha256": sha(self.current_binary)},
            "fixture_binary_sha256": sha(self.generator),
        }
        self.clock = dt.datetime.now(dt.timezone.utc).isoformat()
        self.fixture = subprocess.check_output(
            [str(self.generator), str(self.args.shipments), "idle-owned", self.clock],
            cwd=self.directory,
        )
        (self.directory / "fixture.json").write_bytes(self.fixture)
        self.report["fixture"] = {
            "clock": self.clock,
            "sha256": hashlib.sha256(self.fixture).hexdigest(),
            "counts": json.loads(self.fixture)["counts"],
        }

    def start(self, binary):
        self.binaries = {"freight": binary}
        self.spawn(
            "freight",
            {
                "DATABASE_URL": f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime",
                "FREIGHT_DEMO": "false",
                "TOKIO_WORKER_THREADS": "2",
                "FREIGHT_BIND_ADDR": f"127.0.0.1:{self.ports['freight']}",
                "FREIGHT_API_KEYS_JSON": json.dumps(
                    {
                        KEY: {
                            "tenant_id": "idle",
                            "subject": "owned:idle",
                            "role": "operations",
                        }
                    }
                ),
                "FREIGHT_INTEGRATIONS_JSON": json.dumps(
                    {
                        "idle": {
                            "edi_url": "http://127.0.0.1:9",
                            "edi_token": "owned-unused-loopback-token",
                            "edi_tenant_id": "owned-idle",
                        }
                    }
                ),
            },
        )
        self.wait(
            lambda: self.http("freight", "/health", token=KEY),
            "owned Freight ready",
            60,
        )

    def snapshot(self):
        return self.rows(
            "freight_runtime",
            "SELECT xmin::text AS revision,ctid::text,updated_at,md5(data::text) AS data_md5,octet_length(data::text) AS json_bytes FROM freight_workspaces WHERE tenant_id='idle'",
        )[0]

    def phase(self, name, binary):
        # Apply this binary's schema before seeding and clearing trace history.
        self.start(binary)
        self.stop("freight")
        source = self.args.baseline_source if name == "before" else ROOT
        expected_migrations = len(
            list((source / "migrations").glob("[0-9][0-9][0-9]_*.sql"))
        )
        applied_migrations = int(
            self.sql(
                "freight_runtime", "SELECT count(*) FROM freight_schema_migrations"
            )
        )
        require(
            applied_migrations == expected_migrations,
            "Running binary applied a different source's migration inventory",
        )
        self.sql(
            "freight_runtime", "TRUNCATE freight_load_search,freight_load_catalog,freight_workspaces,freight_document_content CASCADE"
        )
        data = json.loads(self.fixture)["workspace"]
        data["_freight_format"] = int(self.sql("freight_runtime", "SELECT version FROM freight_storage_format WHERE singleton"))
        url = f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime"
        self.command(
            ["psql", url, "-X", "-v", "ON_ERROR_STOP=1"],
            label="owned exact idle fixture",
            input="INSERT INTO freight_workspaces(tenant_id,data) VALUES('idle',"
            + literal(json.dumps(data))
            + "::jsonb);",
            env={"PATH": os.defpath, "LC_ALL": "C"},
        )
        if name == "before":
            self.sql(
                "freight_runtime",
                "CREATE TABLE idle_write_trace(id bigserial PRIMARY KEY); CREATE FUNCTION idle_trace() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO idle_write_trace DEFAULT VALUES; RETURN NEW; END $$; CREATE TRIGGER idle_trace AFTER UPDATE ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION idle_trace()",
            )
        self.sql("freight_runtime", "TRUNCATE idle_write_trace RESTART IDENTITY")
        before = self.snapshot()
        wal = self.sql("freight_runtime", "SELECT pg_current_wal_lsn()")
        self.start(binary)
        started = time.monotonic()
        while time.monotonic() - started < self.args.seconds:
            require(
                self.processes["freight"].poll() is None,
                "Owned API stopped during idle observation",
            )
            self.http("freight", "/health", token=KEY)
            time.sleep(0.25)
        self.stop("freight")
        after = self.snapshot()
        count = int(
            self.sql("freight_runtime", "SELECT count(*) FROM idle_write_trace")
        )
        observed_wal = int(
            self.sql(
                "freight_runtime",
                "SELECT pg_wal_lsn_diff(pg_current_wal_lsn(),"
                + literal(wal)
                + "::pg_lsn)::bigint",
            )
        )
        require(
            before["data_md5"] == after["data_md5"],
            "Idle worker changed business evidence",
        )
        observation = {
            "name": name,
            "applied_migrations": applied_migrations,
            "before": before,
            "after": after,
            "committed_workspace_updates": count,
            "physical_row_unchanged": before == after,
            "observed_cluster_wal_bytes": observed_wal,
            "observed_seconds_after_health": round(time.monotonic() - started, 3),
        }
        if name == "before":
            require(
                count > 0 and before != after,
                "Earlier source did not reproduce idle writes",
            )
        else:
            require(
                count == 0 and before == after, "Idle polls rewrote current workspace"
            )
            observation["deadline_row"] = self.rows(
                "freight_runtime",
                "SELECT inbox_due_at,edi_due_at,sync_due_at FROM freight_workspaces WHERE tenant_id='idle'",
            )[0]
            require(
                all(v is None for v in observation["deadline_row"].values()),
                "Empty queues have a wakeup",
            )
            statement = (
                "SELECT coalesce(inbox_due_at <= "
                + literal(dt.datetime.now(dt.timezone.utc).isoformat())
                + "::text::timestamptz,false) FROM freight_workspaces WHERE tenant_id='idle'"
            )
            observation["deadline_query_plan"] = json.loads(
                self.sql(
                    "freight_runtime",
                    "EXPLAIN (ANALYZE,BUFFERS,VERBOSE,FORMAT JSON) " + statement,
                )
            )
        self.report["observations"].append(observation)
        self.step(
            f"{name} configured-worker idle observation retains original evidence",
            committed_workspace_updates=count,
            physical_row_unchanged=before == after,
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-source", type=Path, required=True)
    parser.add_argument(
        "--baseline-binary",
        type=Path,
        help="Optional retained binary matching the historical attestation; otherwise builds the baseline source",
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--shipments", type=int, default=1000)
    parser.add_argument("--seconds", type=int, default=35)
    parser.add_argument("--postgres-image", default="postgres:16")
    args = parser.parse_args()
    if (
        args.output.exists()
        or not 1 <= args.shipments <= 5000
        or not 16 <= args.seconds <= 120
    ):
        parser.error("output must be new; shipments 1–5000; seconds 16–120")
    os.umask(0o077)
    run = Idle(args)

    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()

    signal.signal(signal.SIGTERM, interrupted)
    try:
        run.build()
        run.postgres()
        run.phase("before", run.baseline_binary)
        run.phase("after", run.current_binary)
        require(run.source == fingerprint(ROOT), "Source changed during qualification")
        require(
            all(sha(ROOT / path) == h for path, h in run.harness.items()),
            "Harness changed during qualification",
        )
        require(
            sha(run.current_binary) == run.report["builds"]["after"]["binary_sha256"],
            "Current binary changed during qualification",
        )
        require(
            sha(run.baseline_binary) == run.report["builds"]["before"]["binary_sha256"],
            "Baseline binary changed during qualification",
        )
        require(
            fingerprint(args.baseline_source)
            == run.report["builds"]["before"]["source"],
            "Baseline source changed during qualification",
        )
        run.report["result"] = "pass"
    except BaseException as error:
        run.report["result"] = "failed"
        run.report["error"] = run.safe(f"{type(error).__name__}: {error}")
        print(run.report["error"], file=sys.stderr, flush=True)
    finally:
        try:
            run.cleanup()
        except BaseException as error:
            run.report["cleanup_error"] = run.safe(str(error))
            run.report["result"] = "failed"
        run.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        run.save()
        with args.output.open("x") as stream:
            stream.write((run.directory / "report.json").read_text())
        print("REPORT", run.directory / "report.json", flush=True)
    return 0 if run.report["result"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
