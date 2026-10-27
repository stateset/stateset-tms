#!/usr/bin/env python3
"""Restore owned Freight/EDI/Sync backups into a genuinely new PostgreSQL cluster.

Uses only the synthetic, labelled, loopback services of runtime_contracts.py.
It never accepts a deployment database URL, production keys, or a remote host.
"""

import argparse
import copy
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import sys
import time

from runtime_contracts import (
    CALLBACK_SECRET,
    CUSTOMER_KEY,
    EDI_TENANT,
    OTHER_TENANT,
    ROOT,
    Runtime,
    fingerprint,
    literal,
    require,
)

DATABASES = ("sync_runtime", "edi_runtime", "freight_runtime")
FINANCIAL_ROOTS = (
    "invoices",
    "settlements",
    "bills",
    "credits",
    "cash_entries",
    "journal",
    "billing_profile",
    "charges",
    "carrier_adjustments",
    "carrier_documents",
    "carrier_credit_applications",
    "cancellation_fees",
)


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def digest(value):
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def identifier(value):
    return '"' + value.replace('"', '""') + '"'


def file_manifest(directory):
    require(
        directory.is_dir() and not directory.is_symlink(),
        "Expected owned data directory",
    )
    entries = {}
    for path in sorted(directory.rglob("*")):
        require(not path.is_symlink(), "Recovery fixture must not contain symlinks")
        info = path.stat()
        entry = {
            "mode": stat.S_IMODE(info.st_mode),
            "kind": "directory" if path.is_dir() else "file",
        }
        if path.is_file():
            entry.update(size=info.st_size, sha256=sha(path), mtime_ns=info.st_mtime_ns)
        else:
            require(path.is_dir(), "Unsupported recovery fixture file type")
        entries[str(path.relative_to(directory))] = entry
    return entries


class Recovery(Runtime):
    def __init__(self, args):
        super().__init__(args)
        self.configs = {}
        self.graceful_stops = []
        self.manifest_generations = {}
        self.report[
            "plan_date_profile"
        ] = "Tomorrow's planned service, verified actual pickup and cleared reconciliation today; no historical prepayments"
        self.report[
            "scope"
        ] = "owned coordinated backup/loss/restore and queue recovery against actual sibling services"
        self.report["harness"] = {
            str(path.relative_to(ROOT)): sha(path)
            for path in (
                Path(__file__).resolve(),
                ROOT / "scripts/runtime_contracts.py",
                ROOT / "scripts/native_postgres.py",
            )
        }
        for relative in self.report["harness"]:
            shutil.copyfile(ROOT / relative, self.directory / Path(relative).name)
        self.report["limitations"] = [
            "Synthetic quiesced recovery point; not a production RPO, RTO, availability or carrier certification claim.",
            "Original service configuration and encryption/signing keys are supplied separately and retained in the test controller.",
            "No public HTTPS callback delivery, remote secret manager, WAL/PITR, replica failover or live upstream drain is qualified.",
            "The configured EDI profile uses PostgreSQL and explicit keys; a labelled binary canary tests data-directory restoration.",
        ]

    def spawn(self, name, config):
        require(
            sha(self.binaries[name]) == self.report["builds"][name]["binary_sha256"],
            f"{name} binary changed after the recorded build",
        )
        self.configs[name] = copy.deepcopy(config)
        super().spawn(name, config)

    def interrupt(self, name):
        process = self.processes[name]
        require(process.poll() is None, f"{name} was not live before quiescing")
        process.send_signal(signal.SIGINT)
        # A timeout fails the drill; final cleanup still owns this process.
        code = process.wait(timeout=30)
        self.processes.pop(name)
        require(code == 0, f"{name} did not shut down cleanly: {code}")
        self.graceful_stops.append(name)

    def raw_workspace(self):
        return self.rows(
            "freight_runtime",
            "SELECT freight_workspace_originals(tenant_id) AS data FROM freight_workspaces WHERE tenant_id='demo'",
        )[0]["data"]

    def signed_callback(self, job):
        timestamp = int(time.time())
        signature = self.command(
            [str(self.binaries["wire"])],
            label="actual sibling recovery callback signer",
            input=json.dumps(
                {
                    "secret": CALLBACK_SECRET,
                    "timestamp": timestamp,
                    "delivery_id": job["id"],
                    "payload": job["payload"],
                }
            ),
        )
        return self.http(
            "freight",
            "/webhooks/edi/demo",
            method="POST",
            body=job["payload"],
            headers={
                "Content-Type": "application/json",
                "x-stateset-event-id": job["id"],
                "x-stateset-timestamp": str(timestamp),
                "x-stateset-key-version": str(job["version"]),
                "x-stateset-signature": "v1=" + signature,
            },
            expected=(200, 202),
        )

    def record_save_resolution(self):
        self.stopped_save = {"request_key": "recovery-stopped-original", "path": "customers", "body": {"name": "Owned stopped original after restore", "contact": "owned@example.test"}, "note": "Prevent the uncertain original from applying after recovery"}
        self.stopped_result = self.mutate("save-recovery/resolve", self.stopped_save, key="owned-resolution-controller")
        require(self.stopped_result["outcome"] == "stopped", "Original request was not stopped")
        self.step("a durable original-request stop is recorded before backup without creating its customer", request_key=self.stopped_save["request_key"], resolution=self.stopped_result)

    def verify_save_resolution(self):
        response = self.http("freight", "/v1/customers", method="POST", body=self.stopped_save["body"], headers={"Idempotency-Key": self.stopped_save["request_key"]}, expected=409)
        require(response["error"]["code"] == "request_stopped", "Restored stop allowed a delayed original request")
        repeated = self.mutate("save-recovery/resolve", self.stopped_save, key="owned-resolution-controller-again")
        require(repeated == self.stopped_result, "Restored resolution lost its original actor/time/note")
        current = self.raw_workspace()
        require(not any(customer["name"] == self.stopped_save["body"]["name"] for customer in current["customers"]), "Delayed stopped request created a customer")
        require(current["receipts"][self.stopped_save["request_key"]] == self.before["receipts"][self.stopped_save["request_key"]], "Restored stopped receipt changed")
        self.step("fresh-cluster restore retains the exact stop and rejects a delayed original without duplicate business application", resolution=repeated)

    def record_recovery_cash(self):
        workspace = self.raw_workspace()
        bill = next(
            b for b in workspace["bills"] if b["number"] == "RUNTIME-CUSTOMER-150001"
        )
        settlement = next(
            s for s in workspace["settlements"] if s["load_id"] == bill["load_id"]
        )
        for direction, target, amount, reference in (
            ("receipt", bill, 150001, "OWNED-SYNTHETIC-CUSTOMER-CLEARED"),
            ("payout", settlement, 125001, "OWNED-SYNTHETIC-CARRIER-CLEARED"),
        ):
            self.mutate(
                "cash",
                {
                    "direction": direction,
                    "currency": "USD",
                    "account": "Owned-synthetic-USD",
                    "external_reference": reference,
                    "amount_cents": amount,
                    "cleared_on": dt.datetime.now(dt.timezone.utc).date().isoformat(),
                    "note": "Owned synthetic cleared bank statement; no payment execution",
                    "allocations": [
                        {
                            "target_id": target["id"],
                            "expected_version": target["version"],
                            "amount_cents": amount,
                        }
                    ],
                },
            )
        entries = self.raw_workspace()["cash_entries"]
        require(
            len(entries) == 2
            and sum(
                e["amount_cents"] if e["direction"] == "receipt" else -e["amount_cents"]
                for e in entries
            )
            == 25000,
            "Recovery fixture did not retain the independent expected cash margin",
        )
        self.step(
            "synthetic cleared bank receipt and payout retain 25000-cent net cash before backup",
            customer_receipt_cents=150001,
            carrier_payout_cents=125001,
            net_cash_cents=25000,
        )
        load = next(load for load in workspace["loads"] if load["id"] == bill["load_id"])
        pickup = next(
            source for source in workspace["carrier_submissions"]
            if source["load_id"] == load["id"]
            and source["tender_id"] == load["tender_id"]
            and source["carrier_id"] == load["carrier_id"]
            and source["kind"] == "pickup" and source["outcome"] == "applied"
        )
        corrections = [c for c in workspace["movement_corrections"] if c["source_id"] == pickup["id"]]
        effective = corrections[-1]["observed_at"] if corrections else pickup["observed_at"]
        payout = next(entry for entry in entries if entry["direction"] == "payout")
        require(
            effective[:10] <= payout["cleared_on"] < load["origin"]["date"],
            "Early actual pickup did not reconcile against the unchanged later plan",
        )
        self.step(
            "verified actual pickup permits cleared carrier payout before the unchanged planned service date",
            planned_origin_date=load["origin"]["date"],
            effective_pickup_observed_at=effective,
            cleared_on=payout["cleared_on"],
            original_pickup_submission_id=pickup["id"],
        )

    def prepare_outage(self):
        source = self.seed_order("RECOVERY-INBOUND-ORDER")
        body = {"order_id": source, "load": self.plan("RECOVERY-INBOUND")}
        original = self.mutate("sync/import", body, key="recovery-original-import")
        self.command_load(original["id"], "tender", carrier_id="ca4")
        outbound = self.wait(
            lambda: next(
                (
                    j
                    for j in self.workspace()["outbox"]
                    if j["load_id"] == original["id"]
                    and j["destination"] == "stateset-edi"
                    and j["status"] == "gateway_accepted"
                ),
                None,
            ),
            "recovery original 204",
        )
        tender = next(
            t
            for t in self.workspace()["tenders"]
            if t["id"] == self.load(original["id"])["tender_id"]
        )
        today = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%d")
        self.http(
            "edi",
            "/v1/edi/inbound/brex",
            method="POST",
            headers={"Content-Type": "application/edi-x12"},
            body=self.raw(
                "990", "GF", f"B1*BREX*{tender['shipment_reference']}*{today}*A~", 91
            ),
        )
        callbacks = self.rows(
            "edi_runtime",
            "SELECT id,document_id,version,payload FROM callback_deliveries WHERE tenant_id="
            + literal(EDI_TENANT)
            + " AND payload::jsonb->>'transaction_set'='990' ORDER BY created_at DESC",
        )
        require(callbacks, "Recovery 990 did not retain original callback bytes")
        callback = callbacks[0]
        self.interrupt("edi")
        received = self.signed_callback(callback)
        require(
            received.get("received") is True,
            "Outage callback was not acknowledged durably",
        )
        inbox = self.wait(
            lambda: next(
                (
                    i
                    for i in self.workspace()["inbox"]
                    if i["document_id"] == callback["document_id"]
                    and i["status"] == "retry"
                    and i["total_attempts"] >= 1
                ),
                None,
            ),
            "inbound failure retained during actual EDI outage",
        )
        require(
            self.load(original["id"])["status"] == "tendered",
            "Offline canonical fetch invented carrier acceptance",
        )
        second_order = self.seed_order("RECOVERY-OUTBOUND-ORDER")
        second = self.import_order(second_order, "RECOVERY-OUTBOUND")
        self.command_load(second["id"], "tender", carrier_id="ca4")
        retry = self.wait(
            lambda: next(
                (
                    j
                    for j in self.workspace()["outbox"]
                    if j["load_id"] == second["id"]
                    and j["destination"] == "stateset-edi"
                    and j["status"] == "retry"
                    and j["total_attempts"] >= 1
                ),
                None,
            ),
            "outbound failure retained during actual EDI outage",
        )
        require(
            retry["transmission"] == "attempted", "Outage erased the attempted handoff"
        )
        self.pending = {
            "source": source,
            "load_id": original["id"],
            "import_body": body,
            "import_response": original,
            "original_outbox": outbound,
            "callback": callback,
            "inbox": inbox,
            "outbound_order": second_order,
            "outbound_load": second["id"],
            "outbound_retry": retry,
        }
        self.step(
            "actual EDI outage retains signed inbound and attempted outbound work without inventing success",
            inbox_id=inbox["id"],
            outbox_id=retry["id"],
            inbound_attempts=inbox["total_attempts"],
            outbound_attempts=retry["total_attempts"],
        )

    def canonical_expression(self, database, table, expression):
        # PostgreSQL reparses deparsed CHECK/index predicates on logical restore.
        # A temporary view asks that same parser for a stable representation;
        # it executes no application rows and disappears with the psql session.
        statement = (
            "CREATE TEMP VIEW freight_recovery_expression AS SELECT ("
            + expression
            + ") AS value FROM "
            + table
            + "; SELECT to_json(pg_get_viewdef('pg_temp.freight_recovery_expression'::regclass,false))::text"
        )
        url = (
            f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/{database}"
        )
        return json.loads(
            self.command(
                ["psql", url, "-X", "-Atq", "-v", "ON_ERROR_STOP=1", "-c", statement],
                label="PostgreSQL expression canonicalization",
                env={"PATH": os.defpath, "LC_ALL": "C"},
            )
        )

    def canonical_catalog(self, database, original):
        catalog = copy.deepcopy(original)
        for constraint in catalog["constraints"]:
            if constraint["contype"] == "c":
                require(
                    constraint["definition"].startswith("CHECK "),
                    "Unexpected CHECK deparser output",
                )
                table = (
                    identifier(constraint["schema"])
                    + "."
                    + identifier(constraint["relation"])
                )
                constraint["definition"] = self.canonical_expression(
                    database, table, constraint["definition"][6:]
                )
        for index in catalog["indexes"]:
            if index["predicate"] is not None:
                suffix = " WHERE " + index["predicate"]
                require(
                    index["indexdef"].endswith(suffix),
                    "Index predicate differs from its catalogue definition",
                )
                index["indexdef"] = index["indexdef"][: -len(suffix)]
                table = (
                    identifier(index["schemaname"])
                    + "."
                    + identifier(index["tablename"])
                )
                index["predicate"] = self.canonical_expression(
                    database, table, index["predicate"]
                )
        return catalog

    def schema_dump(self, database):
        return self.command(
            [
                "docker",
                "exec",
                self.container,
                "pg_dump",
                "-U",
                "freight_runtime",
                "--schema-only",
                "--no-owner",
                "--no-privileges",
                "--quote-all-identifiers",
                database,
            ],
            label=f"{database} complete schema definition",
        )

    @staticmethod
    def normalize_dump_guard(schema):
        guards = re.findall(r"^\\(un)?restrict (\S+)$", schema, re.MULTILINE)
        if guards:
            require(
                len(guards) == 2
                and guards[0][0] == ""
                and guards[1][0] == "un"
                and guards[0][1] == guards[1][1],
                "Unexpected pg_dump guard-token structure",
            )
            schema = re.sub(
                r"^(\\(?:un)?restrict) \S+$",
                r"\1 OWNEDNORMALIZEDGUARDTOKEN",
                schema,
                flags=re.MULTILINE,
            )
        return schema

    def parsed_schema_reference(self, database, schema):
        reference = "recovery_schema_" + database
        self.sql("postgres", "CREATE DATABASE " + identifier(reference))
        path = self.directory / f"{database}-source-schema.sql"
        path.write_text(schema + "\n")
        self.command(
            ["docker", "cp", str(path), self.container + ":/tmp/recovery-schema.sql"],
            label="owned reference schema input",
        )
        self.command(
            [
                "docker",
                "exec",
                self.container,
                "psql",
                "-U",
                "freight_runtime",
                "-X",
                "--quiet",
                "--set=ON_ERROR_STOP=1",
                "--single-transaction",
                "--dbname=" + reference,
                "--file=/tmp/recovery-schema.sql",
            ],
            label="PostgreSQL reference schema parsing",
        )
        parsed = self.schema_dump(reference)
        self.sql("postgres", "DROP DATABASE " + identifier(reference))
        return parsed

    def database_manifest(self, database):
        generation = self.manifest_generations.get(database, 0) + 1
        self.manifest_generations[database] = generation
        tables = self.rows(
            database,
            "SELECT n.nspname AS schema,c.relname AS name,c.relkind FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p','m') AND n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' ORDER BY n.nspname,c.relname",
        )
        content = {}
        for table in tables:
            qualified = identifier(table["schema"]) + "." + identifier(table["name"])
            # Hash PostgreSQL's exact JSON text, not Python-rounded numeric values.
            rows = self.sql(
                database,
                f'SELECT to_jsonb(t)::text FROM {qualified} t ORDER BY to_jsonb(t)::text COLLATE "C"',
            )
            content[qualified] = {
                "rows": len(rows.splitlines()) if rows else 0,
                "sha256": hashlib.sha256(rows.encode()).hexdigest(),
            }
        sequences = self.rows(
            database,
            "SELECT schemaname,sequencename,sequenceowner,data_type,start_value,min_value,max_value,increment_by,cycle,cache_size FROM pg_sequences WHERE schemaname NOT IN ('pg_catalog','information_schema') ORDER BY schemaname,sequencename",
        )
        for sequence in sequences:
            qualified = (
                identifier(sequence["schemaname"])
                + "."
                + identifier(sequence["sequencename"])
            )
            sequence["state"] = self.rows(
                database, f"SELECT last_value,is_called FROM {qualified}"
            )[0]
        catalog = {
            "tables": tables,
            "columns": self.rows(
                database,
                "SELECT table_schema,table_name,column_name,ordinal_position,column_default,is_nullable,data_type,udt_schema,udt_name,character_maximum_length,numeric_precision,numeric_scale,datetime_precision,is_identity,identity_generation,identity_start,identity_increment,identity_maximum,identity_minimum,identity_cycle,is_generated,generation_expression FROM information_schema.columns WHERE table_schema NOT IN ('pg_catalog','information_schema') ORDER BY table_schema,table_name,ordinal_position",
            ),
            "constraints": self.rows(
                database,
                "SELECT n.nspname AS schema,c.relname AS relation,k.conname,k.contype,k.condeferrable,k.condeferred,k.convalidated,pg_get_constraintdef(k.oid,true) AS definition FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') ORDER BY n.nspname,c.relname,k.conname",
            ),
            "indexes": self.rows(
                database,
                "SELECT v.schemaname,v.tablename,v.indexname,v.indexdef,pg_get_expr(i.indpred,i.indrelid) AS predicate FROM pg_indexes v JOIN pg_namespace n ON n.nspname=v.schemaname JOIN pg_class c ON c.relnamespace=n.oid AND c.relname=v.indexname JOIN pg_index i ON i.indexrelid=c.oid WHERE v.schemaname NOT IN ('pg_catalog','information_schema') ORDER BY v.schemaname,v.tablename,v.indexname",
            ),
            "triggers": self.rows(
                database,
                "SELECT n.nspname AS schema,c.relname AS relation,t.tgname,t.tgenabled,pg_get_triggerdef(t.oid,true) AS definition FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE NOT t.tgisinternal AND n.nspname NOT IN ('pg_catalog','information_schema') ORDER BY n.nspname,c.relname,t.tgname",
            ),
            "functions": self.rows(
                database,
                "SELECT n.nspname AS schema,p.proname,pg_get_function_identity_arguments(p.oid) AS arguments,pg_get_functiondef(p.oid) AS definition FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE p.prokind IN ('f','p') AND n.nspname NOT IN ('pg_catalog','information_schema') ORDER BY n.nspname,p.proname,pg_get_function_identity_arguments(p.oid)",
            ),
            "extensions": self.rows(
                database, "SELECT extname,extversion FROM pg_extension ORDER BY extname"
            ),
            "views": self.rows(
                database,
                "SELECT schemaname,viewname,definition FROM pg_views WHERE schemaname NOT IN ('pg_catalog','information_schema') ORDER BY schemaname,viewname",
            ),
        }
        (self.directory / f"{database}-catalog-{generation}.json").write_text(
            json.dumps(catalog, indent=2) + "\n"
        )
        canonical_catalog = self.canonical_catalog(database, catalog)
        (self.directory / f"{database}-canonical-catalog-{generation}.json").write_text(
            json.dumps(canonical_catalog, indent=2) + "\n"
        )
        schema = self.schema_dump(database)
        raw_schema = self.normalize_dump_guard(schema)
        (self.directory / f"{database}-schema-{generation}.sql").write_text(
            raw_schema + "\n"
        )
        canonical_schema = self.normalize_dump_guard(
            self.parsed_schema_reference(database, schema)
            if generation == 1
            else schema
        )
        (self.directory / f"{database}-canonical-schema-{generation}.sql").write_text(
            canonical_schema + "\n"
        )
        self.report.setdefault("schema_observations", {}).setdefault(
            database, []
        ).append(
            {
                "generation": generation,
                "raw_sha256": hashlib.sha256(raw_schema.encode()).hexdigest(),
                "parsed_sha256": hashlib.sha256(canonical_schema.encode()).hexdigest(),
            }
        )
        large_objects = int(
            self.sql(database, "SELECT count(*) FROM pg_largeobject_metadata")
        )
        require(
            large_objects == 0,
            "This fixture profile requires a large-object recovery validator before qualifying PostgreSQL large objects",
        )
        return {
            "tables": content,
            "sequences": sequences,
            "catalog_sha256": digest(canonical_catalog),
            "catalog_counts": {key: len(value) for key, value in catalog.items()},
            "schema_sha256": hashlib.sha256(canonical_schema.encode()).hexdigest(),
            "large_objects": large_objects,
        }

    def validate_backups(self, backups):
        for database in DATABASES:
            record = backups[database]
            path = self.directory / "backups" / record["filename"]
            require(
                path.parent == self.directory / "backups"
                and path.is_file()
                and not path.is_symlink(),
                "Backup must be an owned regular file",
            )
            require(
                path.stat().st_size == record["bytes"]
                and sha(path) == record["sha256"],
                f"Backup integrity failure: {database}",
            )

    def backup_and_lose(self):
        self.unavailable_started = time.monotonic()
        self.interrupt("freight")
        self.interrupt("sync")
        for database in DATABASES:
            require(
                self.sql(
                    database,
                    "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND backend_type='client backend' AND pid<>pg_backend_pid()",
                )
                == "0",
                "Backup started with application database sessions still live",
            )
        self.before = self.raw_workspace()
        self.before_manifests = {
            database: self.database_manifest(database) for database in DATABASES
        }
        self.before_system = self.sql(
            "postgres", "SELECT system_identifier FROM pg_control_system()"
        )
        self.before_postgres = copy.deepcopy(self.report["postgres"])
        self.before_container = self.container
        self.before_event_sequence = int(
            self.sql("edi_runtime", "SELECT coalesce(max(id),0) FROM callback_events")
        )
        self.step(
            "all three actual services quiesce before the common logical recovery point",
            graceful_stops=list(self.graceful_stops),
            databases=self.before_manifests,
        )
        data = self.directory / "edi-data"
        self.report["edi_application_files_before_canary"] = len(
            [p for p in data.rglob("*") if p.is_file()]
        )
        (data / "owned-recovery-canary.bin").write_bytes(
            b"Owned filesystem recovery canary\x00\xff\x80\r\n"
        )
        (data / "owned-recovery-canary.bin").chmod(0o600)
        self.files_before = file_manifest(data)
        shutil.copytree(data, self.directory / "edi-data-backup")
        require(
            file_manifest(self.directory / "edi-data-backup") == self.files_before,
            "EDI data backup changed original file bytes or metadata",
        )
        folder = self.directory / "backups"
        folder.mkdir(mode=0o700)
        backups = {}
        for database in DATABASES:
            started = time.monotonic()
            filename = database + ".dump"
            self.command(
                [
                    "docker",
                    "exec",
                    self.container,
                    "pg_dump",
                    "-U",
                    "freight_runtime",
                    "--format=custom",
                    "--no-owner",
                    "--no-privileges",
                    "--file=/tmp/" + filename,
                    database,
                ],
                label=f"{database} matching-version logical backup",
            )
            path = folder / filename
            self.command(
                ["docker", "cp", self.container + ":/tmp/" + filename, str(path)],
                label=f"{database} private backup copy",
            )
            path.chmod(0o600)
            backups[database] = {
                "filename": filename,
                "bytes": path.stat().st_size,
                "sha256": sha(path),
                "dump_seconds": round(time.monotonic() - started, 3),
            }
        self.validate_backups(backups)
        self.backups = backups
        self.report["recovery_backups"] = backups
        self.report["edi_file_manifest"] = self.files_before
        self.report["configuration_sha256"] = {
            name: digest(config) for name, config in self.configs.items()
        }
        self.step(
            "matching PostgreSQL tools retain three checked archives and exact EDI data files"
        )
        corrupt = folder / "corrupt-freight.dump"
        shutil.copyfile(folder / backups["freight_runtime"]["filename"], corrupt)
        with corrupt.open("r+b") as stream:
            stream.seek(-1, 2)
            original = stream.read(1)
            stream.seek(-1, 2)
            stream.write(bytes([original[0] ^ 1]))
        invalid = copy.deepcopy(backups)
        invalid["freight_runtime"]["filename"] = corrupt.name
        try:
            self.validate_backups(invalid)
        except AssertionError as error:
            require(
                str(error) == "Backup integrity failure: freight_runtime",
                "Corruption check failed for an unrelated reason",
            )
        else:
            raise AssertionError("Corrupted restore input was accepted")
        require(
            self.container == self.before_container
            and self.raw_workspace() == self.before,
            "Rejected backup altered the original workspace",
        )
        self.step(
            "corrupted archive is rejected before any restore or original database destruction",
            corrupt_archive_sha256=sha(corrupt),
        )
        # Deliberately outside the frozen backup point: this must disappear.
        self.lost_canary = self.seed_order("POST-BACKUP-LOSS-CANARY")
        require(
            self.rows(
                "sync_runtime",
                "SELECT id FROM orders WHERE id="
                + literal(self.lost_canary)
                + "::uuid",
            ),
            "Loss canary was not committed after backup",
        )
        super().cleanup()
        shutil.rmtree(data)
        require(
            not data.exists(), "Original EDI data directory survived simulated loss"
        )
        self.step(
            "original labelled database cluster and EDI data directory are destroyed",
            original_cluster_identifier=self.before_system,
            post_backup_canary_id=self.lost_canary,
        )

    def restore(self):
        started = time.monotonic()
        # A mutable image tag must not select another PostgreSQL build mid-drill.
        self.args.postgres_image = self.before_postgres["image_id"]
        super().postgres()
        require(
            self.report["postgres"] == self.before_postgres,
            "Recovery PostgreSQL image/version changed",
        )
        require(
            self.container != self.before_container, "Restore reused original container"
        )
        system = self.sql(
            "postgres", "SELECT system_identifier FROM pg_control_system()"
        )
        require(
            system != self.before_system, "Restore reused original database cluster"
        )
        for database in DATABASES:
            require(
                self.rows(
                    database,
                    "SELECT tablename FROM pg_tables WHERE schemaname='public'",
                )
                == [],
                "Restore target was not empty",
            )
        self.validate_backups(self.backups)
        for database in DATABASES:
            filename = self.backups[database]["filename"]
            path = self.directory / "backups" / filename
            self.command(
                ["docker", "cp", str(path), self.container + ":/tmp/" + filename],
                label=f"{database} restore input",
            )
            self.command(
                [
                    "docker",
                    "exec",
                    self.container,
                    "pg_restore",
                    "-U",
                    "freight_runtime",
                    "--exit-on-error",
                    "--single-transaction",
                    "--no-owner",
                    "--no-privileges",
                    "--dbname=" + database,
                    "/tmp/" + filename,
                ],
                label=f"{database} atomic restore",
            )
        shutil.copytree(self.directory / "edi-data-backup", self.directory / "edi-data")
        require(
            file_manifest(self.directory / "edi-data") == self.files_before,
            "Restored EDI files differ",
        )
        after = {database: self.database_manifest(database) for database in DATABASES}
        for database in DATABASES:
            # Retain specific differences if this check fails.
            if after[database] != self.before_manifests[database]:
                self.report["restore_manifest_mismatch"] = {
                    "database": database,
                    "before": self.before_manifests[database],
                    "after": after[database],
                }
            require(
                after[database] == self.before_manifests[database],
                f"Restored {database} content, sequences or schema differ",
            )
        self.report["restore_validation_seconds"] = round(time.monotonic() - started, 3)
        self.step(
            "fresh cluster restores every application table, sequence, schema definition and original EDI file",
            restored_cluster_identifier=system,
            restored_manifest_sha256=digest(after),
            restored_table_count=sum(len(x["tables"]) for x in after.values()),
        )
        require(
            not self.rows(
                "sync_runtime",
                "SELECT id FROM orders WHERE id="
                + literal(self.lost_canary)
                + "::uuid",
            ),
            "Restore unexpectedly included the post-backup canary",
        )
        self.step(
            "recovery point excludes deliberately later activity instead of claiming zero data loss",
            absent_post_backup_canary_id=self.lost_canary,
        )
        self.sql(
            "freight_runtime",
            "DO $$ BEGIN BEGIN UPDATE freight_workspaces SET data=data-'_freight_format' WHERE tenant_id='demo'; RAISE EXCEPTION 'Obsolete writer was accepted'; EXCEPTION WHEN check_violation THEN NULL; END; BEGIN UPDATE freight_workspaces SET data=freight_workspace_originals(tenant_id) WHERE tenant_id='demo'; RAISE EXCEPTION 'Current-marker aggregate writer was accepted'; EXCEPTION WHEN check_violation THEN NULL; END; END $$",
        )
        require(
            self.raw_workspace() == self.before,
            "Restored storage guard allowed an obsolete write",
        )
        self.step(
            "restored storage-format trigger rejects obsolete and current-marker aggregate writers without changing original data"
        )
        for name, health in (
            ("sync", "/healthz"),
            ("edi", "/health"),
            ("freight", "/health"),
        ):
            original_config = copy.deepcopy(self.configs[name])
            self.spawn(name, original_config)
            self.wait(
                lambda name=name, health=health: self.http(name, health),
                f"restored {name} startup",
                60,
            )
            require(
                digest(self.configs[name]) == self.report["configuration_sha256"][name],
                "Restore changed original service configuration or keys",
            )
        self.report["observed_freight_unavailable_seconds"] = round(
            time.monotonic() - self.unavailable_started, 3
        )
        self.step(
            "all actual services restart with original scoped credentials and encryption/signing configuration"
        )

    def recovered_work(self):
        p = self.pending

        def wait_queue(collection, identity, status, label):
            def observed():
                item = next(
                    (i for i in self.workspace()[collection] if i["id"] == identity),
                    None,
                )
                self.report.setdefault("recovered_queue_observations", {})[
                    collection
                ] = (
                    {
                        key: item.get(key)
                        for key in ("id", "status", "total_attempts", "last_error")
                    }
                    if item
                    else {"id": identity, "missing": True}
                )
                return item if item and item["status"] == status else None

            return self.wait(observed, label, 60)

        inbox = wait_queue(
            "inbox",
            p["inbox"]["id"],
            "applied",
            "restored original inbound work completes",
        )
        outbox = wait_queue(
            "outbox",
            p["outbound_retry"]["id"],
            "gateway_accepted",
            "restored original outbound handoff completes",
        )
        original_load = next(l for l in self.before["loads"] if l["id"] == p["load_id"])
        frozen_inbox = next(i for i in self.before["inbox"] if i["id"] == inbox["id"])
        frozen_outbox = next(
            j for j in self.before["outbox"] if j["id"] == outbox["id"]
        )
        accepted = self.load(p["load_id"])
        require(
            accepted["status"] == "accepted"
            and accepted["version"] == original_load["version"] + 1
            and accepted["sync_order_id"] == p["source"],
            "Restored 990 did not accept the exact original tender once",
        )
        require(
            inbox["original_body"] == p["inbox"]["original_body"]
            and inbox["authentication"] == p["inbox"]["authentication"]
            and inbox["delivery_ids"] == p["inbox"]["delivery_ids"]
            and inbox["total_attempts"] == frozen_inbox["total_attempts"] + 1
            and inbox["history"][: len(frozen_inbox["history"])]
            == frozen_inbox["history"],
            "Inbound recovery lost original authenticated bytes, delivery identity or attempt history",
        )
        require(
            outbox["payload"] == p["outbound_retry"]["payload"]
            and outbox["total_attempts"] == frozen_outbox["total_attempts"] + 1
            and "ST*204*" in outbox["response"],
            "Outbound recovery replaced the original intent or lost attempt history",
        )
        key_hash = hashlib.sha256(outbox["id"].encode()).hexdigest()
        receipts = self.rows(
            "edi_runtime",
            "SELECT r.document_id,d.raw_x12 FROM outbound_request_receipts r JOIN documents d ON d.id=r.document_id AND d.tenant_id=r.tenant_id WHERE r.tenant_id="
            + literal(EDI_TENANT)
            + " AND r.key_hash="
            + literal(key_hash)
            + " AND r.operation='x12:204'",
        )
        require(
            len(receipts) == 1 and receipts[0]["raw_x12"] == outbox["response"],
            "Restored outbound retry did not create exactly one real EDI receipt/document",
        )
        self.step(
            "restored original queues resume against actual EDI with retained histories and exactly one carrier decision",
            inbox_id=inbox["id"],
            outbox_id=outbox["id"],
            new_edi_document_id=receipts[0]["document_id"],
            accepted_load_version=accepted["version"],
        )
        for job in (p["original_outbox"], outbox):
            count = self.sql("edi_runtime", "SELECT count(*) FROM documents")
            controls = self.rows(
                "edi_runtime", "SELECT * FROM control_numbers ORDER BY key"
            )
            wire = self.http(
                "edi",
                "/v1/edi/outbound/204/brex",
                method="POST",
                body=job["payload"],
                headers={"Idempotency-Key": job["id"]},
            )
            require(
                wire == job["response"]
                and self.sql("edi_runtime", "SELECT count(*) FROM documents") == count
                and self.rows(
                    "edi_runtime", "SELECT * FROM control_numbers ORDER BY key"
                )
                == controls,
                "Post-restore EDI replay regenerated wire, documents or control numbers",
            )
        self.signed_callback(p["callback"])
        require(
            self.load(p["load_id"])["version"] == accepted["version"],
            "Restored callback replay repeated the carrier decision",
        )
        replay = self.http(
            "freight",
            "/v1/sync/import",
            method="POST",
            body=p["import_body"],
            headers={"Idempotency-Key": "recovery-original-import"},
        )
        require(
            replay["replayed"] is True and replay["data"] == p["import_response"],
            "Restored Freight receipt lost the original response",
        )
        self.step(
            "post-restore Freight receipts and original EDI wires replay without duplicate decisions, documents or control numbers"
        )
        after = self.raw_workspace()
        for key in FINANCIAL_ROOTS:
            require(
                after[key] == self.before[key],
                f"Recovery changed original financial {key}",
            )
        require(
            after["documents"] == self.before["documents"],
            "Recovery changed original document metadata/reviews",
        )
        require(
            after["events"][: len(self.before["events"])] == self.before["events"],
            "Recovery rewrote retained audit history",
        )
        for key, receipt in self.before["receipts"].items():
            require(
                after["receipts"].get(key) == receipt,
                "Recovery rewrote an original receipt",
            )
        for item in self.before["inbox"]:
            if item["id"] != inbox["id"]:
                require(
                    next(i for i in after["inbox"] if i["id"] == item["id"]) == item,
                    "Recovery changed a previously completed inbox record",
                )
        for job in self.before["outbox"]:
            if job["status"] in (
                "gateway_accepted",
                "synced",
                "held",
                "local_only",
                "native_only",
            ):
                expected = copy.deepcopy(job)
                if job["id"] == p["original_outbox"]["id"]:
                    require(
                        job["status"] == "gateway_accepted",
                        "Original pending carrier decision lost its accepted gateway handoff",
                    )
                    # The restored 990 advances its corresponding original 204.
                    # Every other field must retain the original handoff proof.
                    expected["status"] = "carrier_responded"
                actual = next(j for j in after["outbox"] if j["id"] == job["id"])
                if actual != expected:
                    self.report.setdefault("unexpected_outbox_changes", []).append(
                        {
                            "id": job["id"],
                            "expected_status": expected["status"],
                            "actual_status": actual["status"],
                            "changed_fields": sorted(
                                key
                                for key in set(actual) | set(expected)
                                if actual.get(key) != expected.get(key)
                            ),
                        }
                    )
                require(
                    actual == expected,
                    "Recovery changed a terminal or held original outbox record",
                )
        for key in (
            "stop_events",
            "stop_services",
            "movement_corrections",
            "carrier_submissions",
        ):
            require(after[key] == self.before[key], f"Recovery changed original {key}")
        for document in after["documents"]:
            content = self.http("freight", f"/v1/documents/{document['id']}/content")
            require(
                hashlib.sha256(content.encode()).hexdigest() == document["sha256"],
                "Restored original file download failed its metadata hash",
            )
        for grant in after["document_shares"]:
            if grant["revocation"] is not None:
                self.http(
                    "freight",
                    f"/v1/portal/files/{grant['id']}/content",
                    token=CUSTOMER_KEY,
                    expected=404,
                )
        self.step(
            "restored original financial cents, journals, documents, corrections, receipts and audit prefix remain exact",
            retained_financial_sha256=digest(
                {key: after[key] for key in FINANCIAL_ROOTS}
            ),
            document_count=len(after["documents"]),
            original_receipt_count=len(self.before["receipts"]),
        )
        other = self.http(
            "freight", "/v1/workspace", token="other-owned-runtime-freight-key-32-chars"
        )
        require(
            not other["loads"] and not other["documents"],
            "Restored Freight credential crossed tenants",
        )
        self.http(
            "sync",
            f"/v1/tenants/{OTHER_TENANT}/orders/{p['source']}",
            token="other-owned-runtime-sync-key",
            expected=404,
        )
        require(
            self.sync_order(p["source"])["id"] == p["source"]
            and self.sync_order(p["outbound_order"])["id"] == p["outbound_order"],
            "Canonical Sync reads lost original order identities after restore",
        )
        orders = self.sql(
            "sync_runtime",
            'SELECT to_jsonb(t)::text FROM orders t ORDER BY to_jsonb(t)::text COLLATE "C"',
        )
        require(
            hashlib.sha256(orders.encode()).hexdigest()
            == self.before_manifests["sync_runtime"]["tables"]['"public"."orders"'][
                "sha256"
            ],
            "Recovery changed canonical order identities, source cancellations or tracking without physical progress",
        )
        self.wait(
            lambda: any(
                j["status"] == "held" and "public" in (j["last_error"] or "")
                for j in self.rows(
                    "edi_runtime",
                    "SELECT status,last_error FROM callback_deliveries WHERE document_id="
                    + literal(receipts[0]["document_id"]),
                )
            ),
            "restored native callback public-address policy",
        )
        require(
            int(
                self.sql(
                    "edi_runtime", "SELECT coalesce(max(id),0) FROM callback_events"
                )
            )
            > self.before_event_sequence,
            "Restored sequence did not allocate a new callback event",
        )
        self.step(
            "restored tenant boundaries, callback policy and sequence allocation remain enforced"
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--build-dir", type=Path, default=Path("/tmp/freight-runtime-target")
    )
    parser.add_argument("--postgres-image", default="postgres:16")
    parser.add_argument(
        "--sync-repo", type=Path, default=ROOT.parent / "stateset-sync-server",
        help="Actual Sync source checkout or recorded source snapshot",
    )
    parser.add_argument(
        "--edi-repo", type=Path, default=ROOT.parent / "stateset-edi",
        help="Actual EDI source checkout or recorded source snapshot",
    )
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.output and args.output.exists():
        parser.error("--output must be a new report path")
    args.build_dir = args.build_dir.resolve()
    args.sync_repo = args.sync_repo.resolve()
    args.edi_repo = args.edi_repo.resolve()
    if not (args.sync_repo / "Cargo.toml").is_file() or not (args.edi_repo / "Cargo.toml").is_file():
        parser.error("--sync-repo and --edi-repo must contain actual Cargo source checkouts")
    os.umask(0o077)
    run = Recovery(args)
    run.report["source_repositories"] = {
        "sync": str(args.sync_repo), "edi": str(args.edi_repo),
    }

    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()

    signal.signal(signal.SIGTERM, interrupted)
    try:
        run.build()
        run.postgres()
        run.sync()
        run.edi()
        run.freight()
        run.checks()
        run.report["baseline_checks"] = len(run.report["checks"])
        run.record_recovery_cash()
        run.record_save_resolution()
        run.prepare_outage()
        run.backup_and_lose()
        run.restore()
        run.recovered_work()
        run.verify_save_resolution()
        for relative, expected in run.report["harness"].items():
            require(
                sha(ROOT / relative) == expected,
                "Recovery harness changed while qualifying",
            )
        for name, repo in {
            "freight": ROOT,
            "sync": args.sync_repo,
            "edi": args.edi_repo,
            "wire": ROOT / "runtime-contracts",
        }.items():
            require(
                fingerprint(repo) == run.report["builds"][name]["source"]
                and sha(run.binaries[name])
                == run.report["builds"][name]["binary_sha256"],
                f"{name} source/binary changed during recovery qualification",
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
        if args.output:
            with args.output.open("x") as stream:
                stream.write((run.directory / "report.json").read_text())
        print(f"REPORT {run.directory / 'report.json'}", flush=True)
    return 0 if run.report["result"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
