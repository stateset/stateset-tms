#!/usr/bin/env python3
"""Audit the actual production dependency graph without suppressing advisories.

Cargo.lock also retains inactive optional dependencies. Keep the complete raw
RustSec report, and fail on every vulnerable package reachable through Cargo's
normal/build graph for the declared target and feature configuration. This is
dependency-advisory coverage, not a code or deployment security assessment.

A qualification helper (never the server or terminal application) may carry a
reviewed exception in scripts/advisory-exceptions.json. Each one names one
advisory, crate version and helper package, gives a reason, and expires within
90 days. An expired or unused exception fails the audit, and every excepted
finding stays in the report.
"""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
EXCEPTIONS = "scripts/advisory-exceptions.json"
EXCEPTION_FIELDS = {"advisory", "package", "crate", "version", "approved", "expires", "reason"}
APPLICATION_ROOTS = {"stateset-freight", "stateset-freight-tui"}
MAX_EXCEPTION_DAYS = 90


def binding(path):
    return {"path": str(path.resolve()), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def load_exceptions(path):
    """Reviewed helper exceptions; an absent file means none."""
    if not path.exists():
        return []

    def unique(pairs):
        keys = [key for key, _ in pairs]
        require(len(keys) == len(set(keys)), "Duplicate field in advisory exceptions")
        return dict(pairs)

    document = json.loads(path.read_text(), object_pairs_hook=unique)
    require(isinstance(document, dict) and set(document) == {"exceptions"} and isinstance(document["exceptions"], list),
            "Advisory exceptions must be {\"exceptions\": [...]}")
    seen = set()
    for entry in document["exceptions"]:
        require(isinstance(entry, dict) and set(entry) == EXCEPTION_FIELDS
                and all(isinstance(entry[k], str) and entry[k].strip() for k in EXCEPTION_FIELDS),
                "Each advisory exception needs exactly: " + ", ".join(sorted(EXCEPTION_FIELDS)))
        require(re.fullmatch(r"RUSTSEC-\d{4}-\d{4}", entry["advisory"]), "Advisory exception must name one RUSTSEC id")
        require(entry["package"] not in APPLICATION_ROOTS, "Application graphs cannot carry advisory exceptions")
        approved, expires = dt.date.fromisoformat(entry["approved"]), dt.date.fromisoformat(entry["expires"])
        require(approved <= expires <= approved + dt.timedelta(days=MAX_EXCEPTION_DAYS),
                f"Advisory exception must expire within {MAX_EXCEPTION_DAYS} days of approval")
        key = (entry["package"], entry["advisory"], entry["crate"], entry["version"])
        require(key not in seen, "Duplicate advisory exception")
        seen.add(key)
    return document["exceptions"]


def package_name(value):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]*", value):
        raise argparse.ArgumentTypeError("Select one exact Cargo package name, not a pattern or package specification")
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=package_name, default="stateset-freight",
                        help="Exact production root package (default: stateset-freight; client: stateset-freight-tui)")
    parser.add_argument("--manifest-path", type=Path,
                        help="Repository-relative or absolute Cargo.toml for the main, integration-contracts or runtime-contracts workspace")
    parser.add_argument("--target", help="Defaults to CARGO_BUILD_TARGET or the compiler host")
    parser.add_argument("--features", help="Must match any non-default production build features")
    parser.add_argument("--no-default-features", action="store_true")
    parser.add_argument("--db", type=Path, help="RustSec advisory database cache")
    parser.add_argument("--no-fetch", action="store_true", help="Reuse the recorded advisory database revision")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(not args.output.exists(), "Select a fresh report path; preserve preceding evidence")
    manifest = ((ROOT / args.manifest_path) if args.manifest_path else ROOT / "Cargo.toml").resolve()
    allowed = {(ROOT / name / "Cargo.toml").resolve() for name in (".", "integration-contracts", "runtime-contracts")}
    require(manifest in allowed, "Select an explicit repository qualification workspace manifest")
    lockfile = manifest.parent / "Cargo.lock"
    directory = Path(tempfile.mkdtemp(prefix="freight-production-audit-"))
    (directory / "harness.py").write_bytes(Path(__file__).read_bytes())
    manifests = [ROOT / "Cargo.toml", *sorted((ROOT / "crates").rglob("Cargo.toml"))] if args.manifest_path is None else [manifest]
    report = {"result": "running", "scope": __doc__, "artifacts": str(directory),
              "started_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
              "lockfile": binding(lockfile), "manifests": [binding(p) for p in manifests],
              "workspace_manifest": binding(manifest),
              "harness": binding(Path(__file__)), "harness_archive": binding(directory / "harness.py"),
              "package": args.package,
              "features": args.features, "default_features": not args.no_default_features,
              "advisories_ignored": [], "active_vulnerabilities": [], "excepted_vulnerabilities": [],
              "inactive_lockfile_advisories": []}
    exceptions_path = ROOT / EXCEPTIONS

    def execute(command, name):
        output, error = directory / (name + ".stdout"), directory / (name + ".stderr")
        with output.open("xb") as out, error.open("xb") as err:
            completed = subprocess.run(command, cwd=ROOT, stdout=out, stderr=err)
        return completed.returncode, output, error

    try:
        if exceptions_path.exists():
            report["advisory_exceptions"] = binding(exceptions_path)
        exceptions = [e for e in load_exceptions(exceptions_path) if e["package"] == args.package]
        require(not exceptions or args.manifest_path is not None, "Application graphs cannot carry advisory exceptions")
        today = dt.datetime.now(dt.timezone.utc).date()
        expired = [e["advisory"] for e in exceptions if dt.date.fromisoformat(e["expires"]) < today]
        require(not expired, "Advisory exception expired: " + ", ".join(expired))
        compiler = subprocess.check_output(["rustc", "-vV"], text=True)
        host = next(line.split("host: ", 1)[1] for line in compiler.splitlines() if line.startswith("host: "))
        target = args.target or os.environ.get("CARGO_BUILD_TARGET") or host
        report.update(target=target, compiler=compiler.strip())
        if args.manifest_path is not None:
            # A standalone helper has its own lock and path dependencies. Bind
            # Cargo's actual workspace and every mutable path manifest before
            # selecting its normal/build graph; never substitute the API graph.
            metadata = ["cargo", "metadata", "--locked", "--offline", "--format-version", "1",
                        "--manifest-path", str(manifest), "--filter-platform", target]
            if args.features:
                metadata += ["--features", args.features]
            if args.no_default_features:
                metadata.append("--no-default-features")
            status, metadata_output, metadata_error = execute(metadata, "workspace-metadata")
            report["workspace_metadata"] = {"stdout": binding(metadata_output), "stderr": binding(metadata_error), "exit_code": status}
            require(status == 0, "Cannot resolve the selected workspace; inspect retained diagnostics")
            resolved = json.loads(metadata_output.read_text())
            require(Path(resolved["workspace_root"]).resolve() == manifest.parent,
                    "Cargo resolved a different workspace root")
            paths = {Path(p["manifest_path"]).resolve() for p in resolved["packages"] if p["source"] is None}
            require(manifest in paths, "Selected workspace manifest is absent from Cargo metadata")
            require(binding(manifest) == report["workspace_manifest"], "Workspace manifest changed during resolution")
            manifests = sorted(paths)
            report["manifests"] = [binding(p) for p in manifests]
        tree = ["cargo", "tree", "--locked", "--offline", "--package", args.package, "--target", target,
                "--edges", "normal,build", "--prefix", "none", "--format", "{p}"]
        if args.manifest_path is not None:
            tree += ["--manifest-path", str(manifest)]
        if args.features:
            tree += ["--features", args.features]
        if args.no_default_features:
            tree.append("--no-default-features")
        status, tree_output, tree_error = execute(tree, "production-tree")
        report["dependency_tree"] = {"stdout": binding(tree_output), "stderr": binding(tree_error), "exit_code": status}
        require(status == 0, "Cannot resolve the declared production graph; inspect retained diagnostics")
        # A whitespace-bounded version is essential: a package without a path
        # suffix must never absorb the following line into its version.
        ordered_packages = re.findall(r"^([A-Za-z0-9_-]+) v([0-9][^\s]*)", tree_output.read_text(), re.MULTILINE)
        require(ordered_packages and ordered_packages[0][0] == args.package,
                "Selected production root is not the first dependency graph node")
        packages = set(ordered_packages)
        report["package_version"] = ordered_packages[0][1]
        report["active_packages"] = [{"name": name, "version": version} for name, version in sorted(packages)]
        database = args.db.resolve() if args.db else directory / "advisory-db"
        database_before = subprocess.check_output(["git", "-C", str(database), "rev-parse", "HEAD"], text=True).strip() if args.no_fetch else None
        audit = ["cargo", "audit", "--db", str(database), "--file", str(lockfile), "--json"]
        if args.no_fetch:
            audit.append("--no-fetch")
        status, raw_output, raw_error = execute(audit, "rustsec-lockfile")
        report["raw_lockfile_audit"] = {"stdout": binding(raw_output), "stderr": binding(raw_error), "exit_code": status}
        require(status in {0, 1}, "RustSec audit could not complete; inspect retained diagnostics")
        raw = json.loads(raw_output.read_text())
        settings = raw["settings"]
        require(not settings["ignore"] and not settings["target_arch"] and not settings["target_os"]
                and settings["severity"] is None, "RustSec configuration filters or suppresses advisories")
        vulnerabilities = raw["vulnerabilities"]
        require(vulnerabilities["count"] == len(vulnerabilities["list"])
                and vulnerabilities["found"] == bool(vulnerabilities["list"]), "Incomplete RustSec result")
        # cargo-audit omits Git metadata in --no-fetch mode. Bind the clean
        # local checkout explicitly rather than inventing a database revision.
        database_commit = subprocess.check_output(["git", "-C", str(database), "rev-parse", "HEAD"], text=True).strip()
        require(not subprocess.check_output(["git", "-C", str(database), "status", "--porcelain", "--untracked-files=all"], text=True).strip(), "Advisory database has local changes")
        require(database_before in {None, database_commit}, "Advisory database changed during offline audit")
        require(raw["database"]["last-commit"] in {None, database_commit}, "Advisory database revision changed")
        report["advisory_database"] = {**raw["database"], "resolved_git_commit": database_commit, "worktree_clean": True}
        for finding in vulnerabilities["list"]:
            package = finding["package"]
            key = "active_vulnerabilities" if (package["name"], package["version"]) in packages else "inactive_lockfile_advisories"
            report[key].append(finding)
        used = []
        for exception in exceptions:
            selected = (exception["advisory"], exception["crate"], exception["version"])
            matched = [f for f in report["active_vulnerabilities"]
                       if (f["advisory"]["id"], f["package"]["name"], f["package"]["version"]) == selected]
            for finding in matched:
                report["active_vulnerabilities"].remove(finding)
                report["excepted_vulnerabilities"].append({"exception": exception, "finding": finding})
            if matched:
                used.append(exception)
        unused = [e["advisory"] for e in exceptions if e not in used]
        require(not unused, "Remove unused advisory exception: " + ", ".join(unused))
        report["warnings"] = raw.get("warnings", {})
        require(binding(lockfile) == report["lockfile"], "Lockfile changed during audit")
        require(report.get("advisory_exceptions") == (binding(exceptions_path) if exceptions_path.exists() else None),
                "Advisory exceptions changed during audit")
        require([binding(p) for p in manifests] == report["manifests"], "Build manifests changed during audit")
        require(binding(Path(__file__)) == report["harness"], "Audit harness changed")
        report["result"] = "failed" if report["active_vulnerabilities"] else "pass"
        excepted = "; ".join(f"{v['exception']['advisory']} in {v['exception']['crate']} {v['exception']['version']} "
                             f"until {v['exception']['expires']}" for v in report["excepted_vulnerabilities"])
        print(f"Production RustSec audit ({args.package}): {len(report['active_vulnerabilities'])} active vulnerabilities; "
              f"{len(report['inactive_lockfile_advisories'])} inactive lockfile advisories retained; "
              + (f"reviewed exceptions applied: {excepted}." if excepted else "no advisories suppressed."))
    except Exception as error:
        report.update(result="failed", error=f"{type(error).__name__}: {error}")
        print(report["error"])
    finally:
        report["finished_utc"] = dt.datetime.now(dt.timezone.utc).isoformat()
        with args.output.open("x") as out:
            out.write(json.dumps(report, indent=2) + "\n")
        print("REPORT", args.output)
    return 0 if report["result"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
