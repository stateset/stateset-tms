#!/usr/bin/env python3
"""Owned, loopback-only qualification against the actual sibling services.

No ambient service configuration is inherited by the runtime processes. PostgreSQL
runs in a labelled, disposable container using an already-cached image by default,
or an explicitly selected owned native PostgreSQL 16 cluster. The EDI
public-address callback policy is preserved; exact queued notifications are relayed
by this test using the sibling's actual signing implementation.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

from native_postgres import NativePostgres

ROOT = Path(__file__).resolve().parents[1]
ADMIN = "freight-runtime-admin-synthetic-only-at-least-32-characters"
CHECKER_KEY = "freight-runtime-checker-synthetic-only-at-least-32-characters"
CARRIER_KEY = "freight-runtime-carrier-synthetic-only-at-least-32-characters"
CUSTOMER_KEY = "freight-runtime-customer-synthetic-only-at-least-32-characters"
SYNC_KEY = "freight-runtime-sync-synthetic-only-at-least-32-characters"
EDI_ADMIN = "freight-runtime-edi-admin-synthetic-only-at-least-32-characters"
CALLBACK_SECRET = "freight-runtime-callback-synthetic-only-at-least-32-characters"
TENANT = "00000000-0000-4000-8000-000000000901"
OTHER_TENANT = "00000000-0000-4000-8000-000000000902"
EDI_TENANT = "freight-runtime"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def literal(value):
    return "'" + str(value).replace("'", "''") + "'"


def fingerprint(repo):
    files = [repo / "Cargo.toml", repo / "Cargo.lock", repo / "build.rs"]
    for directory in ("src", "migrations", "proto", "crates", "vendor"):
        base = repo / directory
        if base.exists():
            files += [
                p
                for p in base.rglob("*")
                if p.is_file()
                and not any(x in p.parts for x in ("target", "node_modules", ".git"))
            ]
    h = hashlib.sha256()
    selected = sorted(p for p in set(files) if p.exists())
    for p in selected:
        h.update(str(p.relative_to(repo)).encode() + b"\0" + p.read_bytes() + b"\0")
    return {
        "sha256": h.hexdigest(),
        "files": len(selected),
        "scope": "Cargo.toml/Cargo.lock/build.rs and src/migrations/proto/crates/vendor",
    }


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


class Runtime:
    def __init__(self, args):
        self.args = args
        self.directory = Path(tempfile.mkdtemp(prefix="stateset-freight-runtime-"))
        (self.directory / ".env").write_text(
            "# Empty owned configuration; no ancestor dotenv search.\n"
        )
        self.processes = {}
        self.container = None
        self.native_postgres = None
        self.container_label = str(uuid.uuid4())
        self.ports = {
            name: free_port() for name in ("postgres", "sync", "edi", "freight")
        }
        require(
            len(set(self.ports.values())) == 4, "Port allocation collided; run again"
        )
        self.urls = {
            name: f"http://127.0.0.1:{port}"
            for name, port in self.ports.items()
            if name != "postgres"
        }
        self.opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), NoRedirect()
        )
        self.secrets = [
            ADMIN,
            CHECKER_KEY,
            CARRIER_KEY,
            CUSTOMER_KEY,
            SYNC_KEY,
            EDI_ADMIN,
            CALLBACK_SECRET,
        ]
        self.edi_key = None
        self.report = {
            "scope": "isolated actual sibling runtime qualification",
            "result": "running",
            "started_at": dt.datetime.now(dt.timezone.utc).isoformat(),
            "artifacts": str(self.directory),
            "checks": [],
            "builds": {},
            "harness_sources": self.harness_sources(),
            "callback_transport": {
                "public_https_delivery": "not exercised",
                "local_test_relay": "actual queued bytes signed with stateset_edi::callbacks::signature",
                "native_public_address_policy": "retained",
            },
        }

    @staticmethod
    def harness_sources():
        return {
            name: hashlib.sha256((ROOT / "scripts" / name).read_bytes()).hexdigest()
            for name in ("runtime_contracts.py", "native_postgres.py")
        }

    def verify_inputs(self):
        require(self.report["harness_sources"] == self.harness_sources(),
                "Runtime harness changed during qualification")
        repos = {"sync": self.args.sync_repo, "edi": self.args.edi_repo,
                 "wire": ROOT / "runtime-contracts", "freight": ROOT}
        for name, repo in repos.items():
            require(self.report["builds"][name]["source"] == fingerprint(repo),
                    f"{name} sources changed during qualification")
            require(self.report["builds"][name]["binary_sha256"] == hashlib.sha256(
                        self.binaries[name].read_bytes()).hexdigest(),
                    f"{name} binary changed during qualification")
        self.report["inputs_unchanged_through_checks"] = True

    def safe(self, value):
        value = str(value)
        for secret in self.secrets:
            value = value.replace(secret, "[owned-test-secret]")
        return value

    def step(self, name, **evidence):
        self.report["checks"].append({"name": name, "result": "pass", **evidence})
        print(f"PASS {name}", flush=True)
        self.save()

    def save(self):
        (self.directory / "report.json").write_text(
            self.safe(json.dumps(self.report, indent=2)) + "\n"
        )

    def command(self, command, *, label, input=None, env=None, timeout=600):
        if command[0] == "docker":
            command = [
                shutil.which("docker") or "docker",
                "--host",
                "unix:///var/run/docker.sock",
                *command[1:],
            ]
            env = {"PATH": os.defpath, "LC_ALL": "C"}
        result = subprocess.run(
            command,
            input=input,
            text=True,
            cwd=self.directory,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )
        if result.returncode:
            raise RuntimeError(f"{label} failed: {self.safe(result.stderr[-3000:])}")
        return result.stdout.strip()

    def build(self):
        env = os.environ.copy()
        env.update(
            RUSTC_WRAPPER="",
            CARGO_INCREMENTAL="0",
            CARGO_BUILD_JOBS="2",
            CARGO_PROFILE_DEV_DEBUG="0",
            CARGO_TARGET_DIR=str(self.args.build_dir),
            SQLX_OFFLINE="true",
        )
        repos = {
            "sync": self.args.sync_repo,
            "edi": self.args.edi_repo,
            "wire": ROOT / "runtime-contracts",
            "freight": ROOT,
        }
        bins = {
            "sync": "stateset-sync-server",
            "edi": "stateset-edi",
            "wire": "stateset-freight-runtime-wire",
            "freight": "stateset-freight",
        }
        for name, repo in repos.items():
            print(f"BUILD {name} (logs retained)", flush=True)
            before = fingerprint(repo)
            log = self.directory / f"build-{name}.log"
            command = [
                "cargo",
                "build",
                "--offline",
                "--locked",
                "--manifest-path",
                str(repo / "Cargo.toml"),
                "--bin",
                bins[name],
            ]
            with log.open("w") as stream:
                result = subprocess.run(
                    command,
                    env=env,
                    cwd=self.directory,
                    stdout=stream,
                    stderr=subprocess.STDOUT,
                )
            require(result.returncode == 0, f"{name} build failed; inspect {log}")
            require(
                before == fingerprint(repo),
                f"{name} sources changed during build; rerun qualification",
            )
            binary = self.args.build_dir / "debug" / bins[name]
            self.report["builds"][name] = {
                "source": before,
                "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            }
        self.binaries = {
            name: self.args.build_dir / "debug" / binary
            for name, binary in bins.items()
        }
        self.save()

    def http(
        self,
        service,
        path,
        *,
        method="GET",
        body=None,
        token=None,
        headers=None,
        expected=200,
        return_status=False,
    ):
        url = self.urls[service] + path
        require(
            urllib.parse.urlparse(url).hostname == "127.0.0.1",
            "Only owned loopback services are allowed",
        )
        values = dict(headers or {})
        if service == "freight" and not path.startswith("/webhooks/"):
            values.setdefault("Authorization", "Bearer " + (token or ADMIN))
        elif service == "sync":
            values.setdefault("x-stateset-api-key", token or SYNC_KEY)
        elif service == "edi":
            values.setdefault(
                "Authorization", "Bearer " + (token or self.edi_key or EDI_ADMIN)
            )
        if body is not None and not isinstance(body, (str, bytes)):
            body = json.dumps(body, separators=(",", ":")).encode()
            values.setdefault("Content-Type", "application/json")
        elif isinstance(body, str):
            body = body.encode()
        if method not in ("GET", "HEAD"):
            values.setdefault("Idempotency-Key", str(uuid.uuid4()))
        request = urllib.request.Request(url, data=body, headers=values, method=method)
        try:
            response = self.opener.open(request, timeout=8)
        except urllib.error.HTTPError as e:
            response = e
        raw = response.read()
        try:
            result = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError):
            result = raw.decode(errors="replace")
        choices = expected if isinstance(expected, tuple) else (expected,)
        require(
            response.status in choices,
            f"{service} {method} {path}: {response.status}: {self.safe(str(result)[:1000])}",
        )
        return (response.status, result) if return_status else result

    def wait(self, predicate, label, seconds=45):
        deadline = time.monotonic() + seconds
        last = None
        while time.monotonic() < deadline:
            for name, process in self.processes.items():
                require(
                    process.poll() is None,
                    f"{name} stopped; inspect {self.directory / (name + '.log')}",
                )
            try:
                value = predicate()
                if value:
                    return value
            except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
                last = type(e).__name__
            time.sleep(0.2)
        raise TimeoutError(f"{label} did not complete; last observation: {last}")

    def sql(self, database, statement):
        url = (
            f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/{database}"
        )
        return self.command(
            ["psql", url, "-X", "-At", "-v", "ON_ERROR_STOP=1", "-c", statement],
            label=f"owned {database} SQL",
            env={"PATH": os.defpath, "LC_ALL": "C"},
        )

    def rows(self, database, statement):
        return json.loads(
            self.sql(
                database,
                "SELECT coalesce(json_agg(row_to_json(q)),'[]'::json)::text FROM ("
                + statement
                + ") q",
            )
        )

    def postgres(self):
        if getattr(self.args, "postgres_bin", None) is not None:
            self.native_postgres = NativePostgres(self.directory, self.args.postgres_bin, self.ports['postgres'])
            self.native_postgres.start()
            for _ in range(100):
                require(self.native_postgres.process.poll() is None, "Native PostgreSQL exited; inspect retained log")
                try:
                    user = self.sql("postgres", "SELECT current_user")
                except RuntimeError:
                    time.sleep(0.2)
                    continue
                require(user == "freight_runtime", "Unexpected native PostgreSQL role")
                self.native_postgres.assert_identity(self.sql("postgres", "SHOW data_directory"))
                require(self.sql("postgres", "SHOW server_version_num").startswith("16"),
                        "Connected native PostgreSQL is not version 16")
                break
            else:
                raise TimeoutError("Owned native PostgreSQL did not become ready")
            self.report['postgres'] = {**self.native_postgres.metadata(), 'version': self.sql('postgres', 'SHOW server_version')}
            for database in ('sync_runtime', 'edi_runtime', 'freight_runtime'):
                self.sql('postgres', 'CREATE DATABASE ' + database)
            self.step('isolated native PostgreSQL 16 ready; container and capacity acceptance remain separate')
            return
        image = self.command(
            [
                "docker",
                "image",
                "inspect",
                self.args.postgres_image,
                "--format",
                "{{.Id}}",
            ],
            label="cached PostgreSQL image",
        )
        self.container = self.command(
            [
                "docker",
                "run",
                "--rm",
                "--pull=never",
                "--name",
                "freight-runtime-" + self.container_label,
                "--label",
                "com.stateset.freight.runtime=" + self.container_label,
                "--cpus=2",
                "--memory=768m",
                "--tmpfs",
                "/var/lib/postgresql/data:rw,size=512m",
                "-e",
                "POSTGRES_USER=freight_runtime",
                "-e",
                "POSTGRES_HOST_AUTH_METHOD=trust",
                "-e",
                "POSTGRES_DB=postgres",
                "-e",
                "POSTGRES_INITDB_ARGS=--encoding=UTF8 --locale=C",
                "-p",
                f"127.0.0.1:{self.ports['postgres']}:5432",
                "-d",
                self.args.postgres_image,
            ],
            label="owned PostgreSQL container",
        )
        for _ in range(100):
            try:
                if self.sql("postgres", "SELECT current_user") == "freight_runtime":
                    break
            except RuntimeError:
                time.sleep(0.2)
        else:
            raise TimeoutError("Owned PostgreSQL did not become ready")
        self.report["postgres"] = {
            "backend": "docker",
            "image_id": image,
            "version": self.sql("postgres", "SHOW server_version"),
        }
        for database in ("sync_runtime", "edi_runtime", "freight_runtime"):
            self.sql("postgres", "CREATE DATABASE " + database)
        self.step("isolated PostgreSQL ready")

    def spawn(self, name, config):
        env = {"PATH": os.defpath, "RUST_LOG": "warn", **config}
        stream = (self.directory / f"{name}.log").open("a")
        process = subprocess.Popen(
            [str(self.binaries[name])],
            cwd=self.directory,
            env=env,
            stdout=stream,
            stderr=subprocess.STDOUT,
        )
        stream.close()
        self.processes[name] = process

    def stop(self, name):
        process = self.processes.pop(name)
        process.terminate()
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)

    def sync(self):
        cfg = {
            "server": {
                "host": "127.0.0.1",
                "port": self.ports["sync"],
                "run_migrations_on_startup": True,
                "database_url": f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/sync_runtime",
            },
            "tenants": [
                {
                    "id": t,
                    "display_name": "Owned runtime tenant",
                    "auth": {
                        "api_key": (
                            SYNC_KEY if t == TENANT else "other-owned-runtime-sync-key"
                        ),
                        "enabled": True,
                    },
                }
                for t in (TENANT, OTHER_TENANT)
            ],
        }
        self.spawn(
            "sync",
            {
                "STATESET_APP_CONFIG_JSON": json.dumps(cfg),
                "STATESET_ENV": "test",
                "STATESET_MIGRATIONS_PATH": str(self.args.sync_repo / "migrations"),
            },
        )
        self.wait(lambda: self.http("sync", "/healthz"), "actual Sync startup", 60)
        self.step("actual Sync server and migrations ready")

    def seed_order(self, reference, tenant=TENANT):
        identity = str(uuid.uuid4())
        items = [
            {
                "title": "Owned synthetic goods",
                "quantity": 2,
                "price": 19.99,
                "sku": "RUNTIME-SKU",
            }
        ]
        self.sql(
            "sync_runtime",
            "INSERT INTO orders(id,tenant_id,shopify_order_id,order_number,status,line_items,created_at,updated_at) VALUES("
            + ",".join(
                [
                    literal(identity) + "::uuid",
                    literal(tenant),
                    literal(identity),
                    literal(reference),
                    "'PENDING'",
                    literal(json.dumps(items)) + "::jsonb",
                    "now()",
                    "now()",
                ]
            )
            + ")",
        )
        return identity

    def sync_order(self, identity):
        return self.http("sync", f"/v1/tenants/{TENANT}/orders/{identity}")["data"]

    def edi(self):
        self.spawn(
            "edi",
            {
                "EDI_BIND_ADDR": f"127.0.0.1:{self.ports['edi']}",
                "EDI_API_TOKEN": EDI_ADMIN,
                "EDI_DATA_DIR": str(self.directory / "edi-data"),
                "EDI_MIGRATE_ON_START": "true",
                "DATABASE_URL": f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/edi_runtime",
                "EDI_CONNECTION_ENCRYPTION_KEY": "11" * 32,
                "EDI_SIGNING_KEY": "22" * 32,
                "STATESET_SEQUENCER_URL": self.urls["freight"]
                + "/unused-test-sequencer",
            },
        )
        self.wait(lambda: self.http("edi", "/health"), "actual EDI startup", 60)
        self.http(
            "edi",
            "/v1/tenants",
            method="POST",
            token=EDI_ADMIN,
            body={
                "id": EDI_TENANT,
                "display_name": "Owned freight runtime",
                "enabled": True,
            },
        )
        key = self.http(
            "edi",
            f"/v1/tenants/{EDI_TENANT}/api-keys",
            method="POST",
            token=EDI_ADMIN,
            body={"name": "owned-freight-qualification"},
        )["data"]["api_key"]
        self.edi_key = key
        self.secrets.append(key)
        partner = {
            "id": "brex",
            "name": "Synthetic runtime carrier",
            "partner_isa_qualifier": "ZZ",
            "partner_isa_id": "CARRIER",
            "our_isa_qualifier": "ZZ",
            "our_isa_id": "STATESET",
            "version": "004010",
            "usage": "T",
        }
        self.http("edi", "/v1/partners/brex", method="PUT", body=partner)
        self.http(
            "edi",
            "/v1/callbacks/freight-runtime",
            method="PUT",
            token=EDI_ADMIN,
            headers={"x-tenant-id": EDI_TENANT},
            body={
                "expected_version": 0,
                "url": f"https://127.0.0.1:{self.ports['freight']}/webhooks/edi/demo",
                "secret": CALLBACK_SECRET,
                "partner_id": "brex",
                "enabled": True,
            },
        )
        self.step("actual EDI tenant, key, test partner and callback registered")

    def freight(self):
        self.freight_env = {
            "FREIGHT_DEMO": "true",
            "FREIGHT_BIND_ADDR": f"127.0.0.1:{self.ports['freight']}",
            "DATABASE_URL": f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime",
            "FREIGHT_API_KEYS_JSON": json.dumps(
                {
                    ADMIN: {"tenant_id": "demo", "subject": "runtime:entry-reviewer", "role": "admin"},
                    CHECKER_KEY: {"tenant_id": "demo", "subject": "runtime:independent-checker", "role": "finance"},
                    CARRIER_KEY: {
                        "tenant_id": "demo",
                        "subject": "runtime:carrier",
                        "role": "carrier",
                        "counterparty_id": "ca4",
                    },
                    CUSTOMER_KEY: {
                        "tenant_id": "demo",
                        "subject": "runtime:customer",
                        "role": "customer",
                        "counterparty_id": "c1",
                    },
                    "other-owned-runtime-freight-key-32-chars": "other-workspace",
                }
            ),
            "FREIGHT_INTEGRATIONS_JSON": json.dumps(
                {
                    "demo": {
                        "sync_url": self.urls["sync"],
                        "sync_api_key": SYNC_KEY,
                        "sync_tenant_id": TENANT,
                        "edi_url": self.urls["edi"],
                        "edi_token": self.edi_key,
                        "edi_tenant_id": EDI_TENANT,
                        "callback_keys": {"1": CALLBACK_SECRET},
                    }
                }
            ),
        }
        self.spawn("freight", self.freight_env)
        self.wait(lambda: self.http("freight", "/health"), "actual Freight startup")
        # Bootstrap only the isolated synthetic source, then run all business
        # checks with the production invoice policy. This also verifies that
        # naming an existing tenant demo does not bypass independent review.
        self.stop("freight")
        self.freight_env["FREIGHT_DEMO"] = "false"
        self.spawn("freight", self.freight_env)
        self.wait(lambda: self.http("freight", "/health"), "non-demo Freight startup")
        require(self.workspace()["mode"] == "live", "Runtime did not leave demo mode")
        self.step("actual Freight API and workers ready")

    def workspace(self):
        return self.http("freight", "/v1/workspace")

    def load(self, identity):
        return next(l for l in self.workspace()["loads"] if l["id"] == identity)

    def mutate(self, path, body, key=None, expected=200, token=None):
        result = self.http(
            "freight",
            "/v1/" + path,
            method="POST",
            body=body,
            expected=expected,
            token=token,
            headers={"Idempotency-Key": key or str(uuid.uuid4())},
        )
        return result if expected != 200 else result["data"]

    def rate_confirmation(self, load_id, body):
        """Retry only a refused source-observation race, never an uncertain save.

        The outbox worker also observes Sync. Its newer observation can reject
        the API's pre-commit source refresh without changing the load. Keep the
        original command/version/key and prove no agreement appeared before a
        bounded retry. A changed business version or any other refusal fails.
        """
        path = f"/v1/loads/{load_id}/rate-confirmations"
        key = str(uuid.uuid4())
        original = json.dumps(body, separators=(",", ":")).encode()

        def agreements(workspace):
            return [d for d in workspace["rate_confirmations"] if d["load_id"] == load_id]

        before = agreements(self.workspace())
        for attempt in range(1, 4):
            status, result = self.http(
                "freight", path, method="POST", body=original,
                headers={"Idempotency-Key": key, "Content-Type": "application/json"},
                expected=(200, 409), return_status=True,
            )
            if status == 200:
                return result["data"]
            require(result == {"error": {"message": "Sync source check changed; retry refresh"}},
                    f"Rate confirmation refused: {self.safe(result)}")
            workspace = self.workspace()
            load = next(l for l in workspace["loads"] if l["id"] == load_id)
            require(load["version"] == body["expected_load_version"],
                    "Load changed after source refusal; original rate command must not be rebased")
            require(agreements(workspace) == before,
                    "Rate confirmations changed after source refusal; refusing retry")
            self.report.setdefault("source_observation_retries", []).append({
                "path": path, "attempt": attempt, "status": status,
                "idempotency_key": key,
                "body_sha256": hashlib.sha256(original).hexdigest(),
                "original_load_version": body["expected_load_version"],
                "load_version_and_agreements_unchanged": True,
            })
            self.save()
            require(attempt < 3, "Source observation conflicts exhausted three rate attempts")
            time.sleep(0.2)

    def invoice_entry_review(self, invoice_id):
        invoice = next(i for i in self.workspace()["invoices"] if i["id"] == invoice_id)
        control = invoice["approval_control"]
        require(control["required"], "Independent invoice policy is disabled")
        self.mutate(f"invoices/{invoice_id}/entry-review", {
            "expected_revision": control["revision"],
            "evidence_reference": "Owned actual-codec carrier 210 source RUNTIME-210",
            "note": "Checked exact carrier invoice, current assignment, amount and agreement against owned source",
        })
        return {"expected_revision": control["revision"], "note": "Independent checker inspected the actual owned invoice, agreement and delivery evidence"}

    def plan(self, reference):
        date = (
            (dt.datetime.now(dt.timezone.utc) + dt.timedelta(days=1)).date().isoformat()
        )
        return {
            "reference": reference,
            "customer_id": "c1",
            "mode": "FTL",
            "equipment": "Dry van",
            "commodity": "Owned runtime freight",
            "weight_lbs": 1000,
            "miles": 173,
            "buy_cents": 125001,
            "sell_cents": 150001,
            "origin": {
                "facility": "Runtime pickup",
                "city": "Portland",
                "state": "OR",
                "date": date,
                "address_lines": ["15 Dock Street"],
                "postal_code": "97201",
                "country": "US",
            },
            "destination": {
                "facility": "Runtime delivery",
                "city": "Seattle",
                "state": "WA",
                "date": date,
                "address_lines": ["25 Receiving Ave"],
                "postal_code": "98101",
                "country": "US",
            },
        }

    def import_order(self, identity, reference):
        return self.mutate(
            "sync/import", {"order_id": identity, "load": self.plan(reference)}
        )

    def command_load(self, identity, action, **fields):
        return self.mutate(
            f"loads/{identity}/commands",
            {
                "action": action,
                "expected_version": self.load(identity)["version"],
                **fields,
            },
        )

    def raw(self, transaction, group, segments, control):
        now = dt.datetime.now(dt.timezone.utc)
        count = segments.count("~") + 2
        return f"ISA*00*          *00*          *ZZ*CARRIER        *ZZ*STATESET       *{now:%y%m%d}*{now:%H%M}*U*00401*{control:09}*0*T*>~GS*{group}*CARRIER*STATESET*{now:%Y%m%d}*{now:%H%M}*{control}*X*004010~ST*{transaction}*0001~{segments}SE*{count}*0001~GE*1*{control}~IEA*1*{control:09}~"

    def relay(self, transaction):
        jobs = self.rows(
            "edi_runtime",
            "SELECT id,document_id,version,payload,status,last_error FROM callback_deliveries WHERE tenant_id="
            + literal(EDI_TENANT)
            + " AND (payload::jsonb->>'direction')='inbound' AND (payload::jsonb->>'transaction_set')="
            + literal(transaction)
            + " ORDER BY created_at DESC",
        )
        require(jobs, "Actual EDI document did not enqueue its callback")
        job = jobs[0]
        timestamp = int(time.time())
        signature = self.command(
            [str(self.binaries["wire"])],
            label="actual sibling callback signer",
            input=json.dumps(
                {
                    "secret": CALLBACK_SECRET,
                    "timestamp": timestamp,
                    "delivery_id": job["id"],
                    "payload": job["payload"],
                }
            ),
        )
        headers = {
            "Content-Type": "application/json",
            "x-stateset-event-id": job["id"],
            "x-stateset-timestamp": str(timestamp),
            "x-stateset-key-version": str(job["version"]),
            "x-stateset-signature": "v1=" + signature,
        }
        response = self.http(
            "freight",
            "/webhooks/edi/demo",
            method="POST",
            body=job["payload"],
            headers=headers,
            expected=(200, 202),
        )

        def applied():
            item = next(
                (
                    i
                    for i in self.workspace()["inbox"]
                    if i["document_id"] == job["document_id"]
                ),
                None,
            )
            if item and item["status"] in ("retry", "held", "reconciliation_required"):
                self.report["failed_inbox"] = item
                self.report["failed_canonical_document"] = self.http(
                    "edi", f"/v1/documents/{job['document_id']}"
                )
                raise RuntimeError(
                    f"Actual {transaction} inbox {item['status']}: {item.get('last_error')}"
                )
            return (
                item
                if item and item["status"] in ("applied", "ignored", "finance_review")
                else None
            )

        self.wait(applied, "real EDI document applied by Freight")
        return job, headers, response

    def checks(self):
        probe = self.seed_order("LEGACY-TRACKING-ROUNDTRIP")
        self.http(
            "sync",
            f"/v1/tenants/{TENANT}/orders/{probe}/tracking",
            method="POST",
            body={
                "trackingData": {
                    "tracking_number": "LEGACY-PROBE",
                    "carrier": "Synthetic carrier",
                    "estimated_delivery": "2026-10-20",
                }
            },
        )
        tracking = self.sync_order(probe).get("tracking") or {}
        require(
            tracking.get("trackingNumber") is None,
            "Legacy compatibility probe changed; inspect actual Sync model",
        )
        self.step(
            "actual Sync canonical read demonstrates legacy snake-case field loss",
            canonical_tracking=tracking,
        )
        self.http("sync", f"/v1/tenants/{OTHER_TENANT}/orders", expected=(401, 403))
        identity = self.seed_order("FREIGHT-RUNTIME-PRIMARY")
        other = self.seed_order("OTHER-TENANT-ORDER", OTHER_TENANT)
        candidates = self.http("freight", "/v1/sync/orders?page=1")
        require(
            identity in json.dumps(candidates) and other not in json.dumps(candidates),
            "Real Sync candidates crossed tenants",
        )
        self.http("sync", f"/v1/tenants/{TENANT}/orders/{other}", expected=404)
        original = self.sync_order(identity)
        load = self.import_order(identity, "RUNTIME-FREIGHT-PRIMARY")
        lid = load["id"]
        require(
            load["sync_order_id"] == identity and load["source_order"] == original,
            "Canonical import changed order identity/evidence",
        )
        now = dt.datetime.now(dt.timezone.utc)
        quote = self.mutate(
            "quotes",
            {
                "reference": "RUNTIME-CUSTOMER-QUOTE",
                "planned_load_id": lid,
                "expected_load_version": load["version"],
                "plan": {
                    k: load[k]
                    for k in (
                        "customer_id",
                        "mode",
                        "equipment",
                        "commodity",
                        "weight_lbs",
                        "miles",
                        "origin",
                        "destination",
                    )
                },
                "sell_cents": 150001,
                "valid_until": (now + dt.timedelta(hours=12)).isoformat(),
                "terms": "Owned actual customer transportation agreement",
            },
        )
        agreement = self.mutate(
            "carrier-contracts",
            {
                "number": "RUNTIME-CARRIER-LANE",
                "carrier_id": "ca4",
                "rate": {
                    "customer_id": load["customer_id"],
                    "origin": {"country": "US", "state": "OR", "postal_code": "97201"},
                    "destination": {
                        "country": "US",
                        "state": "WA",
                        "postal_code": "98101",
                    },
                    "equipment": "Dry van",
                    "max_weight_lbs": 45000,
                    "max_miles": 200,
                    "effective_from": now.date().isoformat(),
                    "effective_through": (now + dt.timedelta(days=30))
                    .date()
                    .isoformat(),
                    "amount_cents": 125001,
                    "terms": "Owned inclusive carrier agreement; fresh capacity confirmation required",
                },
                "contact": "Owned carrier contracting lead",
                "reference": "PRIVATE-RUNTIME-CONTRACT-SOURCE",
                "note": "Original synthetic external agreement inspected",
                "reason": "Initial carrier agreement",
            },
        )
        rates = self.http("freight", f"/v1/quotes/{quote['id']}/contract-rates")
        require(
            rates["rates"][0]["eligible"],
            "Actual imported lane did not match carrier agreement",
        )
        quote = self.mutate(
            f"quotes/{quote['id']}/contract-offers",
            {
                "expected_version": quote["version"],
                "agreement_id": agreement["id"],
                "expected_rate_version": agreement["version"],
                "revision_sha256": agreement["revisions"][0]["basis_sha256"],
                "valid_until": (now + dt.timedelta(hours=24)).isoformat(),
                "contact": "Owned carrier lead",
                "reference": "Actual synthetic carrier rate offer",
                "note": "Private carrier procurement evidence",
            },
        )
        require(
            quote["revisions"][0]["offers"][0]["amount_cents"] == 125001
            and quote["revisions"][0]["offers"][0]["rate_source"]["agreement_id"]
            == agreement["id"],
            "Contract sourcing lost exact agreed price or original revision",
        )
        self.step(
            "carrier contract sourcing binds exact rate and fresh capacity on canonical Sync shipment",
            agreement_id=agreement["id"],
            revision=1,
            amount_cents=125001,
            basis_sha256=agreement["revisions"][0]["basis_sha256"],
        )
        quote = self.mutate(
            f"quotes/{quote['id']}/issue",
            {
                "expected_version": quote["version"],
                "offer_id": quote["revisions"][0]["offers"][0]["id"],
            },
        )
        publication = self.mutate(
            f"quotes/{quote['id']}/portal-publications",
            {"expected_version": quote["version"]},
        )
        quote = next(q for q in self.workspace()["quotes"] if q["id"] == quote["id"])
        decision_body = {
            "expected_version": quote["version"],
            "revision": publication["revision"],
            "document_sha256": publication["sha256"],
            "decision": "accept",
            "contact": "Owned customer transportation lead",
            "reference": "Actual synthetic native acceptance",
            "note": "Customer reviewed exact published transportation plan and terms",
        }
        decision = self.http(
            "freight",
            f"/v1/portal/customer-quotes/{publication['id']}/response",
            method="POST",
            token=CUSTOMER_KEY,
            body=decision_body,
        )["data"]
        quote = next(q for q in self.workspace()["quotes"] if q["id"] == quote["id"])
        require(
            quote["status"] == "accepted"
            and quote["booked_load_id"] is None
            and decision["recorded_by"] == "runtime:customer",
            "Native customer acceptance invented booking or lost authenticated actor",
        )
        quote = self.mutate(
            f"quotes/{quote['id']}/book",
            {
                "expected_version": quote["version"],
                "expected_load_version": self.load(lid)["version"],
            },
        )
        require(
            quote["booked_load_id"] == lid
            and self.load(lid)["sync_order_id"] == identity,
            "Native quote booking duplicated original Sync shipment",
        )
        document = self.http(
            "freight",
            f"/v1/portal/customer-quotes/{publication['id']}/statement",
            token=CUSTOMER_KEY,
        )
        require(
            hashlib.sha256(document.encode()).hexdigest() == publication["sha256"],
            "Customer published document changed after booking",
        )
        self.step(
            "native customer acceptance books exact selected rates on original Sync shipment",
            publication_id=publication["id"],
            document_sha256=publication["sha256"],
            recorded_by=decision["recorded_by"],
            load_id=lid,
            sync_order_id=identity,
        )
        self.command_load(lid, "tender", carrier_id="ca4")
        job = self.wait(
            lambda: next(
                (
                    j
                    for j in self.workspace()["outbox"]
                    if j["load_id"] == lid
                    and j["destination"] == "stateset-edi"
                    and j["status"] == "gateway_accepted"
                ),
                None,
            ),
            "actual 204 generated",
        )
        require(
            "ST*204*" in job["response"]
            and self.load(lid)["reference"] in job["response"],
            "Actual gateway returned no valid tender wire",
        )
        replayed_wire = self.http(
            "edi",
            "/v1/edi/outbound/204/brex",
            method="POST",
            body=job["payload"],
            headers={"Idempotency-Key": job["id"]},
        )
        require(
            replayed_wire == job["response"],
            "Actual gateway did not replay original 204 bytes for the stable outbox key",
        )
        self.step(
            "actual EDI stable idempotency key replays identical 204 bytes",
            wire_sha256=hashlib.sha256(job["response"].encode()).hexdigest(),
        )

        shipment = next(
            t["shipment_reference"]
            for t in self.workspace()["tenders"]
            if t["id"] == self.load(lid)["tender_id"]
        )
        self.step(
            "canonical Sync import and actual EDI 204 generation retain order/tender identity",
            sync_order_id=identity,
            freight_load_id=lid,
            tender_shipment_reference=shipment,
        )
        today = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%d")
        self.http(
            "edi",
            "/v1/edi/inbound/brex",
            method="POST",
            body=self.raw("990", "GF", f"B1*BREX*{shipment}*{today}*A~", 1),
            headers={"Content-Type": "application/edi-x12"},
        )
        delivery, headers, _ = self.relay("990")
        require(
            self.load(lid)["status"] == "accepted",
            "Real 990 did not accept exact tender",
        )
        repeat = self.http(
            "freight",
            "/webhooks/edi/demo",
            method="POST",
            body=delivery["payload"],
            headers=headers,
            expected=(200, 202),
        )
        require(
            len(
                [
                    i
                    for i in self.workspace()["inbox"]
                    if i["document_id"] == delivery["document_id"]
                ]
            )
            == 1,
            "Callback replay duplicated inbox",
        )
        self.http(
            "freight",
            "/webhooks/edi/demo",
            method="POST",
            body=delivery["payload"] + " ",
            headers=headers,
            expected=401,
        )
        self.step(
            "actual queued 990 and sibling signature apply once through Freight callback/inbox",
            document_id=delivery["document_id"],
            delivery_id=delivery["id"],
            original_payload_sha256=hashlib.sha256(
                delivery["payload"].encode()
            ).hexdigest(),
        )
        rate = self.rate_confirmation(
            lid,
            {
                "expected_load_version": self.load(lid)["version"],
                "terms": "Owned synthetic carrier transportation agreement",
                "reason": "Staff qualification of native carrier execution",
            },
        )
        offer = self.mutate(
            f"loads/{lid}/portal-offers",
            {
                "expected_load_version": self.load(lid)["version"],
                "rate_confirmation_id": rate["id"],
            },
        )
        native = self.http(
            "freight",
            f"/v1/portal/offers/{offer['id']}/tracking",
            method="POST",
            token=CARRIER_KEY,
            body={
                "expected_load_version": self.load(lid)["version"],
                "kind": "pickup",
                "observed_at": dt.datetime.now(dt.timezone.utc).isoformat(),
                "location": "Portland, OR",
                "reference": "Actual synthetic driver pickup evidence",
                "note": "Owned carrier dispatcher recorded actual test pickup",
            },
        )["data"]
        require(
            self.load(lid)["status"] == "in_transit"
            and native["recorded_by"] == "runtime:carrier"
            and native["tender_id"] == self.load(lid)["tender_id"],
            "Native carrier pickup lost current tender or authenticated identity",
        )
        self.step(
            "native carrier pickup binds real EDI tender and canonical Sync order",
            submission_id=native["id"],
            recorded_by=native["recorded_by"],
            tender_id=native["tender_id"],
        )
        self.http(
            "edi",
            "/v1/edi/inbound/brex",
            method="POST",
            body=self.raw(
                "214",
                "QM",
                f"B10*{shipment}*RUNTIME-FREIGHT-PRIMARY*BREX~LX*1~MS1*Portland*OR*US~AT7*AF*NS***{today}*{dt.datetime.now(dt.timezone.utc):%H%M}*UT~",
                2,
            ),
            headers={"Content-Type": "application/edi-x12"},
        )
        self.relay("214")
        self.wait(
            lambda: self.sync_order(identity).get("status", "").lower() == "shipped",
            "actual Sync physical writeback",
        )
        source = self.sync_order(identity)
        tracking = source.get("tracking") or {}
        require(
            tracking.get("trackingNumber") == load["reference"],
            f"Canonical Sync tracking number was lost: {tracking}",
        )
        require(
            tracking.get("estimatedDelivery") == load["destination"]["date"],
            "Canonical Sync delivery estimate was lost",
        )
        require(
            tracking.get("carrier") == "Blue Ridge Express",
            "Canonical Sync carrier name was lost",
        )

        self.step(
            "actual tracking write/read round-trip retains number, carrier and delivery estimate",
            canonical_tracking=tracking,
        )
        self.http(
            "edi",
            "/v1/edi/inbound/brex",
            method="POST",
            body=self.raw(
                "214",
                "QM",
                f"B10*{shipment}*RUNTIME-FREIGHT-PRIMARY*BREX~LX*1~MS1*Seattle*WA*US~AT7*D1*NS***{today}*{dt.datetime.now(dt.timezone.utc):%H%M}*UT~",
                3,
            ),
            headers={"Content-Type": "application/edi-x12"},
        )
        self.relay("214")
        require(
            self.load(lid)["status"] == "delivered" and not self.load(lid)["pod_ready"],
            "Real D1 must deliver without fabricating POD approval",
        )
        require(
            self.load(lid)["location"] == "Seattle, WA", "Actual 214 location was lost"
        )
        self.http(
            "edi",
            "/v1/edi/inbound/brex",
            method="POST",
            body=self.raw(
                "210",
                "IM",
                f"B3**RUNTIME-210*{shipment}*BREX*PP*{today}*125001~C3*USD~",
                4,
            ),
            headers={"Content-Type": "application/edi-x12"},
        )
        self.relay("210")
        invoice = next(i for i in self.workspace()["invoices"] if i["load_id"] == lid)
        require(
            invoice["amount_cents"] == 125001 and invoice["status"] == "matched",
            "Actual parsed 210 lost the exact carrier agreement",
        )
        self.mutate(f"invoices/{invoice['id']}/approve", self.invoice_entry_review(invoice['id']), expected=422, token=CHECKER_KEY)
        pdf = b"%PDF-1.7\nOwned synthetic receiver delivery evidence\n%%EOF"
        query = urllib.parse.urlencode(
            {
                "expected_load_version": self.load(lid)["version"],
                "filename": "runtime-pod.pdf",
                "reference": "Owned synthetic receiver POD supplied by carrier",
                "note": "Carrier lead supplied actual test receiver file",
            }
        )
        pod_submission = self.http(
            "freight",
            f"/v1/portal/offers/{offer['id']}/pod?{query}",
            method="POST",
            token=CARRIER_KEY,
            body=pdf,
            headers={"Content-Type": "application/pdf"},
        )["data"]
        document = next(
            d
            for d in self.workspace()["documents"]
            if d["id"] == pod_submission["document_id"]
        )
        require(
            document["uploaded_by"] == "runtime:carrier" and not document["reviews"],
            "Carrier POD must retain original actor and remain unreviewed",
        )
        download = self.http(
            "freight",
            f"/v1/portal/submissions/{pod_submission['id']}/content",
            token=CARRIER_KEY,
        )
        require(
            download == pdf.decode(),
            "Carrier scoped download changed original file bytes",
        )
        self.mutate(f"invoices/{invoice['id']}/approve", self.invoice_entry_review(invoice['id']), expected=422, token=CHECKER_KEY)
        self.mutate(
            f"documents/{document['id']}/review",
            {
                "expected_version": document["version"],
                "decision": "accepted",
                "note": "Finance inspected actual owned synthetic receiver evidence",
            },
        )
        reviewed = next(
            d for d in self.workspace()["documents"] if d["id"] == document["id"]
        )
        shared = self.mutate(
            f"documents/{document['id']}/shares",
            {
                "expected_load_version": self.load(lid)["version"],
                "expected_document_version": reviewed["version"],
                "audience": "customer",
                "message": "Original receiver POD for your shipment",
                "review_note": "Private staff inspection of original bytes and recipient",
                "acknowledge_content": True,
            },
        )
        shared_download = self.http(
            "freight", f"/v1/portal/files/{shared['id']}/content", token=CUSTOMER_KEY
        )
        require(
            shared_download == pdf.decode(),
            "Shared customer POD download changed exact bytes",
        )
        before_revoke = next(
            s for s in self.workspace()["document_shares"] if s["id"] == shared["id"]
        )
        require(
            len(before_revoke["download_authorizations"]) == 1
            and before_revoke["download_authorizations"][0]["authorized_by"]
            == "runtime:customer",
            "Shared download authorization lost original actor",
        )
        self.mutate(
            f"document-shares/{shared['id']}/revoke",
            {
                "expected_version": 1,
                "note": "Owned test revocation of future partner access",
            },
        )
        self.http(
            "freight",
            f"/v1/portal/files/{shared['id']}/content",
            token=CUSTOMER_KEY,
            expected=404,
        )
        require(
            self.load(lid)["pod_ready"],
            "Revoking a file share invalidated reviewed POD finance evidence",
        )
        self.step(
            "scoped customer POD share verifies bytes and revokes access without changing finance evidence",
            share_id=shared["id"],
            sha256=reviewed["sha256"],
            authorized_by="runtime:customer",
            authorization_count=1,
        )
        approval = self.invoice_entry_review(invoice['id'])
        self.mutate(f"invoices/{invoice['id']}/approve", approval, expected=403)
        self.mutate(f"invoices/{invoice['id']}/approve", approval, token=CHECKER_KEY)
        retained = next(e for e in self.workspace()["events"] if e["entity_id"] == invoice["id"] and e["kind"] == "invoice.approve")
        require(retained["actor"] == "runtime:independent-checker" and retained["details"]["revision"] == approval["expected_revision"], "Independent invoice decision lost its actor or reviewed revision")
        self.step("actual parsed carrier invoice requires independent named review and retains the approved revision", invoice_id=invoice["id"], reviewer="runtime:entry-reviewer", approver=retained["actor"])
        settlement = self.mutate(f"loads/{lid}/settlement", {})
        require(
            settlement["amount_cents"] == 125001,
            "Actual invoice approval changed carrier cents",
        )
        bill = self.mutate(
            "bills",
            {
                "load_id": lid,
                "number": "RUNTIME-CUSTOMER-150001",
                "due_date": dt.datetime.now(dt.timezone.utc).date().isoformat(),
                "bill_to_address": "Owned synthetic customer billing address",
            },
        )
        self.mutate(f"bills/{bill['id']}/issue", {"expected_version": bill["version"]})
        self.wait(
            lambda: self.rows(
                "sync_runtime",
                "SELECT tracking FROM orders WHERE id=" + literal(identity) + "::uuid",
            )[0]["tracking"].get("freight_status")
            == "delivered",
            "ordered physical delivery tracking",
        )
        require(
            not self.workspace()["cash_entries"],
            "Qualification must not invent an external payment",
        )
        self.step(
            "actual 214 delivery and 210 cents pass reviewed POD, settlement and customer billing without payment execution",
            carrier_invoice_cents=invoice["amount_cents"],
            settlement_cents=settlement["amount_cents"],
            customer_invoice_cents=bill["snapshot"]["amount_cents"],
            pod_sha256=hashlib.sha256(pdf).hexdigest(),
            native_pod_submission_id=pod_submission["id"],
            uploaded_by=document["uploaded_by"],
        )
        isolated = self.http(
            "freight", "/v1/workspace", token="other-owned-runtime-freight-key-32-chars"
        )
        require(
            not any(v["id"] == lid for v in isolated["loads"]),
            "Other Freight tenant could read this shipment",
        )
        self.step("actual Sync and Freight tenant boundaries hold")
        cancelled = self.seed_order("FREIGHT-RUNTIME-CANCELLED")
        pending = self.import_order(cancelled, "RUNTIME-FREIGHT-CANCELLED")
        replacement_quote = self.mutate(
            "quotes",
            {
                "reference": "RUNTIME-CANCELLED-PROCUREMENT",
                "planned_load_id": pending["id"],
                "expected_load_version": pending["version"],
                "plan": {
                    k: pending[k]
                    for k in (
                        "customer_id",
                        "mode",
                        "equipment",
                        "commodity",
                        "weight_lbs",
                        "miles",
                        "origin",
                        "destination",
                    )
                },
                "sell_cents": 150001,
                "valid_until": (now + dt.timedelta(hours=12)).isoformat(),
                "terms": "Actual synthetic customer transport agreement before source cancellation",
            },
        )
        replacement_quote = self.mutate(
            f"quotes/{replacement_quote['id']}/contract-offers",
            {
                "expected_version": replacement_quote["version"],
                "agreement_id": agreement["id"],
                "expected_rate_version": agreement["version"],
                "revision_sha256": agreement["revisions"][0]["basis_sha256"],
                "valid_until": (now + dt.timedelta(hours=24)).isoformat(),
                "contact": "Actual carrier capacity lead",
                "reference": "Initial capacity before cancellation",
                "note": "Current capacity at original inclusive contract rate",
            },
        )
        replacement_quote = self.mutate(
            f"quotes/{replacement_quote['id']}/issue",
            {
                "expected_version": replacement_quote["version"],
                "offer_id": replacement_quote["revisions"][0]["offers"][0]["id"],
            },
        )
        replacement_quote = self.mutate(
            f"quotes/{replacement_quote['id']}/decision",
            {
                "expected_version": replacement_quote["version"],
                "decision": "accept",
                "contact": "Actual customer transportation lead",
                "reference": "Customer agreement before source cancellation",
                "note": "Customer confirmed the current source transportation plan",
            },
        )
        replacement_quote = self.mutate(
            f"quotes/{replacement_quote['id']}/book",
            {
                "expected_version": replacement_quote["version"],
                "expected_load_version": self.load(pending["id"])["version"],
            },
        )
        pending = self.load(pending["id"])
        self.sql(
            "sync_runtime",
            "UPDATE orders SET status='CANCELLED',updated_at=now() WHERE id="
            + literal(cancelled)
            + "::uuid",
        )
        # No prior commitment refresh observes this cancellation. The new
        # contract replacement endpoint must fetch and fence it itself.
        self.mutate(
            f"quotes/{replacement_quote['id']}/contract-reprocure",
            {
                "expected_version": replacement_quote["version"],
                "expected_load_version": pending["version"],
                "agreement_id": agreement["id"],
                "expected_rate_version": agreement["version"],
                "revision_sha256": agreement["revisions"][0]["basis_sha256"],
                "valid_until": (now + dt.timedelta(hours=24)).isoformat(),
                "contact": "Actual replacement capacity lead",
                "reference": "Proposed replacement after remote cancellation",
                "note": "Must remain held after fresh canonical observation",
                "reason": "Current replacement procurement request",
            },
            expected=409,
        )
        after_quote = next(
            q for q in self.workspace()["quotes"] if q["id"] == replacement_quote["id"]
        )
        require(
            after_quote["version"] == replacement_quote["version"]
            and after_quote["procurements"] == replacement_quote["procurements"]
            and self.load(pending["id"])["buy_cents"] == 125001,
            "Cancelled-source contract replacement changed the original award",
        )
        self.step(
            "contract replacement independently refreshes cancelled real Sync source and retains original procurement",
            quote_id=replacement_quote["id"],
            sync_order_id=cancelled,
            retained_buy_cents=125001,
        )
        self.mutate(
            f"loads/{pending['id']}/commands",
            {
                "action": "tender",
                "expected_version": pending["version"],
                "carrier_id": "ca4",
            },
            expected=409,
        )
        require(
            self.sync_order(cancelled)["status"].lower() == "cancelled",
            "Source cancellation was overwritten",
        )
        require(
            not any(t["load_id"] == pending["id"] for t in self.workspace()["tenders"]),
            "Cancelled source created a tender",
        )
        self.step("real source cancellation holds tender and preserves upstream status")
        self.command_load(
            pending["id"],
            "cancel",
            note="Actual customer cancelled before transport commitment",
        )
        self.wait(
            lambda: next(
                (
                    j
                    for j in self.workspace()["outbox"]
                    if j["load_id"] == pending["id"]
                    and j["destination"] == "stateset-sync-server"
                    and j["status"] in ("held", "local_only")
                ),
                None,
            ),
            "cancelled tracking retained safely",
        )
        require(
            self.sync_order(cancelled)["status"].lower() == "cancelled",
            "Cancellation tracking marked source shipped",
        )
        release_order = self.seed_order("FREIGHT-RUNTIME-NATIVE-RELEASE")
        releasable = self.import_order(release_order, "RUNTIME-CARRIER-RELEASE")
        release_load = releasable["id"]
        self.command_load(release_load, "tender", carrier_id="ca4")
        self.wait(
            lambda: any(
                j["load_id"] == release_load
                and j["destination"] == "stateset-edi"
                and j["status"] == "gateway_accepted"
                for j in self.workspace()["outbox"]
            ),
            "actual gateway handoff before carrier release",
        )
        request = self.mutate(
            f"loads/{release_load}/release-requests",
            {
                "expected_load_version": self.load(release_load)["version"],
                "disposition": "cancel",
                "reason": "Private owned operations cancellation coordination",
            },
        )
        publication = self.mutate(
            f"release-requests/{request['id']}/portal-publications",
            {
                "expected_load_version": self.load(release_load)["version"],
                "message": "Carrier please confirm no pickup or custody and release this exact assignment",
                "respond_by": (
                    dt.datetime.now(dt.timezone.utc) + dt.timedelta(hours=24)
                ).isoformat(),
            },
        )
        self.sql(
            "sync_runtime",
            "UPDATE orders SET status='CANCELLED',updated_at=now() WHERE id="
            + literal(release_order)
            + "::uuid",
        )
        carrier_release = self.http(
            "freight",
            f"/v1/portal/releases/{publication['id']}/response",
            method="POST",
            token=CARRIER_KEY,
            body={
                "expected_load_version": self.load(release_load)["version"],
                "document_sha256": publication["sha256"],
                "decision": "release",
                "no_custody": True,
                "contact": "Owned carrier dispatcher",
                "reference": "Actual synthetic authenticated carrier release",
                "note": "Carrier verified driver and confirms no pickup or custody of this assignment",
            },
        )["data"]
        require(
            carrier_release["outcome"] == "applied"
            and carrier_release["recorded_by"] == "runtime:carrier"
            and self.load(release_load)["status"] == "cancelled",
            "Native carrier release lost source identity or cancelled the wrong shipment",
        )
        self.wait(
            lambda: any(
                j["load_id"] == release_load
                and j["destination"] == "stateset-sync-server"
                and j["status"] in ("held", "local_only")
                for j in self.workspace()["outbox"]
            ),
            "native release cancellation tracking held safely",
        )
        require(
            self.sync_order(release_order)["status"].lower() == "cancelled",
            "Native release cancellation tracking incorrectly marked commerce shipped",
        )
        self.step(
            "authenticated carrier release cancels exact real EDI tender while preserving cancelled Sync order",
            publication_id=publication["id"],
            document_sha256=publication["sha256"],
            response_id=carrier_release["id"],
            recorded_by=carrier_release["recorded_by"],
            sync_order_id=release_order,
            outcome=carrier_release["outcome"],
        )
        self.multi_stop_checks()
        callback = self.wait(
            lambda: next(
                (
                    j
                    for j in self.rows(
                        "edi_runtime",
                        "SELECT id,status,last_error FROM callback_deliveries",
                    )
                    if j["status"] == "held" and "public" in (j["last_error"] or "")
                ),
                None,
            ),
            "native EDI private-address callback refusal",
        )
        self.step(
            "native EDI callback policy holds private destination; local relay does not mark it delivered",
            status=callback["status"],
        )
        before = self.workspace()
        self.stop("freight")
        self.spawn("freight", self.freight_env)
        self.wait(lambda: self.http("freight", "/health"), "Freight restart")
        after = self.workspace()
        require(
            next(l for l in after["loads"] if l["id"] == lid)["sync_order_id"]
            == identity,
            "Restart lost source identity",
        )
        require(
            len(after["inbox"]) == len(before["inbox"]),
            "Restart lost or duplicated signed callback receipts",
        )
        self.step(
            "Freight restart against real database preserves source identity and callback receipts"
        )

    def multi_stop_checks(self):
        identity = self.seed_order("FREIGHT-RUNTIME-FOUR-STOP")
        plan = self.plan("RUNTIME-FOUR-STOP")
        plan["intermediate_stops"] = [
            {"id": "stop_tacoma", "kind": "delivery", "stop": {**plan["destination"], "facility": "Runtime Tacoma receiver", "city": "Tacoma", "address_lines": ["30 Owned Receiver Way"], "postal_code": "98402"}},
            {"id": "stop_kent", "kind": "pickup", "stop": {**plan["destination"], "facility": "Runtime Kent consolidation", "city": "Kent", "address_lines": ["40 Owned Consolidation Way"], "postal_code": "98032"}},
        ]
        lid = self.mutate("sync/import", {"order_id": identity, "load": plan})["id"]
        self.command_load(lid, "tender", carrier_id="ca4")
        load = self.load(lid)
        job = next(j for j in self.workspace()["outbox"] if j["load_id"] == lid and j["destination"] == "stateset-edi")
        require(job["status"] == "native_only" and job["total_attempts"] == 0, "Multi-stop tender attempted an incomplete EDI handoff")
        self.mutate(f"outbox/{job['id']}/retry", {}, expected=422)
        for stop, hour in [("stop_tacoma", "10"), ("stop_kent", "11")]:
            self.mutate(f"loads/{lid}/appointments", {"expected_load_version": self.load(lid)["version"], "stop": stop, "start_local": plan["origin"]["date"] + f"T{hour}:00", "end_local": plan["origin"]["date"] + f"T{hour}:30", "timezone": "America/Los_Angeles", "contact": "Owned actual facility scheduling lead", "confirmation_reference": "Actual window " + stop, "note": "Original confirmed route appointment"})
        rate = self.rate_confirmation(lid, {"expected_load_version": self.load(lid)["version"], "terms": "Explicit full four-stop carrier agreement", "reason": "Actual route-specific carrier agreement"})
        offer = self.mutate(f"loads/{lid}/portal-offers", {"expected_load_version": self.load(lid)["version"], "rate_confirmation_id": rate["id"]})
        html = self.http("freight", f"/v1/portal/rate-confirmations/{rate['id']}/statement", token=CARRIER_KEY)
        require(hashlib.sha256(html.encode()).hexdigest() == rate["sha256"] and all(value in html for value in ["10:00", "11:00", "America/Los_Angeles", "Owned actual facility scheduling lead"]) and html.index("Runtime pickup") < html.index("Runtime Tacoma receiver") < html.index("Runtime Kent consolidation") < html.index("Runtime delivery"), "Full native route document lost stop order, actual appointment windows, or exact bytes")
        self.http("freight", f"/v1/portal/offers/{offer['id']}/response", method="POST", token=CARRIER_KEY, body={"expected_load_version": self.load(lid)["version"], "decision": "accept", "rate_confirmation_id": offer["rate_confirmation_id"], "rate_sha256": offer["rate_sha256"], "reference": "Actual owned full route acceptance", "note": "Carrier reviewed all four stops and full inclusive rate"})
        self.step("full native four-stop agreement retains real Sync identity and prevents incomplete EDI 204 handoff", sync_order_id=identity, tender_id=load["tender_id"], document_sha256=rate["sha256"], edi_attempts=job["total_attempts"])
        self.http("freight", f"/v1/portal/offers/{offer['id']}/tracking", method="POST", token=CARRIER_KEY, body={"expected_load_version": self.load(lid)["version"], "kind": "pickup", "observed_at": (dt.datetime.now(dt.timezone.utc)-dt.timedelta(minutes=20)).isoformat(), "location": "Portland, OR", "reference": "Actual origin pickup", "note": "Carrier confirms physical origin custody"})
        def serve(stop, observed):
            return self.http("freight", f"/v1/portal/offers/{offer['id']}/stop-services", method="POST", token=CARRIER_KEY, body={"expected_load_version": self.load(lid)["version"], "stop_id": stop, "observed_at": observed, "contact": "Owned actual facility lead", "reference": "Actual service " + stop, "note": "Carrier verified actual intermediate service"})["data"]
        def native_pickup_roundtrip():
            value = self.rows("sync_runtime", "SELECT tracking FROM orders WHERE id=" + literal(identity) + "::uuid")[0]["tracking"] or {}
            return value.get("freight_status") == "in_transit" and any(m.get("kind") == "pickup" and m.get("corrected") is False for m in value.get("freight_actual_milestones", []))
        self.wait(native_pickup_roundtrip, "multi-stop pickup and actual source in real Sync")
        first = serve("stop_tacoma", (dt.datetime.now(dt.timezone.utc)-dt.timedelta(minutes=15)).isoformat())
        second_time = (dt.datetime.now(dt.timezone.utc)-dt.timedelta(minutes=10)).isoformat()
        tender = next(t for t in self.workspace()["tenders"] if t["id"] == self.load(lid)["tender_id"])
        now = dt.datetime.now(dt.timezone.utc)
        self.http("edi", "/v1/edi/inbound/brex", method="POST", body=self.raw("214", "QM", f"B10*{tender['shipment_reference']}*RUNTIME-FOUR-STOP*BREX~LX*1~MS1*Seattle*WA*US~AT7*D1*NS***{now:%Y%m%d}*{now:%H%M}*UT~", 9), headers={"Content-Type": "application/edi-x12"})
        self.relay("214")
        self.wait(lambda: any(j["load_id"] == lid and j["destination"] == "stateset-sync-server" and j["status"] == "local_only" and j["payload"]["trackingData"].get("freight_status") == "delivered" for j in self.workspace()["outbox"]), "final delivery held until missing intermediate proof")
        require(self.load(lid)["status"] == "delivered" and self.load(lid)["route_attention"] and not self.load(lid)["pod_ready"], "Actual final D1 concealed missing intermediate service")
        tracking = self.rows("sync_runtime", "SELECT tracking FROM orders WHERE id=" + literal(identity) + "::uuid")[0]["tracking"]
        require(tracking.get("freight_status") == "in_transit", "Actual Sync incorrectly received incomplete final route delivery")
        self.step("actual signed final 214 preserves physical delivery while missing intermediate proof holds final Sync tracking and finance", sync_order_id=identity, original_service_id=first["id"], sync_freight_status=tracking.get("freight_status"))
        queue = self.http("freight", "/v1/operations?view=active&search=RUNTIME-FOUR-STOP")
        issue = next(i for i in queue["issues"] if i["signal"]["kind"] == "incomplete_route")
        original = self.load(lid)
        actual_actor = self.workspace()["access"]["subject"]
        claim = self.mutate(f"operations/issues/{issue['id']}/actions", {"expected_version": (issue["case"] or {}).get("version", 0), "expected_source_active": issue["source_active"], "source_sha256": issue["source_sha256"], "action": "claim", "note": "PRIVATE-RUNTIME-CONTROL-TOWER actual dispatcher coordinates missing stop proof", "follow_up_at": (dt.datetime.now(dt.timezone.utc)+dt.timedelta(hours=1)).isoformat()})
        require(claim["owner"] == actual_actor and self.load(lid) == original, "Owning route recovery changed shipment facts or lost the authenticated owner")
        held = self.mutate(f"operations/issues/{issue['id']}/actions", {"expected_version": claim["version"], "expected_source_active": True, "source_sha256": issue["source_sha256"], "action": "snooze", "note": "Critical route hold must remain visible", "defer_until": (dt.datetime.now(dt.timezone.utc)+dt.timedelta(minutes=15)).isoformat()}, expected=422)
        require("cannot be deferred" in held["error"]["message"], "Critical route problem was hidden by work-queue deferral")
        second = serve("stop_kent", second_time)
        self.wait(lambda: (self.rows("sync_runtime", "SELECT tracking FROM orders WHERE id=" + literal(identity) + "::uuid")[0]["tracking"] or {}).get("freight_route_complete") is True, "complete route tracking in actual Sync")
        tracking = self.rows("sync_runtime", "SELECT tracking FROM orders WHERE id=" + literal(identity) + "::uuid")[0]["tracking"]
        require(tracking.get("freight_status") == "delivered" and len(tracking.get("freight_route_stops", [])) == 4 and not self.load(lid)["route_attention"], "Complete route tracking lost ordered stop identity")
        require(not self.load(lid)["pod_ready"], "Route confirmations invented finance-reviewed POD")
        self.step("late actual intermediate confirmation releases complete four-stop Sync tracking without inventing reviewed POD", final_service_id=second["id"], source_order_id=identity, route_stop_count=len(tracking["freight_route_stops"]))
        resolved = self.http("freight", f"/v1/operations/issues/{issue['id']}")
        require(resolved["source_active"] is False and resolved["case"]["status"] == "resolved" and resolved["case"]["owner"] == actual_actor, "Actual service proof did not resolve retained owned work")
        require(resolved["case"]["history"][-1]["action"] == "source_cleared" and any(e["action"] == "claim" and e["actor"] == actual_actor for e in resolved["case"]["history"]), "Control tower lost original ownership or source-verified recovery")
        portal = self.http("freight", "/v1/portal", token=CARRIER_KEY)
        require("PRIVATE-RUNTIME-CONTROL-TOWER" not in json.dumps(portal) and "operations_cases" not in portal, "Partner view exposed private operations work")
        self.step("control tower owns actual EDI route recovery, rejects critical deferral, and closes only after real service releases Sync tracking", issue_id=issue["id"], owner=actual_actor, source_order_id=identity, retained_history_count=len(resolved["case"]["history"]))
        baseline = self.workspace()
        def gate(minutes):
            return {"expected_load_version": self.load(lid)["version"], "stop": "stop_kent", "kind": "arrival", "local_time": (dt.datetime.now(dt.timezone.utc)-dt.timedelta(minutes=minutes)).strftime("%Y-%m-%dT%H:%M"), "timezone": "UTC", "reference": "PRIVATE-RUNTIME-EXECUTION-GATE", "note": "Actual facility gate evidence"}
        rejected = self.mutate(f"loads/{lid}/stop-events", gate(17), expected=422)
        require("chronology" in rejected["error"]["message"], "Contradictory cross-stop gate did not explain the timeline conflict")
        unchanged = self.workspace()
        require(len(unchanged["stop_events"]) == len(baseline["stop_events"]) and len([e for e in unchanged["events"] if e["kind"] == "stop.event.record"]) == len([e for e in baseline["events"] if e["kind"] == "stop.event.record"]) and len(unchanged["outbox"]) == len(baseline["outbox"]), "Rejected gate changed execution or queue history")
        actual_gate = self.mutate(f"loads/{lid}/stop-events", gate(12))
        public = next(o for o in self.http("freight", "/v1/portal", token=CARRIER_KEY)["offers"] if o["id"] == offer["id"])["reporting"]["execution_timeline"]
        require(public["consistent"] and public["record_count"] == 4 and "PRIVATE-RUNTIME-EXECUTION-GATE" not in json.dumps(public), "Scoped execution timeline lost facts or exposed private gate evidence")
        def gate_roundtrip():
            value = self.rows("sync_runtime", "SELECT tracking FROM orders WHERE id=" + literal(identity) + "::uuid")[0]["tracking"] or {}
            arrival = value.get("freight_stops", {}).get("stop_kent", {}).get("arrival_at")
            return value.get("freight_execution_consistent") is True and arrival and dt.datetime.fromisoformat(arrival.replace("Z", "+00:00")) == dt.datetime.fromisoformat(actual_gate["occurred_at"].replace("Z", "+00:00"))
        self.wait(gate_roundtrip, "exact actual gate and consistency marker in real Sync")
        self.step("shared execution chronology rejects contradictory gates atomically and retains consistent scoped timeline through real Sync", gate_id=actual_gate["id"], source_order_id=identity, record_count=public["record_count"])

        before = self.load(lid)
        source = next(s for s in before["movement_sources"] if s["source"]["kind"] == "pickup")
        original_source = source["source"]
        corrected = self.mutate(f"carrier-submissions/{original_source['id']}/time-corrections", {"expected_load_version": before["version"], "source_sha256": source["source_sha256"], "expected_previous_id": None, "observed_at": (dt.datetime.now(dt.timezone.utc)-dt.timedelta(minutes=19)).isoformat(), "contact": "PRIVATE-RUNTIME-CARRIER-CONTACT", "reference": "PRIVATE-RUNTIME-MOVEMENT-CORRECTION", "note": "Actual dispatcher and facility evidence reviewed against original carrier report"})
        after = self.load(lid)
        unchanged_fields = ["status", "location", "last_tracking_at", "tender_id", "carrier_id", "buy_cents", "sell_cents", "sync_order_id", "order_number", "pod_review", "pod_reference"]
        require(all(after[k] == before[k] for k in unchanged_fields), "Time correction rewound physical facts, prices, identity, or POD")
        require(next(s for s in self.workspace()["carrier_submissions"] if s["id"] == original_source["id"]) == original_source, "Time correction edited original carrier evidence")
        public = next(o for o in self.http("freight", "/v1/portal", token=CARRIER_KEY)["offers"] if o["id"] == offer["id"])["reporting"]["execution_timeline"]
        fact = next(f for f in public["stops"][0]["facts"] if f["kind"] == "pickup")
        require(fact["corrected"] and fact["observed_at"] == corrected["observed_at"] and fact["original_observed_at"] == original_source["observed_at"] and "PRIVATE-RUNTIME" not in json.dumps(public), "Scoped movement correction lost provenance or exposed private review")
        def movement_roundtrip():
            value = self.rows("sync_runtime", "SELECT tracking FROM orders WHERE id=" + literal(identity) + "::uuid")[0]["tracking"] or {}
            milestones = value.get("freight_actual_milestones", [])
            return value.get("freight_execution_consistent") is True and any(m.get("kind") == "pickup" and m.get("corrected") is True and m.get("observed_at") == corrected["observed_at"] for m in milestones)
        self.wait(movement_roundtrip, "effective corrected native actual time in real Sync")
        tracking = self.rows("sync_runtime", "SELECT tracking FROM orders WHERE id=" + literal(identity) + "::uuid")[0]["tracking"]
        require("PRIVATE-RUNTIME" not in json.dumps(tracking), "Sync tracking exposed private movement review")
        require(hashlib.sha256(self.http("freight", f"/v1/portal/rate-confirmations/{rate['id']}/statement", token=CARRIER_KEY).encode()).hexdigest() == rate["sha256"], "Time correction changed original carrier agreement bytes")
        self.step("reviewed native movement correction retains original report and physical state while corrected actual time reaches real Sync", source_id=original_source["id"], correction_id=corrected["id"], source_sha256=corrected["source_sha256"], source_order_id=identity)

    def cleanup(self):
        errors = []
        try:
            for name in list(reversed(self.processes)):
                try:
                    self.stop(name)
                except BaseException as error:
                    errors.append(self.safe(f"{name}: {error}"))
        finally:
            if self.native_postgres is not None:
                self.report.setdefault('postgres', {})['cleanup'] = self.native_postgres.stop()
        if self.container:
            label = self.command(
                [
                    "docker",
                    "inspect",
                    "--format",
                    '{{index .Config.Labels "com.stateset.freight.runtime"}}',
                    self.container,
                ],
                label="owned container identity",
            )
            require(
                label == self.container_label, "Refusing to stop an unowned container"
            )
            self.command(
                ["docker", "stop", "--time", "15", self.container],
                label="owned PostgreSQL stop",
            )
            # --rm cleanup can outlive `docker stop`. Reusing this owned name
            # before removal finishes races the daemon during a fresh restore.
            deadline = time.monotonic() + 30
            while self.command(
                ["docker", "ps", "--all", "--quiet", "--no-trunc", "--filter", "id=" + self.container],
                label="owned PostgreSQL removal verification",
                timeout=5,
            ):
                require(time.monotonic() < deadline, "Owned PostgreSQL removal did not finish")
                time.sleep(0.1)
            self.container = None
        if errors:
            raise RuntimeError("Owned service cleanup failed: " + "; ".join(errors))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--build-dir", type=Path, default=Path("/tmp/freight-runtime-target")
    )
    backend = parser.add_mutually_exclusive_group()
    backend.add_argument("--postgres-image", default="postgres:16")
    backend.add_argument("--postgres-bin", type=Path, help="Run an isolated native PostgreSQL 16 cluster instead of Docker")
    args = parser.parse_args()
    args.build_dir = args.build_dir.resolve()
    args.sync_repo = ROOT.parent / "stateset-sync-server"
    args.edi_repo = ROOT.parent / "stateset-edi"
    runtime = Runtime(args)

    def interrupted(_signal, _frame):
        raise KeyboardInterrupt()

    signal.signal(signal.SIGTERM, interrupted)
    try:
        runtime.build()
        runtime.postgres()
        runtime.sync()
        runtime.edi()
        runtime.freight()
        runtime.checks()
        runtime.verify_inputs()
        runtime.report["result"] = "pass"
    except BaseException as error:
        runtime.report["result"] = "failed"
        runtime.report["error"] = runtime.safe(f"{type(error).__name__}: {error}")
        print(runtime.report["error"], file=sys.stderr, flush=True)
    finally:
        try:
            runtime.cleanup()
        except BaseException as error:
            runtime.report["cleanup_error"] = runtime.safe(str(error))
            runtime.report["result"] = "failed"
        runtime.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        runtime.save()
        print(f"REPORT {runtime.directory / 'report.json'}", flush=True)
    return 0 if runtime.report["result"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
