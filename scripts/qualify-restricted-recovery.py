#!/usr/bin/env python3
"""Run the complete image restore drill with a separate Freight runtime login.

The schema job uses the unmodified API image and its default nonroot user.
The serving API receives only business DML, metadata reads and the single
column permission needed by the format fence. All inherited recovery checks
remain active. The controller retains the role/grant recipe across cluster
loss; this is not lost-identity, password, TLS or production commissioning.
"""

import argparse
from collections import Counter
import copy
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import sys

from migration_ledger import verify_migration_ledger

from runtime_contracts import ROOT, fingerprint, literal, require

spec = importlib.util.spec_from_file_location(
    "restricted_container_base", ROOT / "scripts/qualify-container-recovery.py"
)
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
identifier = base.base.identifier

SCHEMA_CHECK = (
    "actual nonroot image schema-only job verifies the exact source migration ledger "
    "without seeding or rewriting migration metadata"
)
LEGACY_SCHEMA_CHECK = (
    "actual nonroot image schema-only job verifies exact28 "
    "without seeding or rewriting migration metadata"
)


def recovery_check_inventory(checks):
    """Retain inherited assertions while removing the old count from its ID."""
    return Counter(SCHEMA_CHECK if item["name"] == LEGACY_SCHEMA_CHECK
                   else item["name"] for item in checks)


def required_recovery_checks(path=None):
    """Load the public inherited contract without private execution reports."""
    path = Path(path) if path is not None else Path(__file__).with_name("recovery-check-inventory.json")
    value = json.loads(path.read_text())
    require(type(value.get("schema")) is int and value["schema"] == 1,
            "Unsupported recovery inventory schema")
    checks = value.get("checks")
    require(isinstance(checks, list) and checks, "Recovery inventory must be nonempty")
    result = Counter()
    for item in checks:
        require(isinstance(item, dict) and set(item) == {"name", "count"}
                and isinstance(item["name"], str) and item["name"]
                and type(item["count"]) is int and item["count"] > 0
                and item["name"] not in result,
                "Invalid or duplicate recovery inventory entry")
        result[item["name"]] = item["count"]
    return result


class RestrictedRecovery(base.ContainerRecovery):
    def __init__(self, args):
        super().__init__(args)
        self.runtime_role = "freight_app_" + self.container_label.replace("-", "")
        self.schema_generations = set()
        self.migration_jobs = []
        self.role_snapshots = []
        self.report["scope"] = (
            "Complete actual Freight/EDI/Sync/web image recovery with a separate "
            "schema job and a restricted Freight runtime database login; "
            "all inherited business, schema and original-file checks retained"
        )
        self.report["harness"]["scripts/qualify-restricted-recovery.py"] = base.sha(
            __file__
        )
        shutil.copyfile(__file__, self.directory / Path(__file__).name)
        self.report["runtime_database_role"] = self.runtime_role
        self.report["limitations"] += [
            "PostgreSQL host authentication is trust in this isolated lab: login authorization is exercised, password verification and TLS are not.",
            "The controller retains the explicit role/grant recipe and recreates it on the replacement cluster; role-inventory backup or recovery from lost configuration is not exercised.",
            "Only Freight uses the restricted runtime login; sibling EDI/Sync retain their original database-owner profiles.",
        ]

    def role_sql(self, statement):
        return self.command(
            [
                "psql",
                f"postgres://{self.runtime_role}@127.0.0.1:{self.ports['postgres']}/freight_runtime",
                "-X",
                "-At",
                "-v",
                "ON_ERROR_STOP=1",
                "-v",
                "VERBOSITY=verbose",
                "-c",
                statement,
            ],
            label="owned restricted-role SQL",
            env={"PATH": os.defpath, "LC_ALL": "C"},
        )

    def schema_job(self):
        generation = self.pg_generation
        if generation in self.schema_generations:
            return
        previous = None
        if generation:
            previous = self.rows(
                "freight_runtime",
                "SELECT version,checksum,applied_at FROM freight_schema_migrations ORDER BY version",
            )
        config = {
            "DATABASE_URL": "postgres://freight_runtime@127.0.0.1:5432/freight_runtime",
            "FREIGHT_MIGRATIONS": "verify",
            "FREIGHT_DEMO": "true",
            "RUST_LOG": "warn",
        }
        command = [
            "docker",
            "run",
            "-d",
            "--pull=never",
            "--name",
            f"freight-schema-{self.container_label}-{generation}",
            "--label",
            "com.stateset.freight.runtime=" + self.container_label,
            "--network",
            "container:" + self.container,
        ]
        for key, value in config.items():
            command += ["-e", key + "=" + value]
        identity = self.command(
            command + [self.images["freight"], "--migrate"],
            label="actual image schema-only job",
        )
        self.migration_jobs.append(identity)
        item = self.inspect_container(identity)
        require(item["Image"] == self.images["freight"], "Schema job image changed")
        require(
            item["Config"]["User"] == "10001:10001", "Schema job changed image user"
        )
        require(
            item["HostConfig"]["NetworkMode"] == "container:" + self.container,
            "Schema job escaped owned namespace",
        )
        code = self.command(
            ["docker", "wait", identity],
            label="actual schema job completion",
            timeout=120,
        )
        log = self.command(["docker", "logs", identity], label="actual schema job log")
        with (self.directory / f"schema-job-{generation}.log").open("x") as out:
            out.write(log + "\n")
        require(
            code == "0" and "Freight schema migrated and verified" in log,
            "Schema-only image job failed",
        )
        installed = self.rows(
            "freight_runtime",
            "SELECT version,checksum,applied_at FROM freight_schema_migrations ORDER BY version",
        )
        expected = verify_migration_ledger(self.source["snapshots"]["freight"], installed)
        require(
            self.sql("freight_runtime", "SELECT version FROM freight_storage_format")
            == "21",
            "Actual schema job format changed",
        )
        if previous is None:
            require(
                self.sql("freight_runtime", "SELECT count(*) FROM freight_workspaces")
                == "0",
                "Schema-only image job seeded a tenant",
            )
        else:
            require(
                installed == previous,
                "Restore schema job rewrote original migration metadata",
            )
        self.report.setdefault("schema_jobs", []).append(
            {
                "postgres_generation": generation,
                "id": identity,
                "image": item["Image"],
                "user": item["Config"]["User"],
                "exit_code": int(code),
                "installed_migrations": installed,
            }
        )
        self.install_role()
        self.schema_generations.add(generation)
        self.step(
            SCHEMA_CHECK,
            postgres_generation=generation,
            migration_count=len(expected),
        )

    def install_role(self):
        role = identifier(self.runtime_role)
        self.sql(
            "postgres",
            f"CREATE ROLE {role} LOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS",
        )
        business = self.rows(
            "freight_runtime",
            "SELECT tablename FROM pg_tables WHERE schemaname='public' AND tablename NOT IN ('freight_schema_migrations','freight_storage_format') ORDER BY tablename",
        )
        statements = [
            "REVOKE ALL ON DATABASE freight_runtime FROM PUBLIC",
            f"GRANT CONNECT ON DATABASE freight_runtime TO {role}",
            "REVOKE CREATE ON SCHEMA public FROM PUBLIC",
            f"GRANT USAGE ON SCHEMA public TO {role}",
            f"GRANT SELECT ON ALL TABLES IN SCHEMA public TO {role}",
            f"GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA public TO {role}",
            f"GRANT UPDATE(singleton) ON freight_storage_format TO {role}",
        ]
        statements += [
            f"GRANT INSERT,UPDATE,DELETE ON {identifier(x['tablename'])} TO {role}"
            for x in business
        ]
        self.sql("freight_runtime", ";".join(statements))
        require(
            self.role_sql(
                "SELECT current_user=session_user AND current_user="
                + literal(self.runtime_role)
            )
            == "t",
            "Runtime connection did not use its actual login",
        )
        for statement in [
            "BEGIN; CREATE TABLE public.owned_denied_probe(id int); ROLLBACK",
            "BEGIN; UPDATE freight_schema_migrations SET checksum=checksum; ROLLBACK",
            "BEGIN; UPDATE freight_storage_format SET version=version; ROLLBACK",
        ]:
            try:
                self.role_sql(statement)
            except RuntimeError as error:
                require(
                    "42501" in str(error),
                    "Restricted SQL failed for a reason other than permission",
                )
            else:
                raise AssertionError("Restricted login accepted privileged SQL")

    def spawn(self, name, config):
        if name == "freight":
            self.schema_job()
            config = copy.deepcopy(config)
            config["DATABASE_URL"] = (
                f"postgres://{self.runtime_role}@127.0.0.1:{self.ports['postgres']}/freight_runtime"
            )
            config["FREIGHT_MIGRATIONS"] = "verify"
        return super().spawn(name, config)

    def database_manifest(self, database):
        # The inherited restore compares complete manifests before starting any
        # application. Recreate the retained role recipe at this exact boundary.
        if database == "freight_runtime" and self.pg_generation > 0:
            self.schema_job()
        return super().database_manifest(database)

    def role_snapshot(self):
        role = literal(self.runtime_role)
        attributes = self.rows(
            "freight_runtime",
            f"SELECT rolname,rolsuper,rolinherit,rolcreaterole,rolcreatedb,rolcanlogin,rolreplication,rolbypassrls FROM pg_roles WHERE rolname={role}",
        )
        require(len(attributes) == 1, "Missing runtime login")
        flags = attributes[0]
        require(
            flags["rolcanlogin"]
            and not any(
                v for k, v in flags.items() if k not in ("rolname", "rolcanlogin")
            ),
            "Runtime login acquired elevated role attributes",
        )
        memberships = self.rows(
            "freight_runtime",
            f"SELECT parent.rolname AS parent FROM pg_auth_members m JOIN pg_roles child ON child.oid=m.member JOIN pg_roles parent ON parent.oid=m.roleid WHERE child.rolname={role} ORDER BY parent.rolname",
        )
        require(not memberships, "Runtime login acquired membership")
        privileges = self.rows(
            "freight_runtime",
            f"SELECT has_database_privilege({role},current_database(),'CONNECT') AS connect,has_database_privilege({role},current_database(),'CREATE') AS create,has_database_privilege({role},current_database(),'TEMP') AS temp,has_schema_privilege({role},'public','USAGE') AS usage,has_schema_privilege({role},'public','CREATE') AS schema_create,has_column_privilege({role},'freight_storage_format','singleton','UPDATE') AS fence_lock,has_column_privilege({role},'freight_storage_format','version','UPDATE') AS version_write",
        )
        require(
            privileges
            == [
                {
                    "connect": True,
                    "create": False,
                    "temp": False,
                    "usage": True,
                    "schema_create": False,
                    "fence_lock": True,
                    "version_write": False,
                }
            ],
            "Runtime metadata or DDL permissions changed",
        )
        tables = self.rows(
            "freight_runtime",
            f"SELECT c.relname,pg_get_userbyid(c.relowner) AS owner,has_table_privilege({role},c.oid,'SELECT') AS read,has_table_privilege({role},c.oid,'INSERT') AS insert,has_table_privilege({role},c.oid,'UPDATE') AS update,has_table_privilege({role},c.oid,'DELETE') AS delete,has_table_privilege({role},c.oid,'TRUNCATE') AS truncate,has_table_privilege({role},c.oid,'TRIGGER') AS trigger FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p') ORDER BY c.relname",
        )
        require(tables, "Runtime table inventory empty")
        for table in tables:
            metadata = table["relname"] in (
                "freight_schema_migrations",
                "freight_storage_format",
            )
            require(
                table["owner"] == "freight_runtime"
                and table["read"]
                and not table["truncate"]
                and not table["trigger"],
                "Runtime table ownership or read rights changed",
            )
            require(
                all(
                    table[key] == (not metadata)
                    for key in ("insert", "update", "delete")
                ),
                "Runtime metadata or business write rights changed",
            )
        return {
            "attributes": attributes,
            "memberships": memberships,
            "privileges": privileges,
            "tables": tables,
        }

    def verify_image_health(self):
        super().verify_image_health()
        sessions = self.rows(
            "freight_runtime",
            "SELECT DISTINCT usename FROM pg_stat_activity WHERE datname=current_database() AND backend_type='client backend' AND pid<>pg_backend_pid() ORDER BY usename",
        )
        require(
            sessions == [{"usename": self.runtime_role}],
            "Serving API connected with a different database role",
        )
        snapshot = self.role_snapshot()
        self.role_snapshots.append(snapshot)
        self.report["runtime_role_observations"] = self.role_snapshots
        self.step(
            "healthy actual API uses restricted login with business DML and immutable schema metadata permissions",
            postgres_generation=self.pg_generation,
        )

    def verify_save_resolution(self):
        super().verify_save_resolution()
        require(
            len(self.role_snapshots) == 2
            and self.role_snapshots[0]
            == self.role_snapshots[1]
            == self.role_snapshot(),
            "Replacement cluster changed runtime role or grants",
        )
        self.step(
            "replacement cluster recreates the exact retained role/grant recipe without elevating the serving API"
        )

    def remove_schema_jobs(self):
        for identity in list(self.migration_jobs):
            item = self.inspect_container(identity)
            if item["State"]["Running"]:
                self.command(
                    ["docker", "stop", "--time", "15", identity],
                    label="owned schema job stop",
                )
            self.command(
                ["docker", "rm", "--volumes", identity],
                label="owned schema job removal",
            )
            self.migration_jobs.remove(identity)
        self.report["schema_job_cleanup"] = "all owned schema job containers removed"

    def remove_apps_and_volume(self):
        # Remove stopped namespace dependants before losing their PG anchor.
        self.remove_schema_jobs()
        super().remove_apps_and_volume()

    def cleanup(self):
        self.remove_schema_jobs()
        super().cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.bundle, args.output = args.bundle.resolve(), args.output.resolve()
    require(not args.output.exists(), "Preserve earlier reports")
    args.sync_repo = args.bundle / "workspace/stateset-sync-server"
    args.edi_repo = args.bundle / "workspace/stateset-edi"
    args.build_dir, args.postgres_image = args.bundle, "pinned"
    os.umask(0o077)
    run = RestrictedRecovery(args)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        run.build()
        run.postgres()
        run.sync()
        run.edi()
        run.freight()
        run.start_web()
        run.checks()
        run.report["baseline_checks"] = len(run.report["checks"])
        run.record_recovery_cash()
        run.record_save_resolution()
        run.prepare_outage()
        run.backup_and_lose()
        run.restore()
        run.recovered_work()
        run.verify_save_resolution()
        require(
            run.report.get("edi_volume_restored"),
            "Actual EDI volume restore not witnessed",
        )
        for relative, digest in run.report["harness"].items():
            require(
                base.sha(ROOT / relative) == digest,
                "Recovery harness changed during run",
            )
        for name in ("freight", "edi", "sync"):
            require(
                fingerprint(Path(run.source["snapshots"][name]))
                == run.source["sources"][name],
                "Frozen service source drift",
            )
            for relative, digest in run.source["inputs"][name].items():
                require(
                    base.sha(Path(run.source["snapshots"][name]) / relative) == digest,
                    "Frozen Docker input drift",
                )
            require(
                base.sha(run.binaries[name])
                == run.report["builds"][name]["binary_sha256"],
                "Image binary drift",
            )
        run.report["result"] = "pass"
    except BaseException as error:
        run.report.update(
            result="failed", error=run.safe(type(error).__name__ + ": " + str(error))
        )
        print(run.report["error"], file=sys.stderr, flush=True)
    finally:
        try:
            run.cleanup()
        except BaseException as error:
            run.report.update(result="failed", cleanup_error=run.safe(str(error)))
        run.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        run.save()
        with args.output.open("x") as out:
            out.write((run.directory / "report.json").read_text())
        print("REPORT", args.output, flush=True)
    return 0 if run.report["result"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
