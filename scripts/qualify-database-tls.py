#!/usr/bin/env python3
"""Actual-image TLS overlay qualification, including the complete role workflow.

Generates only private synthetic test PKI and owns a fresh Compose project.
Does not accept deployment certificates, credentials or database connections.
"""

import argparse
from collections import Counter
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import sys
import traceback

from runtime_contracts import ROOT, fingerprint, require


def module(name, relative):
    spec = importlib.util.spec_from_file_location(name, ROOT / relative)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


base = module("database_tls_roles", "scripts/qualify-authenticated-roles.py")
fixtures = module("database_tls_fixtures", "scripts/test_postgres_tls.py")
EXTRA_INPUTS = [
    "compose.database-tls.yaml",
    "scripts/postgres-tls.pg_hba.conf",
    "scripts/validate-postgres-tls.sh",
    "scripts/install-postgres-tls.sh",
    "scripts/test_postgres_tls.py",
    "scripts/qualify-database-tls.py",
]


class TLSQualification(base.RoleQualification):
    def __init__(self, args):
        super().__init__(args)
        self.pki_ready = False
        for name in EXTRA_INPUTS:
            self.manifest[name] = base.sha(ROOT / name)
            dst = self.directory / name
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, dst)
            dst.chmod(0o644)
        fixtures.PostgresTLS.setUpClass()
        self.pki_ready = True
        self.pki = self.directory / "pki"
        self.pki.mkdir(mode=0o700)
        for file in fixtures.PostgresTLS.pki.iterdir():
            if file.is_file() and file.suffix in [".crt", ".key"]:
                target = self.pki / file.name
                shutil.copyfile(file, target)
                target.chmod(0o600 if file.suffix == ".key" else 0o644)
        self.env.update(
            FREIGHT_DATABASE_TLS_CERT=str(self.pki / "server.crt"),
            FREIGHT_DATABASE_TLS_KEY=str(self.pki / "leaf.key"),
            FREIGHT_DATABASE_TLS_CA=str(self.pki / "ca.crt"),
        )
        self.compose += ["--file", str(self.directory / "compose.database-tls.yaml")]
        self.report["inputs"] = self.manifest
        self.report["scope"] = (
            "Actual-image TLS-only PostgreSQL, certificate-verified schema/role/API connections and complete managed-role SCRAM/cancellation/rotation/original-receipt/session workflow, with privately generated synthetic test PKI."
        )
        self.report["limitations"] = [
            "Private synthetic CA/keys generated in the controller; no external CA issuance, certificate custody, revocation or disaster recovery commissioning.",
            "TLS protects database TCP traffic; public web/API ingress remains loopback HTTP in this local fixture.",
            "No production capacity, independent operator outcomes or full EDI/Sync TLS restore claim.",
            "Earlier592 application and58 full-stack recovery scenarios remain separate and source-bound; the repeated21 role checks are included in this profile, not added twice.",
        ]

    def tcp_sql_environment(self, user, password):
        return {
            "PGHOST": "postgres",
            "PGUSER": user,
            "PGDATABASE": "freight",
            "PGPASSWORD": password or "",
            "PGSSLMODE": "verify-full",
            "PGSSLROOTCERT": "/run/freight-database-ca.crt",
        }

    def verify_host_authentication(self):
        rules = json.loads(
            self.sql(
                "SELECT json_agg(row_to_json(t)) FROM (SELECT type,database,auth_method,error FROM pg_hba_file_rules ORDER BY line_number)t"
            )[0]
        )
        require(
            all(x["error"] is None for x in rules), "Invalid installed TLS HBA rules"
        )
        require(
            any(
                x["type"] == "hostnossl"
                and x["auth_method"] == "reject"
                and x["database"] == ["all"]
                for x in rules
            ),
            "Unencrypted host connections are not explicitly refused",
        )
        require(
            any(
                x["type"] == "hostssl"
                and x["auth_method"] == "scram-sha-256"
                and x["database"] == ["all"]
                for x in rules
            ),
            "TLS host SCRAM rule missing",
        )
        require(
            not any(
                x["type"].startswith("host")
                and x["auth_method"] not in ["reject", "scram-sha-256"]
                for x in rules
            ),
            "Unexpected weaker host rule",
        )
        self.report["actual_hba_rules"] = rules
        self.tls_sessions()
        self.check(
            "actual installed HBA requires TLS plus SCRAM and real API sessions use TLS1.2or1.3"
        )

    def tls_sessions(self):
        sessions = json.loads(
            self.sql(
                "SELECT json_agg(row_to_json(t)) FROM (SELECT a.usename,s.ssl,s.version,s.cipher,s.bits FROM pg_stat_activity a JOIN pg_stat_ssl s USING(pid) WHERE a.datname=current_database() AND a.usename='freight_app' ORDER BY a.pid)t"
            )[0]
        )
        require(
            sessions
            and all(
                x["ssl"] and x["version"] in ["TLSv1.2", "TLSv1.3"] and x["bits"] >= 128
                for x in sessions
            ),
            "API has a non-TLS or weak TLS connection",
        )
        self.report.setdefault("actual_tls_sessions", []).append(sessions)

    def tls_material(self):
        pg = self.service("postgres")["Id"]
        files = self.run(
            base.D
            + [
                "exec",
                pg,
                "find",
                "/run/freight-database-tls",
                "-type",
                "f",
                "-printf",
                "%P %m %U %G %T@ ",
                "-exec",
                "sha256sum",
                "{}",
                ";",
            ]
        )[0]
        target = self.run(
            base.D + ["exec", pg, "readlink", "/run/freight-database-tls/current"]
        )[0]
        return files + "\n" + target

    def refused_api(self, name, flags, markers):
        out, error, _ = self.compose_run(
            "run", "--rm", "--no-deps", "-T", *flags, "api", expected=None, timeout=60
        )
        require(
            any(x.lower() in (out + error).lower() for x in markers),
            "API TLS refusal had another cause: " + self.safe((out + error)[-1800:]),
        )
        self.check(name)

    def exercise(self):
        super().exercise()
        inherited = json.loads(
            (ROOT / "docs/evidence/authenticated-roles-profile-v4.json").read_text()
        )
        require(
            not (
                Counter(x["name"] for x in inherited["checks"])
                - Counter(x["name"] for x in self.report["checks"])
            ),
            "An inherited role assertion was lost",
        )
        pg = self.service("postgres")["Id"]
        actual_uid = self.run(base.D + ["exec", pg, "id", "-u", "postgres"])[0]
        key_mode = self.run(
            base.D
            + [
                "exec",
                pg,
                "stat",
                "-Lc",
                "%a %u",
                "/run/freight-database-tls/current/server.key",
            ]
        )[0]
        require(
            key_mode == "600 " + actual_uid,
            "Actual PostgreSQL key ownership/mode is unsafe",
        )
        for name in ["migrate", "grants", "api"]:
            item = self.service(name)
            env = dict(x.split("=", 1) for x in item["Config"]["Env"])
            require(
                env["PGSSLMODE"] == "verify-full"
                and env["PGSSLROOTCERT"] == "/run/freight-database-ca.crt",
                "Actual database client lost certificate verification",
            )
            require(
                any(
                    m["Destination"] == "/run/freight-database-ca.crt" and not m["RW"]
                    for m in item["Mounts"]
                ),
                "Actual client CA mount is not readonly",
            )
        self.check(
            "actual PostgreSQL private key is mode0600 under its image user and every client has verify-full plus readonly CA"
        )
        before = self.metadata(), self.business()
        material = self.tls_material()
        env = {
            **self.env,
            **self.tcp_sql_environment("freight_app", self.runtime_password),
            "PGSSLMODE": "disable",
        }
        command = base.D + ["exec"]
        for key in self.tcp_sql_environment("freight_app", self.runtime_password):
            command += ["-e", key]
        _, error, _ = self.run(
            command + [pg, "psql", "-w", "-X", "-At", "-c", "SELECT current_user"],
            env=env,
            expected=None,
        )
        require(
            "no encryption" in error or "pg_hba.conf rejects" in error,
            "Plain TCP refusal had another cause",
        )
        self.refused_api(
            "actual API refuses sslmode=disable against the TLS-only server",
            ["-e", "PGSSLMODE=disable"],
            ["no encryption", "pg_hba.conf rejects"],
        )
        self.refused_api(
            "actual API refuses an unrelated certificate authority",
            [
                "-v",
                str(self.pki / "other-ca.crt") + ":/run/freight-other-ca.crt:ro",
                "-e",
                "PGSSLROOTCERT=/run/freight-other-ca.crt",
            ],
            ["UnknownIssuer", "unknown issuer", "certificate verify failed"],
        )
        networks = self.service("postgres")["NetworkSettings"]["Networks"]
        require(len(networks) == 1, "Unexpected PostgreSQL network scope")
        address = next(iter(networks.values()))["IPAddress"]
        self.refused_api(
            "actual API refuses an IP hostname missing from the server DNS certificate",
            ["-e", f"DATABASE_URL=postgres://freight_app@{address}:5432/freight"],
            ["NotValidForName", "not valid for", "IP address mismatch"],
        )
        self.refused_api(
            "actual API refuses a missing CA file without falling back",
            ["-e", "PGSSLROOTCERT=/run/owned-missing-ca.crt"],
            ["No such file", "cannot find", "os error 2"],
        )
        for name, variable, candidate, markers in [
            (
                "mismatched-key",
                "FREIGHT_DATABASE_TLS_KEY",
                "next.key",
                ["do not match"],
            ),
            ("expired-leaf", "FREIGHT_DATABASE_TLS_CERT", "expired.crt", ["expired"]),
            (
                "wrong-hostname",
                "FREIGHT_DATABASE_TLS_CERT",
                "wrong-host.crt",
                ["hostname mismatch"],
            ),
            (
                "untrusted-authority",
                "FREIGHT_DATABASE_TLS_CA",
                "other-ca.crt",
                ["unable to get local issuer", "unable to get issuer"],
            ),
        ]:
            out, error, _ = self.compose_run(
                "run",
                "--rm",
                "--no-deps",
                "-T",
                "database-tls-files",
                expected=None,
                env={**self.env, variable: str(self.pki / candidate)},
            )
            require(
                any(marker in (out + error).lower() for marker in markers),
                "Certificate helper refused for another cause: " + name,
            )
            require(
                self.tls_material() == material,
                "Rejected certificate rotation changed original published material",
            )
        self.check(
            "actual image TLS installer refuses bad key, expired leaf, wrong hostname and untrusted CA without changing published material"
        )
        require(
            before == (self.metadata(), self.business()),
            "TLS refusal controls changed original database rows",
        )
        self.check(
            "all real TLS refusal controls preserve exact migration metadata and every business table"
        )
        self.wait_ready()
        self.wait_web_access()
        replay = self.http("/v1/customers", self.customer_body, self.customer_key)
        require(
            replay["data"] == self.customer_original["data"]
            and replay["replayed"] is True,
            "TLS controls changed original receipt",
        )
        self.tls_sessions()
        self.check(
            "original receipt and signed web session remain valid with healthy certificate-verified API after all TLS faults"
        )
        base.previews.retained(self, record=False)
        self.check('PDF original and source-bound preview remain valid after all TLS fault controls')
        base.previews.exercise_client(self, 'after TLS fault controls')
        for name, digest in self.manifest.items():
            require(
                base.sha(ROOT / name) == base.sha(self.directory / name) == digest,
                "TLS operational input drift",
            )
        require(fingerprint(ROOT) == self.source, "Application source drift")
        self.report["result"] = "pass"

    def cleanup(self):
        try:
            super().cleanup()
        finally:
            if self.pki_ready:
                fixtures.PostgresTLS.tearDownClass()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--verification",
        type=Path,
        default=ROOT / "docs/evidence/schema-startup-verification.json",
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument('--tui', type=Path, help='Optional built client: also require real API/TUI PDF graphics before and after TLS faults')
    parser.add_argument('--docker-subnet', help='Unused RFC1918 /24 through /28 for this owned Compose project; choose a range outside host/VPN routes')
    args = parser.parse_args()
    require(not args.output.exists(), "Preserve earlier reports")
    os.umask(0o077)
    run = TLSQualification(args)
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        run.exercise()
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
