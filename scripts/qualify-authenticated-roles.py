#!/usr/bin/env python3
"""Qualify the separate-role Compose workflow using exact cached images.

Owns one new Compose project, database volume and loopback ports. Uses synthetic
passwords and business records only. Never accepts a deployment database URL.
"""

import argparse
import copy
import datetime as dt
import hashlib
import http.cookiejar
import ipaddress
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import traceback
import urllib.error
import urllib.request
import uuid

from runtime_contracts import ROOT, NoRedirect, fingerprint, free_port, require
import profile_document_previews as previews

D = ["docker", "--host", "unix:///var/run/docker.sock"]
INPUTS = [
    "compose.restricted.yaml",
    "scripts/configure-runtime-role.sh",
    "scripts/configure-runtime-role.sql",
    "scripts/qualify-authenticated-roles.py",
] + previews.INPUTS


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def private_docker_subnet(value, existing):
    """Validate an explicitly selected small private qualification network."""
    network = ipaddress.ip_network(value, strict=True)
    private = [ipaddress.ip_network(cidr) for cidr in
               ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']]
    if network.version != 4 or not 24 <= network.prefixlen <= 28 or not any(
            network.subnet_of(parent) for parent in private):
        raise ValueError('Select an unused RFC1918 IPv4 subnet with prefix /24 through /28')
    for value in existing:
        allocated = ipaddress.ip_network(value, strict=False)
        if allocated.version == 4 and network.overlaps(allocated):
            raise ValueError('Selected qualification subnet overlaps an existing Docker network')
    return str(network)


class RoleQualification:
    def __init__(self, args):
        self.args = args
        parent = json.loads(args.verification.read_text())
        require(
            parent["result"] == "pass", "Passing image/source verification required"
        )
        self.source = fingerprint(ROOT)
        require(
            parent["source"] == self.source, "Current API differs from image evidence"
        )
        self.images = {n: parent["images"][n] for n in ["freight", "web", "postgres"]}
        selected_client = getattr(args, 'tui', None)
        self.client_binary = selected_client.resolve() if selected_client is not None else None
        if self.client_binary is not None:
            require(self.client_binary.is_file() and os.access(self.client_binary, os.X_OK),
                    'Select an existing built TUI executable')
            if parent.get('client'):
                require(sha(self.client_binary) == parent['client']['sha256'],
                        'Selected TUI differs from image/client evidence')
                require(all(sha(ROOT / name) == digest for name, digest in
                            parent['client']['source_inputs'].items()), 'TUI source evidence drift')
        self.directory = Path(tempfile.mkdtemp(prefix="stateset-freight-auth-"))
        self.project = "freight-auth-" + uuid.uuid4().hex[:12]
        self.manifest = {n: sha(ROOT / n) for n in INPUTS}
        for name in INPUTS:
            dst = self.directory / name
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, dst)
            dst.chmod(0o644)
        (self.directory / ".env").write_text("# Empty owned configuration.\n")
        self.owner_password = "Owned-owner:" + uuid.uuid4().hex + "@/ ?#%$'\"\\é"
        self.runtime_password = "Owned-runtime:" + uuid.uuid4().hex + "@/ ?#%$'\"\\雪"
        self.old_passwords = []
        self.token = "owned-auth-role-key-" + uuid.uuid4().hex
        self.other_token = "owned-auth-other-key-" + uuid.uuid4().hex
        self.ports = {"api": free_port(), "web": free_port()}
        require(len(set(self.ports.values())) == 2, "Owned ports collided")
        self.env = {
            "PATH": os.defpath,
            "LC_ALL": "C.UTF-8",
            "POSTGRES_PASSWORD": self.owner_password,
            "FREIGHT_RUNTIME_DATABASE_PASSWORD": self.runtime_password,
            "FREIGHT_API_IMAGE": self.images["freight"],
            "FREIGHT_WEB_IMAGE": self.images["web"],
            "FREIGHT_POSTGRES_IMAGE": self.images["postgres"],
            "FREIGHT_API_PORT": str(self.ports["api"]),
            "FREIGHT_WEB_PORT": str(self.ports["web"]),
            "FREIGHT_WEB_ORIGIN": f"http://127.0.0.1:{self.ports['web']}",
            "FREIGHT_API_KEYS_JSON": json.dumps(
                {
                    self.token: {
                        "tenant_id": "demo",
                        "subject": "owned:role-reviewer",
                        "role": "admin",
                    },
                    self.other_token: {
                        "tenant_id": "other",
                        "subject": "owned:other-role-reviewer",
                        "role": "admin",
                    },
                }
            ),
            "FREIGHT_INTEGRATIONS_JSON": "{}",
            "FREIGHT_SESSION_SECRET": "owned-authenticated-role-session-"
            + uuid.uuid4().hex,
            "FREIGHT_DEMO": "true",
        }
        self.compose = D + [
            "compose",
            "--project-name",
            self.project,
            "--file",
            str(self.directory / "compose.restricted.yaml"),
            "--env-file",
            str(self.directory / ".env"),
        ]
        self.report = {
            "result": "running",
            "scope": "Actual dedicated Compose schema-job/managed-runtime-role workflow with SCRAM authentication, original image users/healthchecks, synthetic secrets and owned local database; no TLS, production commissioning or off-host recovery claim.",
            "source": self.source,
            "images": self.images,
            "inputs": self.manifest,
            "parent": {
                "path": str(args.verification),
                "sha256": sha(args.verification),
            },
            "project": self.project,
            "artifacts": str(self.directory),
            "ports": self.ports,
            "checks": [],
            "client_graphics": dict(requested=self.client_binary is not None, phases=[],
                binary=str(self.client_binary) if self.client_binary is not None else None,
                sha256=sha(self.client_binary) if self.client_binary is not None else None),
            "started_at": dt.datetime.now(dt.timezone.utc).isoformat(),
            "limitations": [
                "Docker bridge networking and loopback publication; no TLS/ingress/network-policy qualification.",
                "Synthetic passwords remain in the private controller environment and privileged Docker configuration; external secret-manager commissioning is separate.",
                "The schema/role administrator retains database ownership; only the serving Freight login is restricted.",
                "Application and full EDI/Sync recovery reports retain their own source and counts; this driver adds separate operating evidence.",
            ],
        }
        self.calls = 0
        subnet = getattr(args, 'docker_subnet', None)
        if subnet is not None:
            # Inspect only address allocations; no unrelated resource is changed.
            private_docker_subnet(subnet, [])
            identities = self.run(D + ['network', 'ls', '--quiet'],
                                  label='existing Docker network inventory')[0].split()
            allocated = []
            if identities:
                values = self.run(D + ['network', 'inspect', '--format',
                                       '{{json .IPAM.Config}}', *identities],
                                  label='existing Docker subnet inventory')[0]
                for line in values.splitlines():
                    allocated.extend(v['Subnet'] for v in (json.loads(line) or [])
                                     if v.get('Subnet'))
            subnet = private_docker_subnet(subnet, allocated)
            override = self.directory / 'qualification-network.json'
            override.write_text(json.dumps({'networks': {'default': {
                'ipam': {'config': [{'subnet': subnet}]}}}}, indent=2) + '\n')
            self.compose += ['--file', str(override)]
            self.report['network_override'] = {'subnet': subnet, 'sha256': sha(override),
                                               'existing_allocations_checked': len(allocated)}

    def safe(self, value):
        value = str(value)
        for secret in [
            self.owner_password,
            self.runtime_password,
            self.token,
            self.other_token,
            self.env["FREIGHT_SESSION_SECRET"],
            *self.old_passwords,
        ]:
            # Compose's serializable config doubles dollars; JSON additionally
            # escapes quotes/backslashes/Unicode. Redact all display forms.
            for form in [secret, secret.replace("$", "$$")]:
                for rendered in [
                    form,
                    json.dumps(form)[1:-1],
                    json.dumps(form, ensure_ascii=False)[1:-1],
                ]:
                    value = value.replace(rendered, "[owned-test-secret]")
        return value

    def save(self):
        (self.directory / "report.json").write_text(
            self.safe(json.dumps(self.report, indent=2)) + "\n"
        )

    def check(self, name, **data):
        self.report["checks"].append({"name": name, "result": "pass", **data})
        self.save()
        print("PASS", name, flush=True)

    def run(
        self, command, *, env=None, input=None, expected=0, timeout=180, label="command"
    ):
        self.calls += 1
        result = subprocess.run(
            command,
            cwd=self.directory,
            env=env or self.env,
            input=input,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )
        with (self.directory / f"command-{self.calls:03d}.log").open("x") as out:
            out.write(self.safe(result.stdout + result.stderr))
        require(
            (
                result.returncode == expected
                if expected is not None
                else result.returncode != 0
            ),
            label + " failed: " + self.safe(result.stderr[-2500:]),
        )
        return result.stdout.strip(), result.stderr, result.returncode

    def compose_run(self, *args, **kwargs):
        return self.run(self.compose + list(args), **kwargs)

    def service(self, name):
        identities = self.compose_run("ps", "--all", "--quiet", name)[0].splitlines()
        require(len(identities) == 1, "Expected one owned " + name)
        item = json.loads(self.run(D + ["inspect", identities[0]])[0])[0]
        require(
            item["Config"]["Labels"].get("com.docker.compose.project") == self.project,
            "Unowned Compose resource",
        )
        return item

    def tcp_sql_environment(self, user, password):
        return {
            "PGHOST": "127.0.0.1",
            "PGUSER": user,
            "PGDATABASE": "freight",
            "PGPASSWORD": password or "",
        }

    def verify_host_authentication(self):
        rules = json.loads(
            self.sql(
                "SELECT json_agg(auth_method) FROM pg_hba_file_rules WHERE type LIKE 'host%' AND error IS NULL"
            )[0]
        )
        require(
            rules and set(rules) == {"scram-sha-256"},
            "Host authentication is not uniformly SCRAM",
        )

    def sql(self, statement, *, user="freight", password=None, expected=0):
        pg = self.service("postgres")["Id"]
        command = D + ["exec"]
        env = dict(self.env)
        if user != "freight" or password is not None:
            settings = self.tcp_sql_environment(user, password)
            env.update(settings)
            for setting in settings:
                command += ["-e", setting]
            command += [pg, "psql", "-w"]
        else:
            command += [pg, "psql", "-U", "freight", "-d", "freight"]
        return self.run(
            command
            + [
                "-X",
                "-At",
                "-v",
                "ON_ERROR_STOP=1",
                "-v",
                "VERBOSITY=verbose",
                "-c",
                statement,
            ],
            env=env,
            expected=expected,
            label="owned SQL",
        )

    def metadata(self):
        return self.sql(
            "SELECT json_build_object('history',(SELECT json_agg(t ORDER BY version) FROM freight_schema_migrations t),'format',(SELECT row_to_json(t) FROM freight_storage_format t))"
        )[0]

    def business(self):
        tables = self.sql(
            "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename"
        )[0].splitlines()
        return {
            name: hashlib.sha256(
                self.sql(
                    f'SELECT to_jsonb(t)::text FROM public."{name}" t ORDER BY to_jsonb(t)::text COLLATE "C"'
                )[0].encode()
            ).hexdigest()
            for name in tables
        }

    def role(self):
        return self.sql(
            "SELECT json_build_object('role',(SELECT row_to_json(t) FROM (SELECT rolname,rolsuper,rolinherit,rolcreaterole,rolcreatedb,rolcanlogin,rolreplication,rolbypassrls,rolvaliduntil,rolconfig FROM pg_roles WHERE rolname='freight_app')t),'password_hash',(SELECT encode(sha256(rolpassword::bytea),'hex') FROM pg_authid WHERE rolname='freight_app'),'comment',shobj_description((SELECT oid FROM pg_roles WHERE rolname='freight_app'),'pg_authid'),'version_write',has_column_privilege('freight_app','freight_storage_format','version','UPDATE'),'history_write',has_table_privilege('freight_app','freight_schema_migrations','INSERT,UPDATE,DELETE,TRUNCATE,TRIGGER,REFERENCES'))"
        )[0]

    def http(self, path, body=None, key=None, *, web=False, opener=None, expected=200):
        self.report["last_http"] = {
            "service": "web" if web else "api",
            "path": path,
            "method": "POST" if body is not None else "GET",
        }
        port = self.ports["web" if web else "api"]
        headers = (
            {"Origin": f"http://127.0.0.1:{self.ports['web']}"}
            if web
            else {"Authorization": "Bearer " + self.token}
        )
        if key:
            headers["Idempotency-Key"] = key
        data = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(body, separators=(",", ":")).encode()
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}" + path, data=data, headers=headers
        )
        client = opener or urllib.request.build_opener(
            urllib.request.ProxyHandler({}), NoRedirect()
        )
        try:
            response = client.open(request, timeout=10)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            raw = response.read()
            require(
                response.status == expected,
                "Unexpected HTTP status "
                + str(response.status)
                + " "
                + path
                + " "
                + self.safe(raw[:1000]),
            )
            return json.loads(raw)

    def wait_ready(self):
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            try:
                if self.http("/ready"):
                    item = self.service("api")
                    if item["State"].get("Health", {}).get("Status") == "healthy":
                        return
            except (OSError, TimeoutError, AssertionError):
                pass
            time.sleep(0.3)
        raise TimeoutError("Actual API readiness/default image health did not recover")

    def wait_web_access(self):
        # Only poll a read while a newly recreated Node server boots. Permanent
        # authentication refusals fail immediately; no write is resubmitted.
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            try:
                access = self.http(
                    "/api/freight/access", web=True, opener=self.web_client
                )
                require(
                    access["tenant_id"] == "demo", "Original web session owner changed"
                )
                return
            except (OSError, urllib.error.URLError):
                pass
            except AssertionError as error:
                if not str(error).startswith(
                    ("Unexpected HTTP status 502 ", "Unexpected HTTP status 503 ")
                ):
                    raise
            time.sleep(0.3)
        raise TimeoutError("Recreated web server did not accept the original session")

    def cancelled_role_job(self):
        self.sql("GRANT UPDATE(version) ON freight_storage_format TO freight_app")
        before = (self.role(), self.metadata(), self.business())
        blocker_name = "owned-role-blocker-" + uuid.uuid4().hex
        job_name = self.project + "-cancelled-grants"
        job_app = "owned-role-job-" + uuid.uuid4().hex
        pg = self.service("postgres")["Id"]
        pending_password = "Owned-cancelled-" + uuid.uuid4().hex
        self.old_passwords.append(pending_password)
        env = {**self.env, "FREIGHT_RUNTIME_DATABASE_PASSWORD": pending_password}
        blocker = job = None
        try:
            with (self.directory / "password-lock-blocker.log").open("x") as out:
                blocker = subprocess.Popen(
                    D
                    + [
                        "exec",
                        "-i",
                        "-e",
                        "PGAPPNAME=" + blocker_name,
                        pg,
                        "psql",
                        "-U",
                        "freight",
                        "-d",
                        "freight",
                        "-X",
                        "-At",
                        "-v",
                        "ON_ERROR_STOP=1",
                    ],
                    env=self.env,
                    stdin=subprocess.PIPE,
                    stdout=out,
                    stderr=subprocess.STDOUT,
                    text=True,
                )
                blocker.stdin.write(
                    "BEGIN; SELECT oid FROM pg_authid WHERE rolname='freight_app' FOR UPDATE; SELECT pg_sleep(90);\n"
                )
                blocker.stdin.close()
                deadline = time.monotonic() + 15
                while (
                    self.sql(
                        "SELECT count(*) FROM pg_stat_activity WHERE application_name='"
                        + blocker_name
                        + "' AND wait_event='PgSleep'"
                    )[0]
                    != "1"
                ):
                    require(
                        blocker.poll() is None and time.monotonic() < deadline,
                        "Password row-lock blocker did not become live",
                    )
                    time.sleep(0.1)
                with (self.directory / "cancelled-role-job.log").open("x") as job_log:
                    job = subprocess.Popen(
                        self.compose
                        + [
                            "run",
                            "--rm",
                            "--no-deps",
                            "-T",
                            "--name",
                            job_name,
                            "-e",
                            "PGAPPNAME=" + job_app,
                            "grants",
                        ],
                        env=env,
                        cwd=self.directory,
                        stdout=job_log,
                        stderr=subprocess.STDOUT,
                        text=True,
                    )
                    deadline = time.monotonic() + 25
                    while (
                        self.sql(
                            "SELECT count(*) FROM pg_stat_activity WHERE application_name='"
                            + job_app
                            + "' AND wait_event_type='Lock' AND query LIKE 'ALTER%freight_app%'"
                        )[0]
                        != "1"
                    ):
                        require(
                            job.poll() is None and time.monotonic() < deadline,
                            "Actual role job did not reach its blocked password update",
                        )
                        time.sleep(0.1)
                    item = json.loads(self.run(D + ["inspect", job_name])[0])[0]
                    require(
                        item["Config"]["Labels"].get("com.docker.compose.project")
                        == self.project
                        and item["State"]["Running"],
                        "Cancellation target is not the owned live job",
                    )
                    self.run(D + ["stop", "--time", "5", job_name], timeout=20)
                    require(
                        job.wait(timeout=20) != 0, "Cancelled role job reported success"
                    )
                self.sql(
                    "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name='"
                    + blocker_name
                    + "' AND usename='freight'"
                )
                blocker.wait(timeout=15)
                deadline = time.monotonic() + 15
                while (
                    self.sql(
                        "SELECT count(*) FROM pg_stat_activity WHERE application_name='"
                        + job_app
                        + "'"
                    )[0]
                    != "0"
                ):
                    require(
                        time.monotonic() < deadline,
                        "Cancelled role transaction remained live",
                    )
                    time.sleep(0.1)
            require(
                before == (self.role(), self.metadata(), self.business()),
                "Cancellation changed password, grants or original rows",
            )
            self.sql(
                "SELECT current_user",
                user="freight_app",
                password=self.runtime_password,
            )
            _, error, _ = self.sql(
                "SELECT current_user",
                user="freight_app",
                password=pending_password,
                expected=None,
            )
            require(
                "password authentication failed" in error,
                "Cancelled secret refusal was not authentication failure",
            )
            self.check(
                "cancelling the actual role job at its blocked password update rolls back grants/password and retains original rows"
            )
        finally:
            if job is not None and job.poll() is None:
                item = json.loads(self.run(D + ["inspect", job_name])[0])[0]
                require(
                    item["Config"]["Labels"].get("com.docker.compose.project")
                    == self.project,
                    "Unowned job cleanup",
                )
                self.run(D + ["stop", "--time", "5", job_name], timeout=20)
                job.wait(timeout=20)
            self.sql(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name='"
                + blocker_name
                + "' AND usename='freight'"
            )
            if blocker is not None:
                blocker.wait(timeout=15)
            self.sql(
                "REVOKE UPDATE(version) ON freight_storage_format FROM freight_app"
            )

    def grants(self, *, expected=0, env=None):
        return self.compose_run(
            "run",
            "--rm",
            "--no-deps",
            "-T",
            "grants",
            expected=expected,
            env=env,
            label="owned runtime-role job",
        )

    def refused_role(self, name, fault, undo, message):
        self.sql(fault)
        before = (self.role(), self.metadata(), self.business())
        try:
            _, error, _ = self.grants(expected=None)
            require(message in error, "Role job refused for another reason")
            require(
                before == (self.role(), self.metadata(), self.business()),
                "Refused role job changed role/password/schema/business data",
            )
            self.check(name)
        finally:
            self.sql(undo)

    def exercise(self):
        for image in self.images.values():
            require(
                image.startswith("sha256:") and len(image) == 71,
                "Exact cached image ID required",
            )
            require(
                self.run(D + ["image", "inspect", image, "--format", "{{.Id}}"])[0]
                == image,
                "Image changed",
            )
        config = json.loads(self.compose_run("config", "--format", "json")[0])
        require(
            config["services"]["api"]["environment"]["PGPASSWORD"]
            in [self.runtime_password, self.runtime_password.replace("$", "$$")],
            "Runtime receives wrong password",
        )
        require(
            self.owner_password not in json.dumps(config["services"]["api"]),
            "Owner secret supplied to API",
        )
        require(
            config["services"]["api"]["environment"]["FREIGHT_MIGRATIONS"] == "verify",
            "Runtime policy changed",
        )
        self.check(
            "resolved Compose keeps owner credentials out of API and orders migration then grants before verification-only serving"
        )
        previews.verify_compose(config)
        self.check('resolved managed-role Compose selects the isolated renderer without forwarding database or TLS credentials')
        self.compose_run("up", "--detach", "--no-build", "--pull", "never", timeout=240)
        for name in ["migrate", "grants"]:
            item = self.service(name)
            require(
                not item["State"]["Running"] and item["State"]["ExitCode"] == 0,
                "One-shot schema/role job failed",
            )
        require(
            self.service("migrate")["Config"]["User"] == "10001:10001",
            "Schema job changed image user",
        )
        self.env["FREIGHT_DEMO"] = "false"
        self.compose_run(
            "up",
            "--detach",
            "--no-deps",
            "--no-build",
            "--pull",
            "never",
            "--force-recreate",
            "api",
            "web",
        )
        self.wait_ready()
        actual_environment = dict(
            item.split("=", 1) for item in self.service("api")["Config"]["Env"]
        )
        require(
            actual_environment["PGPASSWORD"] == self.runtime_password,
            "Actual API password bytes changed",
        )
        require(
            self.owner_password not in actual_environment.values(),
            "Actual API received owner secret",
        )
        require(
            self.http("/v1/workspace")["mode"] == "live",
            "Runtime retained demo finance policy",
        )
        self.check(
            "actual cached image schema and role jobs complete, then non-demo API reaches original image health with runtime login"
        )
        self.verify_host_authentication()
        require(
            self.sql(
                "SELECT rolpassword LIKE 'SCRAM-SHA-256$%' FROM pg_authid WHERE rolname='freight_app'"
            )[0]
            == "t",
            "Runtime password is not SCRAM",
        )
        require(
            self.sql(
                "SELECT current_user=session_user AND current_user='freight_app'",
                user="freight_app",
                password=self.runtime_password,
            )[0]
            == "t",
            "Actual TCP login failed",
        )
        require(
            self.sql(
                "SELECT string_agg(DISTINCT usename,',') FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()"
            )[0]
            == "freight_app",
            "API connects as another role",
        )
        self.check(
            "actual API database sessions use runtime login and real TCP authentication verifies SCRAM with literal Unicode and URL-delimiter passwords"
        )
        for label, user, password in [
            ("wrong runtime password", "freight_app", self.runtime_password + "wrong"),
            (
                "owner password against runtime login",
                "freight_app",
                self.owner_password,
            ),
            ("runtime password against owner login", "freight", self.runtime_password),
            ("missing runtime password", "freight_app", ""),
        ]:
            _, error, _ = self.sql(
                "SELECT current_user", user=user, password=password, expected=None
            )
            require(
                "password authentication failed" in error
                or "no password supplied" in error,
                "Authentication control failed for another reason",
            )
            self.check(label + " is refused by PostgreSQL host authentication")
        for statement in [
            "BEGIN; CREATE TABLE public.denied_auth(id int); ROLLBACK",
            "BEGIN; CREATE TEMP TABLE denied_auth(id int); ROLLBACK",
            "BEGIN; UPDATE freight_schema_migrations SET checksum=checksum; ROLLBACK",
            "BEGIN; UPDATE freight_storage_format SET version=version; ROLLBACK",
        ]:
            _, error, _ = self.sql(
                statement,
                user="freight_app",
                password=self.runtime_password,
                expected=None,
            )
            require("42501" in error, "DDL/metadata refusal was not permission denial")
        self.sql(
            "BEGIN; SELECT version FROM freight_storage_format WHERE singleton FOR SHARE; ROLLBACK",
            user="freight_app",
            password=self.runtime_password,
        )
        self.check(
            "authenticated runtime permits format locking but refuses schema/temp creation and history/version writes"
        )
        key = "owned-role-customer-" + uuid.uuid4().hex
        body = {
            "name": "Owned authenticated runtime customer",
            "contact": "owned-role@example.test",
        }
        original = self.http("/v1/customers", body, key)
        require(original["replayed"] is False, "Initial customer unexpectedly replayed")
        replay = self.http("/v1/customers", body, key)
        require(
            replay["data"] == original["data"] and replay["replayed"] is True,
            "Original customer replay changed",
        )
        require(
            sum(
                x["id"] == original["data"]["id"]
                for x in self.http("/v1/workspace")["customers"]
            )
            == 1,
            "Original customer was duplicated",
        )
        self.http(
            "/v1/customers",
            {**body, "contact": "changed@example.test"},
            key,
            expected=409,
        )
        self.customer_body, self.customer_key, self.customer_original = (
            body,
            key,
            original,
        )
        self.web_client = urllib.request.build_opener(
            urllib.request.ProxyHandler({}),
            NoRedirect(),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
        )
        self.http(
            "/api/session", {"token": self.token}, web=True, opener=self.web_client
        )
        require(
            self.http("/api/freight/access", web=True, opener=self.web_client)[
                "tenant_id"
            ]
            == "demo",
            "Protected web gateway failed",
        )
        self.check(
            "authenticated runtime HTTP write/read/original replay/changed-body refusal and actual web session pass"
        )
        previews.exercise(self)
        self.compose_run("stop", "web", "api")
        before = (self.metadata(), self.business())
        self.grants()
        require(
            before == (self.metadata(), self.business()),
            "Repeated role job changed original schema or business rows",
        )
        self.check(
            "repeated actual managed-role job preserves exact migration timestamps and every business table"
        )
        self.refused_role(
            "unmanaged role collision refuses without changing password or originals",
            "COMMENT ON ROLE freight_app IS NULL",
            "COMMENT ON ROLE freight_app IS 'stateset-freight managed runtime v1'",
            "not a managed Freight runtime role",
        )
        self.refused_role(
            "elevated role attributes refuse without changing password or originals",
            "ALTER ROLE freight_app INHERIT",
            "ALTER ROLE freight_app NOINHERIT",
            "unexpected attributes",
        )
        self.refused_role(
            "database-specific role settings refuse without changing password or originals",
            "ALTER ROLE freight_app IN DATABASE freight SET statement_timeout='999s'",
            "ALTER ROLE freight_app IN DATABASE freight RESET ALL",
            "unexpected attributes",
        )
        self.refused_role(
            "role memberships refuse without changing password or originals",
            "CREATE ROLE owned_auth_parent; GRANT owned_auth_parent TO freight_app",
            "REVOKE owned_auth_parent FROM freight_app; DROP ROLE owned_auth_parent",
            "memberships or owns",
        )
        self.refused_role(
            "runtime-owned objects refuse without changing password or originals",
            "CREATE TABLE public.owned_auth_object(id int); ALTER TABLE public.owned_auth_object OWNER TO freight_app",
            "ALTER TABLE public.owned_auth_object OWNER TO freight; DROP TABLE public.owned_auth_object",
            "memberships or owns",
        )
        self.refused_role(
            "PUBLIC metadata-write permission fails closed and rolls back role/password reconciliation",
            "GRANT UPDATE(version) ON freight_storage_format TO PUBLIC",
            "REVOKE UPDATE(version) ON freight_storage_format FROM PUBLIC",
            "remain elevated",
        )
        before = (self.role(), self.metadata(), self.business())
        for value in [self.owner_password, self.runtime_password + "\nsecond-line"]:
            env = {**self.env, "FREIGHT_RUNTIME_DATABASE_PASSWORD": value}
            self.grants(expected=None, env=env)
            require(
                before == (self.role(), self.metadata(), self.business()),
                "Rejected password config changed database",
            )
        self.check(
            "equal administrator/runtime secrets and newline input refuse before changing database"
        )
        self.cancelled_role_job()
        self.sql(
            "GRANT UPDATE(version) ON freight_storage_format TO freight_app; GRANT UPDATE(checksum) ON freight_schema_migrations TO freight_app"
        )
        require(
            self.sql(
                "SELECT has_column_privilege('freight_app','freight_storage_format','version','UPDATE')"
            )[0]
            == "t",
            "Stale-column negative control not installed",
        )
        before = (self.metadata(), self.business())
        self.grants()
        require(
            before == (self.metadata(), self.business()),
            "Grant reconciliation changed originals",
        )
        require(
            self.sql(
                "SELECT NOT has_column_privilege('freight_app','freight_storage_format','version','UPDATE') AND NOT has_column_privilege('freight_app','freight_schema_migrations','checksum','UPDATE')"
            )[0]
            == "t",
            "Stale metadata column grants survived reconciliation",
        )
        self.check(
            "managed-role reconciliation removes stale metadata column grants without rewriting originals"
        )
        old = self.runtime_password
        self.old_passwords.append(old)
        self.runtime_password = "Owned-rotated:" + uuid.uuid4().hex + "@/ ?#%$'\"\\雪"
        self.env["FREIGHT_RUNTIME_DATABASE_PASSWORD"] = self.runtime_password
        self.grants()
        self.sql(
            "SELECT current_user", user="freight_app", password=self.runtime_password
        )
        _, error, _ = self.sql(
            "SELECT current_user", user="freight_app", password=old, expected=None
        )
        require(
            "password authentication failed" in error,
            "Retired secret refusal was not authentication failure",
        )
        require(
            before == (self.metadata(), self.business()),
            "Password rotation changed business/schema originals",
        )
        self.check(
            "explicit role/password rotation accepts new literal secret, refuses retired secret and preserves originals"
        )
        self.compose_run(
            "up",
            "--detach",
            "--no-deps",
            "--no-build",
            "--pull",
            "never",
            "--force-recreate",
            "api",
            "web",
        )
        self.wait_ready()
        self.wait_web_access()
        replay = self.http("/v1/customers", body, key)
        require(
            replay["data"] == original["data"] and replay["replayed"] is True,
            "API original receipt changed after rotation/restart",
        )
        require(
            self.http("/api/freight/access", web=True, opener=self.web_client)[
                "tenant_id"
            ]
            == "demo",
            "Original signed web session failed after rotation/restart",
        )
        self.check(
            "recreated actual API/web use rotated runtime password and retain original receipt and signed session"
        )
        previews.retained(self)
        previews.exercise_client(self, 'after runtime password rotation')
        for name in INPUTS:
            require(
                sha(ROOT / name) == self.manifest[name]
                and sha(self.directory / name) == self.manifest[name],
                "Operational input drift",
            )
        require(fingerprint(ROOT) == self.source, "Application source drift")
        self.report["result"] = "pass"

    def cleanup(self):
        self.compose_run("logs", "--no-color", timeout=30)
        # Exact project labels are checked before this project-scoped removal.
        ids = self.run(
            D
            + [
                "ps",
                "--all",
                "--quiet",
                "--filter",
                "label=com.docker.compose.project=" + self.project,
            ]
        )[0].splitlines()
        for identity in ids:
            item = json.loads(self.run(D + ["inspect", identity])[0])[0]
            require(
                item["Config"]["Labels"].get("com.docker.compose.project")
                == self.project,
                "Unowned cleanup resource",
            )
        self.compose_run("down", "--volumes", "--remove-orphans", timeout=90)
        for resource, command in [
            ("containers", ["ps", "--all", "--quiet"]),
            ("volumes", ["volume", "ls", "--quiet"]),
            ("networks", ["network", "ls", "--quiet"]),
        ]:
            require(
                not self.run(
                    D
                    + command
                    + ["--filter", "label=com.docker.compose.project=" + self.project]
                )[0],
                "Owned " + resource + " remained",
            )
        for port in self.ports.values():
            with socket.socket() as sock:
                sock.settimeout(1)
                require(
                    sock.connect_ex(("127.0.0.1", port)) != 0,
                    "Owned port still listening",
                )
        self.report["cleanup"] = {
            "containers_absent": True,
            "volume_absent": True,
            "network_absent": True,
            "ports_closed": True,
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--verification",
        type=Path,
        default=ROOT / "docs/evidence/schema-startup-verification.json",
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument('--tui', type=Path, help='Optional built client: also require five real API/TUI PDF graphics PTY modes')
    parser.add_argument('--docker-subnet', help='Unused RFC1918 /24 through /28 for this owned Compose project; choose a range outside host/VPN routes')
    args = parser.parse_args()
    require(not args.output.exists(), "Preserve earlier reports")
    os.umask(0o077)
    run = RoleQualification(args)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        run.exercise()
    except BaseException as error:
        run.report.update(
            result="failed", error=run.safe(type(error).__name__ + ": " + str(error))
        )
        run.report["traceback"] = run.safe(traceback.format_exc())
        print(run.report["error"], file=__import__("sys").stderr, flush=True)
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
