#!/usr/bin/env python3
"""Observe a finite actual-image read burst under explicit API/PG cgroup limits.

Owned synthetic resources only. Retains unsuccessful responses and OOM evidence.
This is an overload diagnostic, not sustained capacity or a production SLO.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import signal
import subprocess
import threading
import time
import traceback
import urllib.error
import urllib.parse
import urllib.request

from runtime_contracts import ROOT, Runtime, fingerprint, literal, require
from migration_ledger import verify_migration_ledger
from qualification_artifacts import read_artifact, write_artifact
from qualification_json import OriginalJsonWorker, validate_plan as validate_json_plan

spec = importlib.util.spec_from_file_location(
    "envelope_exact", ROOT / "scripts/profile-storage-records.py"
)
exact = importlib.util.module_from_spec(spec)
spec.loader.exec_module(exact)


def sha(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def read(path):
    return json.loads(Path(path).read_text())


def verify_codec_binary(path, expected_sha256):
    path = Path(path)
    require(path.is_file() and os.access(path, os.X_OK),
            "Codec must be an executable regular file before starting resources")
    require(sha(path) == expected_sha256, "Codec build binding mismatch")


def allocator_environment(value):
    if value is None:
        return {}
    require(
        type(value) is int and value in [1, 2, 4],
        "Unsupported allocator arena experiment",
    )
    return {"MALLOC_ARENA_MAX": str(value)}


def parse_cgroup(text, memory_mib):
    """Keep actual versioned counters; absent evidence must never become zero."""
    sections = {}
    for section in text.split("@"):
        if section:
            key, separator, data = section.partition("\n")
            require(separator and key not in sections, "Invalid cgroup counter framing")
            sections[key] = data.strip()

    def integer(key):
        value = sections.get(key, "")
        require(value.isdigit(), "Missing numeric cgroup counter: " + key)
        return int(value)

    def counters(key):
        value = sections.get(key, "unavailable")
        require(value != "unavailable", "Missing cgroup counter group: " + key)
        rows = [line.split() for line in value.splitlines()]
        require(
            rows and all(len(row) == 2 and row[1].isdigit() for row in rows),
            "Invalid cgroup counters",
        )
        require(len({row[0] for row in rows}) == len(rows), "Duplicate cgroup counter")
        return {key: int(value) for key, value in rows}

    if sections.get("memory.events", "unavailable") != "unavailable":
        version = 2
        events = counters("memory.events")
        require(
            all(key in events for key in ["oom", "oom_kill", "max"]),
            "Missing cgroup2 OOM counters",
        )
        current, limit, swap = (
            integer(key) for key in ["memory.current", "memory.max", "memory.swap.max"]
        )
        peak = (
            integer("memory.peak")
            if sections.get("memory.peak", "unavailable") != "unavailable"
            else None
        )
        quota, period = map(int, sections["cpu.max"].split())
        cpu = counters("cpu.stat")
        require("usage_usec" in cpu, "Missing cgroup2 CPU accounting")
        events["limit_hits"] = events["max"]
    else:
        version = 1
        oom = counters("memory/memory.oom_control")
        require(
            all(key in oom for key in ["under_oom", "oom_kill"]),
            "Missing cgroup1 OOM counters",
        )
        current = integer("memory/memory.usage_in_bytes")
        peak = integer("memory/memory.max_usage_in_bytes")
        limit = integer("memory/memory.limit_in_bytes")
        swap = integer("memory/memory.memsw.limit_in_bytes") - limit
        events = {**oom, "limit_hits": integer("memory/memory.failcnt")}
        quota, period = (
            integer(key) for key in ["cpu/cpu.cfs_quota_us", "cpu/cpu.cfs_period_us"]
        )
        cpu = {**counters("cpu/cpu.stat"), "usage_ns": integer("cpuacct/cpuacct.usage")}
    require(
        limit == memory_mib * 2**20 and swap == 0,
        "Kernel memory/swap limits differ from plan",
    )
    require(period > 0 and quota == period * 2, "Kernel CPU quota differs from plan")
    return {
        "cgroup_version": version,
        "memory_current": current,
        "memory_peak": peak,
        "memory_max": limit,
        "swap_max": swap,
        "cpu_max": {"quota": quota, "period": period},
        "memory_events": events,
        "cpu_stat": cpu,
    }


def require_no_memory_faults(item):
    events = item["memory_events"]
    require(
        events["oom_kill"] == 0 and events["limit_hits"] == 0,
        "Cgroup memory limit/OOM kill observed",
    )
    if item["cgroup_version"] == 2:
        require(events["oom"] == 0, "Cgroup2 OOM pressure observed")
    else:
        require(events["under_oom"] == 0, "Cgroup1 OOM pressure observed")


class Envelope(Runtime):
    def __init__(self, args):
        super().__init__(args)
        self.api = self.migration = self.volume = None
        self.backed_up = False
        self.observer_stop = threading.Event()
        self.observer = None
        self.observer_ready = threading.Event()
        self.samples = []
        self.tokens = {name: secrets.token_urlsafe(36) for name in ["large", "probe"]}
        self.secrets += list(self.tokens.values())
        self.binding = read(args.api_binding)
        self.source = fingerprint(ROOT)
        require(
            self.binding["source"] == self.source,
            "Image binding differs from current application source",
        )
        self.image = self.binding["image"]
        self.plan = read(args.plan)
        require(
            self.plan["source"] == self.source and self.plan["image"] == self.image,
            "Plan source/image mismatch",
        )
        require(
            self.plan["api"]["memory_mib"] == args.api_memory_mib,
            "Select a matching new plan for a changed envelope",
        )
        require(
            self.plan["api"]["read_concurrency"] == args.read_concurrency,
            "Read concurrency differs from the supplied plan",
        )
        require(
            self.plan["workload"]["fixtures"] == [str(args.large), str(args.probe)],
            "Plan fixture paths differ",
        )
        self.evidence_compression = self.plan.get("evidence_compression")
        require(self.evidence_compression in (None, "gzip"), "Unsupported evidence compression")
        self.client_json_plan = validate_json_plan(self.plan.get("client_json_processing"))
        self.original_json = None
        self.pg_image = self.plan["postgres_image"]
        self.workspace_jobs = self.plan["api"].get("workspace_concurrency")
        require(
            self.workspace_jobs is None
            or (type(self.workspace_jobs) is int and 1 <= self.workspace_jobs <= 64),
            "Invalid workspace processing plan",
        )
        self.allocator_environment = allocator_environment(
            self.plan["api"].get("malloc_arena_max")
        )
        self.codec = read(args.codec_binding)
        verify_codec_binary(args.codec, self.codec["binary"]["sha256"])
        require(
            self.codec["source"] == self.source,
            "Codec build binding mismatch",
        )
        self.role = "envelope_" + self.container_label.replace("-", "")
        self.report.update(
            scope=__doc__,
            source=self.source,
            ownership_label=self.container_label,
            ports={k: self.ports[k] for k in ["postgres", "freight"]},
            images={"freight": self.image, "postgres": self.pg_image},
            api_binding={
                "path": str(args.api_binding),
                "sha256": sha(args.api_binding),
            },
            plan={"path": str(args.plan), "sha256": sha(args.plan)},
            codec_binding={
                "path": str(args.codec_binding),
                "sha256": sha(args.codec_binding),
            },
            harness={
                str(p.relative_to(ROOT)): sha(p)
                for p in [
                    Path(__file__).resolve(),
                    ROOT / "scripts/runtime_contracts.py",
                    ROOT / "scripts/profile-storage-records.py",
                    ROOT / "scripts/migration_ledger.py",
                    ROOT / "scripts/qualification_artifacts.py",
                    ROOT / "scripts/qualification_json.py",
                ]
            },
            observer="Actual cgroup1/2 counters read with a short nonprivileged docker-exec process about every0.5seconds; observer and default image healthchecks consume the same cgroup budgets",
            resource_samples=self.samples,
            requests=[],
            capacity_slo="not established",
            writes_started=False,
        )
        self.report.pop("callback_transport", None)

    def encode_original(self, value):
        worker = getattr(self, "original_json", None)
        return worker.encode(value) if worker is not None else exact.encode(value)

    def decode_original(self, raw):
        worker = getattr(self, "original_json", None)
        return worker.decode(raw) if worker is not None else exact.decode(raw)

    def read_original_document(self, raw):
        worker = getattr(self, "original_json", None)
        if worker is not None:
            return worker.original(raw)
        value = exact.decode(raw)
        return value, exact.encode(value)

    def own(self, identity, kind="container"):
        item = json.loads(
            self.command(
                ["docker", kind, "inspect", identity], label="owned resource inspection"
            )
        )[0]
        labels = item["Config"]["Labels"] if kind == "container" else item["Labels"]
        require(
            labels.get("com.stateset.freight.runtime") == self.container_label,
            "Refusing unowned resource",
        )
        return item

    def seed_fixtures(self):
        return [("large", self.args.large), ("probe", self.args.probe)]

    def setup(self):
        # All derived constructor/plan checks finish before a child is started;
        # main's finally always closes it, even if image/setup/backup fails.
        if self.client_json_plan is not None:
            self.original_json = OriginalJsonWorker(
                maximum=self.client_json_plan["max_frame_bytes"],
                timeout=self.client_json_plan["timeout_seconds"])
        self.report["client_json_processing"] = (
            self.original_json.status() if self.original_json is not None
            else {"mode": "native", "workers": 0})
        for image in [self.image, self.pg_image]:
            require(
                self.command(
                    ["docker", "image", "inspect", image, "--format", "{{.Id}}"],
                    label="exact image",
                )
                == image,
                "Image changed",
            )
        self.volume = "freight-envelope-" + self.container_label
        self.command(
            [
                "docker",
                "volume",
                "create",
                "--label",
                "com.stateset.freight.runtime=" + self.container_label,
                self.volume,
            ],
            label="owned disk volume",
        )
        self.container = self.command(
            [
                "docker",
                "run",
                "-d",
                "--pull=never",
                "--name",
                "freight-envelope-pg-" + self.container_label,
                "--label",
                "com.stateset.freight.runtime=" + self.container_label,
                "--cpus=2",
                "--memory=768m",
                "--memory-swap=768m",
                "--mount",
                "type=volume,source="
                + self.volume
                + ",target=/var/lib/postgresql/data,volume-nocopy",
                "-p",
                f"127.0.0.1:{self.ports['postgres']}:5432",
                "-p",
                f"127.0.0.1:{self.ports['freight']}:8090",
                "-e",
                "POSTGRES_USER=freight_runtime",
                "-e",
                "POSTGRES_DB=freight_runtime",
                "-e",
                "POSTGRES_HOST_AUTH_METHOD=trust",
                self.pg_image,
            ],
            label="owned capped PostgreSQL",
        )
        deadline = time.monotonic() + 60
        while True:
            try:
                if (
                    self.sql("freight_runtime", "SELECT current_user")
                    == "freight_runtime"
                ):
                    break
            except RuntimeError:
                pass
            require(time.monotonic() < deadline, "PostgreSQL readiness deadline")
            time.sleep(0.2)
        self.migration = self.command(
            [
                "docker",
                "run",
                "-d",
                "--pull=never",
                "--label",
                "com.stateset.freight.runtime=" + self.container_label,
                "--network",
                "container:" + self.container,
                "--memory=512m",
                "--memory-swap=512m",
                "--cpus=2",
                "-e",
                "DATABASE_URL=postgres://freight_runtime@127.0.0.1:5432/freight_runtime",
                "-e",
                "FREIGHT_DEMO=false",
                self.image,
                "--migrate",
            ],
            label="actual schema-only image",
        )
        require(
            self.command(
                ["docker", "wait", self.migration],
                label="schema completion",
                timeout=120,
            )
            == "0",
            "Schema job failed",
        )
        require(
            self.own(self.migration)["Config"]["User"] == "10001:10001",
            "Schema job changed image user",
        )
        require(
            self.sql("freight_runtime", "SELECT version FROM freight_storage_format")
            == "21",
            "Unsupported storage format",
        )
        installed = self.rows("freight_runtime", "SELECT version,checksum,applied_at FROM freight_schema_migrations ORDER BY version")
        verify_migration_ledger(ROOT, installed)
        self.report["installed_migrations"] = installed
        require(
            self.sql("freight_runtime", "SELECT count(*) FROM freight_workspaces")
            == "0",
            "Schema job seeded data",
        )
        tables = self.rows(
            "freight_runtime",
            "SELECT tablename FROM pg_tables WHERE schemaname='public' AND tablename NOT IN ('freight_schema_migrations','freight_storage_format')",
        )
        statements = [
            f"CREATE ROLE {self.role} LOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS",
            "REVOKE ALL ON DATABASE freight_runtime FROM PUBLIC",
            f"GRANT CONNECT ON DATABASE freight_runtime TO {self.role}",
            "REVOKE CREATE ON SCHEMA public FROM PUBLIC",
            f"GRANT USAGE ON SCHEMA public TO {self.role}",
            f"GRANT SELECT ON ALL TABLES IN SCHEMA public TO {self.role}",
            f"GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA public TO {self.role}",
            f"GRANT UPDATE(singleton) ON freight_storage_format TO {self.role}",
        ]
        for row in tables:
            name = row["tablename"]
            require(name.replace("_", "").isalnum(), "Unexpected table identifier")
            statements.append(f"GRANT INSERT,UPDATE,DELETE ON {name} TO {self.role}")
        self.sql("freight_runtime", ";".join(statements))
        self.expected = {}
        for tenant, path in self.seed_fixtures():
            value = exact.decode(path.read_bytes())
            value["_freight_format"] = 21
            source = exact.encode(value).encode()
            frame = exact.decode(
                subprocess.check_output(
                    [str(self.args.codec), "pack", "64", "65536"], input=source
                )
            )
            blocks = frame.pop("blocks")
            frame["blocks"] = []
            header = exact.encode({"_freight_format": 21, "frame": frame})
            sql = [
                "BEGIN",
                "INSERT INTO freight_workspaces(tenant_id,data) VALUES("
                + literal(tenant)
                + ","
                + literal(header)
                + "::jsonb)",
            ]
            for block in blocks:
                sql.append(
                    "INSERT INTO freight_workspace_blocks(tenant_id,root,key,payload) VALUES("
                    + ",".join(
                        literal(x)
                        for x in [
                            tenant,
                            block["root"],
                            block["key"],
                            exact.encode(block),
                        ]
                    )
                    + "::jsonb)"
                )
            sql += [
                "UPDATE freight_workspaces SET data="
                + literal(header)
                + "::jsonb WHERE tenant_id="
                + literal(tenant),
                "COMMIT",
            ]
            self.command(
                [
                    "psql",
                    f"postgres://freight_runtime@127.0.0.1:{self.ports['postgres']}/freight_runtime",
                    "-X",
                    "-q",
                    "-v",
                    "ON_ERROR_STOP=1",
                ],
                label="owned exact canonical seed",
                input=";\n".join(sql) + ";",
                env={"PATH": os.defpath},
            )
            restored = exact.decode(
                self.sql(
                    "freight_runtime",
                    "SELECT freight_workspace_originals(" + literal(tenant) + ")::text",
                ).encode()
            )
            require(
                exact.encode(restored) == exact.encode(value),
                "Seed changed exact originals",
            )
            self.expected[tenant] = {
                "loads": len(value["loads"]),
                "id": value["loads"][0]["id"],
                "source_sha256": hashlib.sha256(
                    exact.encode(value).encode()
                ).hexdigest(),
                "input": {"path": str(path), "sha256": sha(path)},
            }
        self.report["fixtures"] = self.expected
        environment = {
            "DATABASE_URL": f"postgres://{self.role}@127.0.0.1:5432/freight_runtime",
            "FREIGHT_MIGRATIONS": "verify",
            "FREIGHT_DEMO": "false",
            "FREIGHT_CLERK_JSON": "",
            "FREIGHT_INTEGRATIONS_JSON": "{}",
            "FREIGHT_BIND_ADDR": "0.0.0.0:8090",
            "TOKIO_WORKER_THREADS": "2",
            "FREIGHT_READ_CONCURRENCY": str(self.args.read_concurrency),
            "FREIGHT_DATABASE_MAX_CONNECTIONS": "2",
            "RUST_LOG": "error",
            "FREIGHT_API_KEYS_JSON": json.dumps(
                {
                    token: {
                        "tenant_id": tenant,
                        "subject": "envelope:operator",
                        "role": "admin",
                    }
                    for tenant, token in self.tokens.items()
                }
            ),
        }
        environment.update(self.allocator_environment)
        if self.workspace_jobs is not None:
            environment["FREIGHT_WORKSPACE_CONCURRENCY"] = str(self.workspace_jobs)
        limit = f"{self.args.api_memory_mib}m"
        command = [
            "docker",
            "run",
            "-d",
            "--pull=never",
            "--name",
            "freight-envelope-api-" + self.container_label,
            "--label",
            "com.stateset.freight.runtime=" + self.container_label,
            "--network",
            "container:" + self.container,
            "--cpus=2",
            "--memory=" + limit,
            "--memory-swap=" + limit,
        ]
        for key, value in environment.items():
            command += ["-e", key + "=" + value]
        self.api = self.command(command + [self.image], label="actual capped API image")
        api_config = self.own(self.api)["Config"]
        require(
            api_config["User"] == "10001:10001",
            "API changed image user",
        )
        actual_env = dict(value.split("=", 1) for value in api_config["Env"])
        if self.workspace_jobs is not None:
            require(
                actual_env.get("FREIGHT_WORKSPACE_CONCURRENCY")
                == str(self.workspace_jobs),
                "Workspace processing configuration differs from plan",
            )
            self.report["workspace_admission"] = {
                "configured_jobs": self.workspace_jobs,
                "effective_environment": {
                    "FREIGHT_WORKSPACE_CONCURRENCY": actual_env[
                        "FREIGHT_WORKSPACE_CONCURRENCY"
                    ]
                },
                "scope": "Admitted materialization/preparation jobs only; not a total RAM bound",
            }
        for key, value in self.allocator_environment.items():
            require(
                actual_env.get(key) == value, "Allocator environment differs from plan"
            )
        if self.allocator_environment:
            require(
                not actual_env.get("GLIBC_TUNABLES"),
                "Competing allocator tunables invalidate experiment",
            )
            self.report["allocator_experiment"] = {
                "environment": self.allocator_environment,
                "actual_libc": self.command(
                    ["docker", "exec", self.api, "getconf", "GNU_LIBC_VERSION"],
                    label="actual image libc identity",
                ),
                "scope": "Finite experimental environment only; no image or application/default change",
            }
        for key in [
            "DATABASE_URL",
            "FREIGHT_MIGRATIONS",
            "TOKIO_WORKER_THREADS",
            "FREIGHT_READ_CONCURRENCY",
            "FREIGHT_DATABASE_MAX_CONNECTIONS",
        ]:
            require(
                actual_env[key] == environment[key],
                "Actual API configuration differs: " + key,
            )
        self.report["runtime_role"] = self.role
        self.report["api_settings"] = {
            key: actual_env[key]
            for key in [
                "FREIGHT_MIGRATIONS",
                "TOKIO_WORKER_THREADS",
                "FREIGHT_READ_CONCURRENCY",
                "FREIGHT_DATABASE_MAX_CONNECTIONS",
            ]
        }
        self.report["actual_resources"] = {}
        for service, identity, mib in [
            ("freight", self.api, self.args.api_memory_mib),
            ("postgres", self.container, 768),
        ]:
            config = self.own(identity)["HostConfig"]
            require(
                config["Memory"] == config["MemorySwap"] == mib * 2**20
                and config["NanoCpus"] == 2_000_000_000,
                "Actual quota differs from plan",
            )
            self.report["actual_resources"][service] = {
                key: config[key]
                for key in ["Memory", "MemorySwap", "NanoCpus", "NetworkMode"]
            }
        deadline = time.monotonic() + 60
        while True:
            try:
                with self.opener.open(
                    self.urls["freight"] + "/ready", timeout=1
                ) as response:
                    if response.status == 200:
                        break
            except (OSError, TimeoutError):
                pass
            require(
                self.own(self.api)["State"]["Running"] and time.monotonic() < deadline,
                "API readiness failed",
            )
            time.sleep(0.1)
        self.observer = threading.Thread(target=self.observe, daemon=True)
        role = self.rows(
            "freight_runtime",
            "SELECT rolname,rolsuper,rolcreatedb,rolcreaterole,rolreplication,rolbypassrls,rolinherit FROM pg_roles WHERE rolname="
            + literal(self.role),
        )
        connections = self.rows(
            "freight_runtime",
            "SELECT usename,count(*) AS connections FROM pg_stat_activity WHERE datname=current_database() AND usename="
            + literal(self.role)
            + " GROUP BY usename",
        )
        require(
            len(role) == 1
            and all(
                role[0][key] is False
                for key in [
                    "rolsuper",
                    "rolcreatedb",
                    "rolcreaterole",
                    "rolreplication",
                    "rolbypassrls",
                    "rolinherit",
                ]
            ),
            "Runtime role attributes changed",
        )
        require(
            connections
            and connections[0]["usename"] == self.role
            and connections[0]["connections"] > 0,
            "Actual restricted API login was not observed",
        )
        self.report["actual_runtime_login"] = {
            "role": role[0],
            "connections": connections,
        }
        self.observer.start()
        require(self.observer_ready.wait(15), "Initial resource sample did not finish")
        self.healthy()
        self.step(
            "exact actual images, restricted serving login, synthetic original frames and cgroup limits verified"
        )

    def observe(self):
        script = 'for envelope_file in memory.current memory.peak memory.events cpu.stat memory.max memory.swap.max cpu.max memory/memory.usage_in_bytes memory/memory.max_usage_in_bytes memory/memory.oom_control memory/memory.failcnt memory/memory.limit_in_bytes memory/memory.memsw.limit_in_bytes cpu/cpu.stat cpu/cpu.cfs_quota_us cpu/cpu.cfs_period_us cpuacct/cpuacct.usage; do printf "@%s\\n" "$envelope_file"; if [ -r "/sys/fs/cgroup/$envelope_file" ]; then cat "/sys/fs/cgroup/$envelope_file"; else printf "unavailable\\n"; fi; done'
        while not self.observer_stop.is_set():
            sample = {"at": time.monotonic(), "services": {}}
            for service, identity in [
                ("freight", self.api),
                ("postgres", self.container),
            ]:
                try:
                    state = self.own(identity)["State"]
                    item = {
                        "running": state["Running"],
                        "oom_killed": state["OOMKilled"],
                        "exit_code": state["ExitCode"],
                    }
                    if state["Running"]:
                        text = self.command(
                            ["docker", "exec", identity, "sh", "-c", script],
                            label="owned cgroup counters",
                            timeout=10,
                        )
                        if not self.samples:
                            with (
                                self.directory / ("cgroup-initial-" + service + ".txt")
                            ).open("x") as out:
                                out.write(text + "\n")
                        expected_mib = (
                            self.args.api_memory_mib if service == "freight" else 768
                        )
                        item.update(parse_cgroup(text, expected_mib))
                    sample["services"][service] = item
                except Exception as error:
                    sample["services"][service] = {
                        "observer_error": self.safe(str(error))
                    }
            self.samples.append(sample)
            self.observer_ready.set()
            self.observer_stop.wait(0.5)

    def request(self, tenant, path, name, barrier=None, *, body=None, key=None):
        if barrier:
            barrier.wait(timeout=15)
        started = time.monotonic()
        output = self.directory / (name + ".json")
        created_output = False
        result = {
            "name": name,
            "tenant": tenant,
            "path": path,
            "started": started,
            "status": 0,
            "bytes": 0,
            "method": "POST" if body is not None else "GET",
        }
        try:
            headers = {"Authorization": "Bearer " + self.tokens[tenant]}
            data = None
            if body is not None:
                data = json.dumps(body, separators=(",", ":")).encode()
                headers["Content-Type"] = "application/json"
                result["request_body_sha256"] = hashlib.sha256(data).hexdigest()
            if key is not None:
                headers["Idempotency-Key"] = key
            request = urllib.request.Request(
                self.urls["freight"] + path,
                headers=headers,
                data=data,
            )
            try:
                response = self.opener.open(request, timeout=60)
            except urllib.error.HTTPError as error:
                response = error
            with response, output.open("xb") as out:
                created_output = True
                result.update(
                    status=response.status,
                    retry_after=response.headers.get("Retry-After"),
                    cache_control=response.headers.get("Cache-Control"),
                )
                while True:
                    chunk = response.read(1024 * 1024)
                    if not chunk:
                        break
                    out.write(chunk)
                    result["bytes"] += len(chunk)
        except Exception as error:
            result["error"] = type(error).__name__ + ": " + self.safe(str(error))
        result["wall_ms"] = (time.monotonic() - started) * 1000
        if created_output and output.exists():
            if self.evidence_compression:
                result["response"] = write_artifact(output, output.read_bytes(), self.evidence_compression)
                output.unlink()  # Only this request's exclusively created temporary body.
            else:
                result["response"] = {"path": str(output), "sha256": sha(output)}
        return result

    def stable(self, result):
        value = exact.decode(read_artifact(result["response"]))
        if "operations" in value:
            value["operations"].pop("generated_at", None)
        require(
            len(value["loads"]) == self.expected[result["tenant"]]["loads"],
            "Incomplete or cross-tenant workspace",
        )
        return hashlib.sha256(exact.encode(value).encode()).hexdigest()

    def physical(self):
        return self.sql(
            "freight_runtime",
            "SELECT jsonb_build_object('headers',(SELECT jsonb_agg(jsonb_build_array(tenant_id,xmin::text,ctid::text,md5(data::text)) ORDER BY tenant_id) FROM freight_workspaces),'blocks',(SELECT jsonb_agg(jsonb_build_array(tenant_id,root,key,xmin::text,ctid::text,md5(payload::text)) ORDER BY tenant_id,root,key) FROM freight_workspace_blocks))::text",
        )

    def healthy(self):
        require(
            all(
                self.own(identity)["State"]["Running"]
                and not self.own(identity)["State"]["OOMKilled"]
                for identity in [self.api, self.container]
            ),
            "API/PostgreSQL exited or was OOM killed",
        )
        require(bool(self.samples), "No resource samples")
        for sample in self.samples:
            for item in sample["services"].values():
                require(
                    "observer_error" not in item,
                    "Resource observation failed; no success claim",
                )
                require(
                    not item.get("oom_killed") and item.get("running"),
                    "Observed service exit/OOM",
                )
                require_no_memory_faults(item)

    def checks(self):
        stable = {}
        for tenant in ["large", "probe"]:
            result = self.request(tenant, "/v1/workspace", "warm-" + tenant)
            self.report["requests"].append(result)
            require(
                result["status"] == 200 and "error" not in result,
                "Warm workspace failed; preserve failure",
            )
            stable[tenant] = self.stable(result)
        catalog_path = "/v1/loads?limit=1&q=" + urllib.parse.quote(
            self.expected["probe"]["id"]
        )
        warm = self.request("probe", catalog_path, "warm-catalog")
        self.report["requests"].append(warm)
        require(warm["status"] == 200, "Warm small catalog failed")
        catalog = exact.decode(read_artifact(warm["response"]))
        self.report["warm_catalog_normalized_sha256"] = hashlib.sha256(
            exact.encode(catalog).encode()
        ).hexdigest()
        before = self.physical()
        self.report["physical_before_sha256"] = hashlib.sha256(
            before.encode()
        ).hexdigest()
        time.sleep(35)
        self.healthy()
        require(
            self.physical() == before, "Idle workers changed original physical rows"
        )
        self.step(
            "warm exact business workspaces and35-second idle retain original physical rows without OOM"
        )
        specs = [("large", "/v1/workspace", f"burst-large-{n}") for n in range(8)] + [
            ("probe", catalog_path, "burst-probe"),
            ("probe", "/health", "burst-health"),
            ("probe", "/ready", "burst-ready"),
        ]
        barrier = threading.Barrier(len(specs) + 1)
        with ThreadPoolExecutor(max_workers=len(specs)) as pool:
            futures = [pool.submit(self.request, *item, barrier) for item in specs]
            barrier.wait(timeout=15)
            results = [future.result() for future in futures]
        self.report["requests"].extend(results)
        self.report["burst_completed"] = True
        time.sleep(35)
        self.report["physical_after_sha256"] = hashlib.sha256(
            self.physical().encode()
        ).hexdigest()
        require(self.physical() == before, "Read burst changed original physical rows")
        self.healthy()
        accepted = 0
        for result in results:
            require("error" not in result, "Request transport failed")
            if result["name"].startswith("burst-large-"):
                if result["status"] == 200:
                    require(
                        self.stable(result) == stable["large"],
                        "Accepted business output changed",
                    )
                    accepted += 1
                else:
                    body = json.loads(read_artifact(result["response"]))
                    require(
                        result["status"] == 503
                        and result["retry_after"] == "1"
                        and result["cache_control"] == "no-store"
                        and body["error"]["code"]
                        in [
                            "read_capacity_exceeded",
                            "workspace_capacity_exceeded",
                            "database_temporarily_unavailable",
                        ],
                        "Unexpected rejection",
                    )
            else:
                require(result["status"] == 200, "Probe/liveness/readiness failed")
                if result["name"] == "burst-probe":
                    require(
                        exact.decode(read_artifact(result["response"]))
                        == catalog,
                        "Small-tenant catalog changed",
                    )
        require(accepted > 0, "No useful large read completed")
        self.report["accepted_large_reads"] = accepted
        self.step(
            "finite simultaneous read burst retains accepted originals, explicit overload refusals, tenant probe and liveness without OOM"
        )

    def verify_sources_after(self):
        for tenant, expected in getattr(self, "expected", {}).items():
            value = exact.decode(
                self.sql(
                    "freight_runtime",
                    "SELECT freight_workspace_originals(" + literal(tenant) + ")::text",
                ).encode()
            )
            require(
                hashlib.sha256(exact.encode(value).encode()).hexdigest()
                == expected["source_sha256"],
                "Original business source changed during read-only diagnostic",
            )
        self.report["source_originals_unchanged_after"] = True

    def finish(self):
        try:
            self._finish_resources()
        finally:
            worker = getattr(self, "original_json", None)
            if worker is not None:
                try:
                    worker.close()
                finally:
                    self.report["client_json_processing"] = worker.status()

    def _finish_resources(self):
        self.observer_stop.set()
        if self.observer:
            self.observer.join(25)
            require(not self.observer.is_alive(), "Resource observer did not stop")
        self.report["terminal_states"] = {}
        for name, identity in [
            ("freight", self.api),
            ("schema", self.migration),
            ("postgres", self.container),
        ]:
            if identity:
                self.report["terminal_states"][name] = self.own(identity)["State"]
                completed = subprocess.run(
                    [
                        "docker",
                        "--host",
                        "unix:///var/run/docker.sock",
                        "logs",
                        identity,
                    ],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    env={"PATH": os.defpath},
                )
                require(completed.returncode == 0, "Owned logs unavailable")
                with (self.directory / (name + ".log")).open("xb") as out:
                    out.write(completed.stdout)
        if self.container and self.own(self.container)["State"]["Running"]:
            self.verify_sources_after()
            path = self.directory / "database.dump"
            with path.open("xb") as out:
                result = subprocess.run(
                    [
                        "docker",
                        "--host",
                        "unix:///var/run/docker.sock",
                        "exec",
                        self.container,
                        "pg_dump",
                        "-U",
                        "freight_runtime",
                        "-d",
                        "freight_runtime",
                        "--format=custom",
                        "--no-owner",
                        "--no-privileges",
                    ],
                    stdout=out,
                    stderr=subprocess.PIPE,
                )
            require(
                result.returncode == 0 and path.read_bytes()[:5] == b"PGDMP",
                "Fixture backup failed; retain owned volume",
            )
            result = subprocess.run(
                [
                    "docker",
                    "--host",
                    "unix:///var/run/docker.sock",
                    "exec",
                    "-i",
                    self.container,
                    "pg_restore",
                    "--list",
                ],
                input=path.read_bytes(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            require(
                result.returncode == 0,
                "Fixture backup listing failed; retain owned volume",
            )
            with (self.directory / "database-toc.txt").open("xb") as out:
                out.write(result.stdout)
            self.report["database_archive"] = {
                "path": str(path),
                "sha256": sha(path),
                "bytes": path.stat().st_size,
                "toc_sha256": sha(self.directory / "database-toc.txt"),
                "scope": "Checked logical fixture archive, not a restore qualification",
            }
            self.backed_up = True
        for identity in [self.api, self.migration, self.container]:
            if identity:
                item = self.own(identity)
                if item["State"]["Running"]:
                    self.command(
                        ["docker", "stop", "--time", "15", identity], label="owned stop"
                    )
                self.own(identity)
                self.command(
                    ["docker", "rm", "--volumes", identity],
                    label="owned container cleanup",
                )
        if self.volume:
            self.own(self.volume, "volume")
            require(self.backed_up, "Retaining fixture volume without a checked backup")
            self.command(
                ["docker", "volume", "rm", self.volume],
                label="owned fixture volume cleanup",
            )
        self.report["cleanup"] = "owned containers and backed-up fixture volume removed"


def main(envelope_type=Envelope):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in [
        "plan",
        "api-binding",
        "codec",
        "codec-binding",
        "large",
        "probe",
        "output",
    ]:
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument(
        "--api-memory-mib", type=int, choices=[512, 768, 1024, 2048], default=512
    )
    parser.add_argument("--read-concurrency", type=int, choices=[1, 2], default=2)
    args = parser.parse_args()
    for name in [
        "plan",
        "api_binding",
        "codec",
        "codec_binding",
        "large",
        "probe",
        "output",
    ]:
        setattr(args, name, getattr(args, name).resolve())
    require(not args.output.exists(), "Preserve reports; choose a new output")
    os.umask(0o077)
    run = envelope_type(args)

    def interrupted(_signal, _frame):
        raise KeyboardInterrupt("Owned resource-envelope run interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    try:
        run.setup()
        run.checks()
        run.report["result"] = "pass_overload_contracts_only"
    except BaseException as error:
        run.report.update(
            result="failed",
            error=run.safe(type(error).__name__ + ": " + str(error)),
            traceback=run.safe(traceback.format_exc()),
        )
        print(run.report["error"], flush=True)
    finally:
        try:
            run.finish()
        except BaseException as error:
            run.report.update(
                result="failed",
                cleanup_error=run.safe(type(error).__name__ + ": " + str(error)),
            )
        if fingerprint(ROOT) != run.source:
            run.report.update(result="failed", source_drift=True)
        for name, digest in run.report["harness"].items():
            if sha(ROOT / name) != digest:
                run.report.update(result="failed", harness_drift=name)
        for expected in getattr(run, "expected", {}).values():
            if sha(expected["input"]["path"]) != expected["input"]["sha256"]:
                run.report.update(result="failed", fixture_drift=True)
        run.report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        run.save()
        with args.output.open("x") as out:
            out.write((run.directory / "report.json").read_text())
        print("REPORT", args.output, flush=True)
    return 0 if run.report["result"] == "pass_overload_contracts_only" else 1


if __name__ == "__main__":
    raise SystemExit(main())
