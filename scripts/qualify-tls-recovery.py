#!/usr/bin/env python3
"""Complete owned image recovery with SCRAM and certificate-verified database TCP.

Retains every restricted-recovery assertion. Generates synthetic PKI and login
secrets; never accepts deployment database connections or operator credentials.
The controller retains those secrets across both genuinely separate clusters.
"""

import argparse
import base64
import datetime as dt
import hashlib
import hmac
import importlib.util
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import subprocess
import sys
import traceback
from urllib.parse import urlsplit

from runtime_contracts import ROOT, fingerprint, literal, require


def module(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


base = module("tls_recovery_restricted", "scripts/qualify-restricted-recovery.py")
fixtures = module("tls_recovery_pki", "scripts/test_postgres_tls.py")
CA = "/run/freight-database-ca.crt"
MATERIAL = "/run/freight-database-tls"
EXTRA_INPUTS = [
    "scripts/migration_ledger.py",
    "scripts/recovery-check-inventory.json",
    "scripts/qualify-tls-recovery.py",
    "scripts/test_postgres_tls.py",
    "scripts/install-postgres-tls.sh",
    "scripts/validate-postgres-tls.sh",
    "scripts/postgres-tls.pg_hba.conf",
]


def verifier(password):
    """PostgreSQL SCRAM verifier, so CREATE ROLE never carries a cleartext secret."""
    salt = secrets.token_bytes(16)
    salted = hashlib.pbkdf2_hmac("sha256", password.encode(), salt, 4096)
    client = hmac.digest(salted, b"Client Key", "sha256")
    stored = hashlib.sha256(client).digest()
    server = hmac.digest(salted, b"Server Key", "sha256")
    encoded = lambda value: base64.b64encode(value).decode()
    return f"SCRAM-SHA-256$4096:{encoded(salt)}${encoded(stored)}:{encoded(server)}"


class TLSRecovery(base.RestrictedRecovery):
    def __init__(self, args):
        super().__init__(args)
        self.required_checks = base.required_recovery_checks()
        self.report["required_inherited_checks"] = sum(self.required_checks.values())
        self.tls_volume = None
        self.pki_ready = False
        self.owner_password = secrets.token_urlsafe(36)
        self.runtime_password = secrets.token_urlsafe(36)
        self.runtime_verifier = verifier(self.runtime_password)
        self.secrets += [
            self.owner_password,
            self.runtime_password,
            self.runtime_verifier,
        ]
        self.private = self.directory / "private"
        self.private.mkdir(mode=0o700)
        for name, password in [
            ("owner", self.owner_password),
            ("runtime", self.runtime_password),
        ]:
            (self.private / (name + ".env")).write_text(
                f"PGPASSWORD={password}\nPGHOST=127.0.0.1\nPGPORT=5432\n"
                f"PGSSLMODE=verify-full\nPGSSLROOTCERT={CA}\n"
            )
        (self.private / "bootstrap.env").write_text(
            "POSTGRES_PASSWORD=" + self.owner_password + "\n"
        )
        for name in EXTRA_INPUTS:
            self.report["harness"][name] = base.base.sha(ROOT / name)
            shutil.copyfile(ROOT / name, self.directory / Path(name).name)
            (self.directory / Path(name).name).chmod(0o644)
        self.report["scope"] = (
            "Complete actual Freight/EDI/Sync/web image recovery with SCRAM-only "
            "database host authentication, full CA/hostname verification for all "
            "three services and schema/controller/backup/restore clients, and "
            "all inherited58 restricted-role recovery assertions."
        )
        self.report["network_model"] = (
            "Owned shared container namespace with loopback-only controller "
            "ports. Database TCP requires TLS and SCRAM; public ingress and "
            "service HTTP remain the original isolated test profile."
        )
        self.report["limitations"] = [
            x
            for x in self.report["limitations"]
            if "host authentication is trust" not in x
        ] + [
            "Synthetic CA, server key and both login secrets survive in the private controller; this is not off-host or lost-key/configuration recovery.",
            "EDI/Sync retain their inherited database-owner identity; only Freight uses the restricted serving login. No sibling least-privilege claim.",
            "Private TLS volume survives both database generations and is destroyed only at final cleanup; PKI recovery and external CA custody are not qualified.",
            "No public ingress TLS, live carrier/public callback, production capacity, availability, RPO/RTO or independent operator certification.",
        ]
        self.report["ports"] = self.ports

    def build(self):
        super().build()
        fixtures.PostgresTLS.setUpClass()
        self.pki_ready = True
        self.pki = fixtures.PostgresTLS.pki
        (self.pki / "pod.ext").write_text(
            "basicConstraints=critical,CA:false\n"
            "keyUsage=critical,digitalSignature,keyEncipherment\n"
            "extendedKeyUsage=serverAuth\n"
            "subjectAltName=DNS:postgres,IP:127.0.0.1\n"
        )
        fixtures.PostgresTLS.openssl(
            "x509",
            "-req",
            "-in",
            "leaf.csr",
            "-CA",
            "ca.crt",
            "-CAkey",
            "ca.key",
            "-set_serial",
            "901",
            "-days",
            "2",
            "-extfile",
            "pod.ext",
            "-out",
            "pod.crt",
        )
        for name in ["ca.crt", "other-ca.crt", "pod.crt"]:
            shutil.copyfile(self.pki / name, self.directory / name)
            (self.directory / name).chmod(0o644)
        self.tls_volume = "freight-recovery-" + self.container_label + "-tls"
        self.command(
            [
                "docker",
                "volume",
                "create",
                "--label",
                "com.stateset.freight.runtime=" + self.container_label,
                self.tls_volume,
            ],
            label="owned TLS volume",
        )
        args = [
            "docker",
            "run",
            "--rm",
            "--pull=never",
            "--network",
            "none",
            "--label",
            "com.stateset.freight.runtime=" + self.container_label,
            "--entrypoint",
            "bash",
            "--mount",
            "type=volume,source=" + self.tls_volume + ",target=/tls,volume-nocopy",
        ]
        for source, target in [
            (self.pki / "pod.crt", "/source/server.crt"),
            (self.pki / "leaf.key", "/source/server.key"),
            (self.pki / "ca.crt", "/source/ca.crt"),
            (
                self.directory / "install-postgres-tls.sh",
                "/scripts/install-postgres-tls.sh",
            ),
            (
                self.directory / "validate-postgres-tls.sh",
                "/scripts/validate-postgres-tls.sh",
            ),
        ]:
            args += ["--mount", f"type=bind,source={source},target={target},readonly"]
        self.report["tls_install_log"] = self.command(
            args + [self.images["postgres"], "/scripts/install-postgres-tls.sh"],
            label="actual image complete TLS publication",
        )
        self.report["public_certificates"] = {
            name: base.base.sha(self.directory / name)
            for name in ["ca.crt", "other-ca.crt", "pod.crt"]
        }

    def sql(self, database, statement):
        prefix = "CREATE ROLE " + base.identifier(self.runtime_role) + " LOGIN "
        if statement.startswith(prefix):
            statement += " PASSWORD " + literal(self.runtime_verifier)
        return super().sql(database, statement)

    def command(self, command, **kwargs):
        command = list(command)
        if command[0] == "psql":
            url = next((x for x in command if x.startswith("postgres://")), None)
            user = urlsplit(url).username if url else "freight_runtime"
            owner = "runtime" if user == self.runtime_role else "owner"
            # Use the matching pinned16 client, including IP SAN verification,
            # rather than relying on an unsupported host libpq version.
            command = [
                "docker",
                "exec",
                "--env-file",
                str(self.private / (owner + ".env")),
                self.container,
                *[
                    value.replace(
                        f"@127.0.0.1:{self.ports['postgres']}/", "@127.0.0.1:5432/"
                    )
                    for value in command
                ],
            ]
        if command[:2] == ["docker", "run"]:
            # Intercept only inherited launches of the pinned DB/schema/app
            # images. Helpers/extraction containers have no corresponding run.
            if "POSTGRES_HOST_AUTH_METHOD=trust" in command:
                # Keep failed startup logs until exact owned cleanup. The base
                # adapter otherwise auto-removes a failed database immediately.
                command.remove("--rm")
                index = command.index("POSTGRES_HOST_AUTH_METHOD=trust")
                command[index] = "POSTGRES_HOST_AUTH_METHOD=scram-sha-256"
                command[2:2] = [
                    "--env-file",
                    str(self.private / "bootstrap.env"),
                    "--mount",
                    "type=volume,source="
                    + self.tls_volume
                    + ",target="
                    + MATERIAL
                    + ",readonly,volume-nocopy",
                    "--mount",
                    f"type=bind,source={self.directory / 'postgres-tls.pg_hba.conf'},target=/run/freight-tls-hba.conf,readonly",
                    "--mount",
                    f"type=bind,source={self.directory / 'ca.crt'},target={CA},readonly",
                ]
                command += [
                    "postgres",
                    "-c",
                    "ssl=on",
                    "-c",
                    "ssl_min_protocol_version=TLSv1.2",
                    "-c",
                    f"ssl_cert_file={MATERIAL}/current/server.crt",
                    "-c",
                    f"ssl_key_file={MATERIAL}/current/server.key",
                    "-c",
                    "hba_file=/run/freight-tls-hba.conf",
                    "-c",
                    "password_encryption=scram-sha-256",
                ]
            elif (
                "--network" in command and "container:" + str(self.container) in command
            ):
                image = next((x for x in command if x in self.images.values()), None)
                if image in [self.images[x] for x in ["freight", "edi", "sync"]]:
                    owner = (
                        "owner"
                        if image != self.images["freight"] or "--migrate" in command
                        else "runtime"
                    )
                    command[2:2] = [
                        "--env-file",
                        str(self.private / (owner + ".env")),
                        "--mount",
                        f"type=bind,source={self.directory / 'ca.crt'},target={CA},readonly",
                    ]
        if (
            command[:2] == ["docker", "exec"]
            and len(command) > 3
            and command[3] in ["psql", "pg_dump", "pg_restore"]
        ):
            command[2:2] = ["--env-file", str(self.private / "owner.env")]
        stopping = command[:2] == ["docker", "stop"] and command[-1] == self.container
        if stopping:
            self.inspect_container(self.container)
            logs = subprocess.run(
                [
                    "docker",
                    "--host",
                    "unix:///var/run/docker.sock",
                    "logs",
                    self.container,
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                timeout=30,
                env={"PATH": os.defpath, "LC_ALL": "C"},
            )
            require(logs.returncode == 0, "Owned PostgreSQL logs unavailable")
            with (self.directory / f"postgres-{self.pg_generation}.log").open(
                "x"
            ) as out:
                out.write(self.safe(logs.stdout) + "\n")
        try:
            result = super().command(command, **kwargs)
        except RuntimeError as error:
            if command[0] == "psql":
                self.report["last_controller_sql_error"] = self.safe(str(error))
                self.save()
            raise
        if stopping:
            item = self.inspect_container(self.container)
            require(not item["State"]["Running"], "Refuse removal of live database")
            super().command(
                ["docker", "rm", "--volumes", self.container],
                label="owned PostgreSQL final removal",
            )
        return result

    def verify_image_health(self):
        super().verify_image_health()
        rules = self.rows(
            "postgres",
            "SELECT type,database,auth_method,error FROM pg_hba_file_rules ORDER BY line_number",
        )
        require(all(x["error"] is None for x in rules), "Malformed TLS HBA")
        require(
            [
                (x["type"], x["auth_method"])
                for x in rules
                if x["type"].startswith("host")
            ]
            == [
                ("hostnossl", "reject"),
                ("hostssl", "scram-sha-256"),
                ("host", "reject"),
            ],
            "Database TCP policy differs from TLS plus SCRAM",
        )
        sessions = self.rows(
            "postgres",
            "SELECT a.datname,a.usename,s.ssl,s.version,s.cipher,s.bits FROM pg_stat_activity a JOIN pg_stat_ssl s USING(pid) WHERE a.datname IN ('freight_runtime','edi_runtime','sync_runtime') AND a.backend_type='client backend' ORDER BY a.datname,a.pid",
        )
        require(
            {x["datname"] for x in sessions} == set(base.base.base.DATABASES),
            "Missing actual service TLS sessions",
        )
        require(
            all(
                x["ssl"] and x["version"] in ["TLSv1.2", "TLSv1.3"] and x["bits"] >= 128
                for x in sessions
            ),
            "A service has an unprotected database session",
        )
        clients = []
        for name in ["freight", "edi", "sync"]:
            item = self.processes[name].inspect()
            env = dict(x.split("=", 1) for x in item["Config"]["Env"])
            password = (
                self.runtime_password if name == "freight" else self.owner_password
            )
            require(
                env.get("PGSSLMODE") == "verify-full"
                and env.get("PGSSLROOTCERT") == CA
                and env.get("PGPASSWORD") == password,
                "Actual service lost certificate verification or its credential",
            )
            require(
                any(
                    x["Destination"] == CA
                    and x["RW"] is False
                    and x["Source"] == str(self.directory / "ca.crt")
                    for x in item["Mounts"]
                ),
                "Service CA mount differs",
            )
            require(
                not any(x["Destination"] == MATERIAL for x in item["Mounts"]),
                "A serving client acquired the database private key",
            )
            clients.append(
                {
                    "service": name,
                    "image": item["Image"],
                    "verify_full": True,
                    "public_ca_readonly": True,
                }
            )
        require(
            self.sql(
                "postgres",
                "SELECT bool_and(rolpassword LIKE 'SCRAM-SHA-256$%') FROM pg_authid WHERE rolname IN ('freight_runtime',"
                + literal(self.runtime_role)
                + ")",
            )
            == "t",
            "A database identity lacks a SCRAM verifier",
        )
        self.report.setdefault("tls_observations", []).append(
            {
                "generation": self.pg_generation,
                "sessions": sessions,
                "clients": clients,
                "hba": rules,
            }
        )
        self.step(
            "all three actual images verify the CA and server identity with SCRAM and live TLS sessions",
            postgres_generation=self.pg_generation,
        )

    def refuse_clients(self):
        try:
            self.command(
                [
                    "docker",
                    "exec",
                    "--env-file",
                    str(self.private / "owner.env"),
                    "-e",
                    "PGSSLMODE=disable",
                    self.container,
                    "psql",
                    "-U",
                    "freight_runtime",
                    "-d",
                    "postgres",
                    "-X",
                    "-At",
                    "-c",
                    "SELECT current_user",
                ],
                label="owned plaintext negative client",
            )
        except RuntimeError as error:
            require(
                "reject" in str(error).lower()
                and "no encryption" in str(error).lower(),
                "Plaintext refusal had another cause",
            )
            self.report["plaintext_refusal"] = self.safe(str(error))
        else:
            raise AssertionError("Database accepted unencrypted TCP")
        self.step("actual PostgreSQL TCP rejects unencrypted clients before SQL")
        before = self.raw_workspace()
        history = self.rows(
            "freight_runtime",
            "SELECT version,checksum,applied_at FROM freight_schema_migrations ORDER BY version",
        )
        for name in ["freight", "edi", "sync"]:
            original = dict(
                x.split("=", 1) for x in self.processes[name].inspect()["Config"]["Env"]
            )
            for case, markers in [
                ("unrelated-ca", ["UnknownIssuer"]),
                (
                    "wrong-name",
                    ["NotValidForName", 'certificate not valid for name "localhost"'],
                ),
                ("wrong-password", ["password authentication failed"]),
            ]:
                env = dict(original)
                if case == "unrelated-ca":
                    env["PGSSLROOTCERT"] = "/run/unrelated-ca.crt"
                elif case == "wrong-name":
                    env = {
                        k: v.replace("@127.0.0.1:5432/", "@localhost:5432/")
                        for k, v in env.items()
                    }
                else:
                    secret = secrets.token_urlsafe(36)
                    self.secrets.append(secret)
                    env["PGPASSWORD"] = secret
                require(
                    all(
                        "\n" not in k + v and "\r" not in k + v for k, v in env.items()
                    ),
                    "Invalid negative-client environment",
                )
                path = self.private / (name + "-" + case + ".env")
                with path.open("x") as out:
                    out.write("".join(k + "=" + v + "\n" for k, v in env.items()))
                identity = self.command(
                    [
                        "docker",
                        "run",
                        "-d",
                        "--pull=never",
                        "--name",
                        f"freight-negative-{self.container_label}-{name}-{case}",
                        "--label",
                        "com.stateset.freight.runtime=" + self.container_label,
                        "--network",
                        "container:" + self.container,
                        "--env-file",
                        str(path),
                        "--mount",
                        f"type=bind,source={self.directory / 'other-ca.crt'},target=/run/unrelated-ca.crt,readonly",
                        self.images[name],
                    ],
                    label="owned actual negative database client",
                )
                try:
                    item = self.inspect_container(identity)
                    require(
                        item["Image"] == self.images[name]
                        and item["Config"]["User"]
                        == self.processes[name].inspect()["Config"]["User"],
                        "Negative client changed its actual image/user",
                    )
                    code = self.command(
                        ["docker", "wait", identity],
                        label="actual negative client completion",
                        timeout=60,
                    )
                    logs = subprocess.run(
                        [
                            "docker",
                            "--host",
                            "unix:///var/run/docker.sock",
                            "logs",
                            identity,
                        ],
                        env={"PATH": os.defpath, "LC_ALL": "C"},
                        stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT,
                        text=True,
                        timeout=15,
                    )
                    require(logs.returncode == 0, "Negative client logs unavailable")
                    log = self.safe(logs.stdout)
                    with (self.directory / (name + "-" + case + ".log")).open(
                        "x"
                    ) as out:
                        out.write(log)
                    require(
                        code != "0"
                        and any(marker.lower() in log.lower() for marker in markers),
                        "Actual negative client refused for another reason: "
                        + log[-1800:],
                    )
                    self.report.setdefault("negative_clients", []).append(
                        {
                            "service": name,
                            "case": case,
                            "image": item["Image"],
                            "exit_code": int(code),
                            "expected_marker": next(
                                marker
                                for marker in markers
                                if marker.lower() in log.lower()
                            ),
                            "log_sha256": base.base.sha(
                                self.directory / (name + "-" + case + ".log")
                            ),
                        }
                    )
                finally:
                    item = self.inspect_container(identity)
                    if item["State"]["Running"]:
                        self.command(
                            ["docker", "stop", "--time", "5", identity],
                            label="owned negative client stop",
                        )
                    self.command(
                        ["docker", "rm", "--volumes", identity],
                        label="owned negative client removal",
                    )
                self.step(
                    "actual " + name + " image refuses " + case + " before serving"
                )
        require(
            self.raw_workspace() == before
            and self.rows(
                "freight_runtime",
                "SELECT version,checksum,applied_at FROM freight_schema_migrations ORDER BY version",
            )
            == history,
            "Refused clients changed original business rows or migration metadata",
        )
        self.step(
            "refused database clients preserve exact original business rows and migration metadata"
        )

    def cleanup(self):
        super().cleanup()
        if self.tls_volume:
            item = json.loads(
                self.command(
                    ["docker", "volume", "inspect", self.tls_volume],
                    label="owned TLS cleanup identity",
                )
            )[0]
            require(
                item["Labels"].get("com.stateset.freight.runtime")
                == self.container_label,
                "Refuse unowned TLS cleanup",
            )
            self.command(
                ["docker", "volume", "rm", self.tls_volume], label="owned TLS cleanup"
            )
            self.tls_volume = None
        if self.pki_ready:
            fixtures.PostgresTLS.tearDownClass()
            self.pki_ready = False
        self.report["tls_cleanup"] = (
            "owned TLS volume and temporary private PKI removed; controller credential directory remains private"
        )


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
    run = TLSRecovery(args)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        run.build()
        run.postgres()
        run.sync()
        run.edi()
        run.freight()
        run.start_web()
        run.checks()
        run.refuse_clients()
        run.report["baseline_checks"] = len(run.report["checks"])
        run.record_recovery_cash()
        run.record_save_resolution()
        run.prepare_outage()
        run.backup_and_lose()
        run.restore()
        run.recovered_work()
        run.verify_save_resolution()
        require(
            not (
                run.required_checks
                - base.recovery_check_inventory(run.report["checks"])
            ),
            "An inherited recovery assertion was lost",
        )
        require(
            run.report.get("edi_volume_restored")
            and len(run.report.get("tls_observations", [])) == 2,
            "Full TLS restore observations missing",
        )
        for relative, digest in run.report["harness"].items():
            require(
                base.base.sha(ROOT / relative) == digest,
                "Recovery harness changed during run",
            )
        for name in ["freight", "edi", "sync"]:
            snapshot = Path(run.source["snapshots"][name])
            require(
                fingerprint(snapshot) == run.source["sources"][name],
                "Frozen service source drift",
            )
            for relative, digest in run.source["inputs"][name].items():
                require(
                    base.base.sha(snapshot / relative) == digest,
                    "Frozen Docker input drift",
                )
            require(
                base.base.sha(run.binaries[name])
                == run.report["builds"][name]["binary_sha256"],
                "Image binary drift",
            )
        run.report["result"] = "pass"
    except BaseException as error:
        run.report.update(
            result="failed",
            error=run.safe(type(error).__name__ + ": " + str(error)),
            traceback=run.safe(traceback.format_exc()),
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
