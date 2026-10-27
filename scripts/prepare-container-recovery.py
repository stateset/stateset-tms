#!/usr/bin/env python3
"""Prepare or verify a frozen local bundle for qualify-container-recovery.py.

Builds the actual sibling Dockerfiles without changing their source checkouts.
Freight/web/PostgreSQL images must already match a source-bound functional
verification record. All bundle outputs are new; existing evidence is retained.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

from runtime_contracts import ROOT, fingerprint, require

SELECTION = {
    "edi": [
        "Cargo.toml",
        "Cargo.lock",
        "Dockerfile",
        ".dockerignore",
        "src",
        "crates",
        "migrations",
        "site",
        "vendor",
    ],
    "sync": [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "Dockerfile",
        ".dockerignore",
        "src",
        "proto",
        "benches",
        "migrations",
        "connectors",
        "assets",
    ],
}
DIRECTORIES = {
    "freight": "stateset-freight",
    "edi": "stateset-edi",
    "sync": "stateset-sync-server",
}
D = ["docker", "--host", "unix:///var/run/docker.sock"]
CONTROLLER_INPUTS = (
    "scripts/prepare-container-recovery.py",
    "scripts/qualify-container-recovery.py",
    "scripts/qualify-recovery.py",
    "scripts/runtime_contracts.py",
    "scripts/native_postgres.py",
    "scripts/migration_ledger.py",
    "scripts/qualify-restricted-recovery.py",
    "scripts/qualify-tls-recovery.py",
    "scripts/recovery-check-inventory.json",
    "scripts/test_postgres_tls.py",
    "scripts/install-postgres-tls.sh",
    "scripts/validate-postgres-tls.sh",
    "scripts/postgres-tls.pg_hba.conf",
)


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def read(path):
    return json.loads(Path(path).read_text())


def write(path, value):
    with path.open("x") as out:
        json.dump(value, out, indent=2)
        out.write("\n")


def snapshot_path(bundle, service):
    return bundle / "workspace" / DIRECTORIES[service]


def manifest_file(root, name):
    """Validate a captured file before either reading it or copying its name."""
    require(
        root.is_dir() and root == root.resolve(),
        "Snapshot root must be a canonical directory",
    )
    require(isinstance(name, str) and name and "\0" not in name, "Unsafe manifest path")
    relative = Path(name)
    require(
        not relative.is_absolute()
        and ".." not in relative.parts
        and str(relative) == name
        and name != ".",
        "Unsafe manifest path",
    )
    path = root
    for part in relative.parts:
        path = path / part
        require(not path.is_symlink(), "Snapshot symlinks require explicit handling")
    require(path.is_file(), "Manifest input must be a regular file: " + name)
    return path


def capture_wire_source(root, destination):
    """Merge identical primary-manifest inputs without replacing captured bytes."""
    require(not destination.is_symlink(), "Wire snapshot must not be a symlink")
    require(
        destination == destination.resolve(),
        "Wire snapshot path must be canonical; parent symlinks are forbidden",
    )
    destination.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for name in ["Cargo.toml", "Cargo.lock", "src/main.rs"]:
        src = manifest_file(root, "runtime-contracts/" + name)
        digest = sha(src)
        dst = destination
        for part in Path(name).parts:
            dst = dst / part
            require(not dst.is_symlink(), "Wire snapshot symlinks are forbidden")
        dst.parent.mkdir(parents=True, exist_ok=True)
        if dst.exists():
            require(
                dst.is_file() and sha(dst) == digest
                and dst.stat().st_mode & 0o777 == src.stat().st_mode & 0o777,
                "Captured wire input conflicts: " + name,
            )
        else:
            shutil.copyfile(src, dst)
            dst.chmod(src.stat().st_mode & 0o777)
        require(sha(src) == sha(dst) == digest, "Wire input changed during capture")
        manifest[name] = digest
    return manifest


def verify(bundle):
    source, images, wire = [
        read(bundle / name) for name in ["source.json", "images.json", "wire.json"]
    ]
    require(source["sources"] == images["sources"], "Image/source manifests differ")
    require(
        fingerprint(ROOT) == source["sources"]["freight"],
        "Current Freight differs from bundle",
    )
    for service, manifest in source["inputs"].items():
        snapshot = snapshot_path(bundle, service)
        for name, digest in manifest.items():
            require(
                sha(manifest_file(snapshot, name)) == digest,
                "Snapshot input changed: " + name,
            )
        require(
            fingerprint(snapshot) == source["sources"][service],
            "Snapshot source changed",
        )
    for identifier in images["images"].values():
        require(
            identifier.startswith("sha256:") and len(identifier) == 71,
            "Immutable image ID required",
        )
        actual = subprocess.check_output(
            D + ["image", "inspect", identifier, "--format", "{{.Id}}"],
            env={"PATH": os.defpath},
            text=True,
        ).strip()
        require(actual == identifier, "Image identity changed")
    require(sha(bundle / "wire") == wire["binary"]["sha256"], "Wire helper changed")
    for name, digest in wire["source_manifest"].items():
        require(
            sha(
                manifest_file(
                    snapshot_path(bundle, "freight") / "runtime-contracts", name
                )
            )
            == digest,
            "Wire source changed",
        )
    print("PASS frozen inputs, image IDs and actual wire helper", flush=True)


def prepare(args):
    bundle = args.bundle
    require(not bundle.exists(), "Bundle output must be a new directory")
    report, freight = read(args.freight_verification), read(args.freight_source)
    require(
        report["result"] == "pass"
        and report["source"] == freight["source"] == fingerprint(ROOT),
        "Functional verification must bind current Freight",
    )
    for item in report["evidence"].values():
        path = manifest_file(ROOT, item["path"])
        require(
            sha(path) == item["sha256"] and path.stat().st_size == item["bytes"],
            "Functional evidence changed",
        )
    for name, digest in freight["manifest"].items():
        require(sha(manifest_file(ROOT, name)) == digest, "Functional input changed")
    images = {
        "freight": report["images"]["api"],
        "web": report["images"]["web"],
        "postgres": report["images"]["postgres"],
    }
    bundle.mkdir(mode=0o700)
    (bundle / "workspace").mkdir()
    repositories = {
        "freight": ROOT,
        "edi": args.edi_repo.resolve(),
        "sync": args.sync_repo.resolve(),
    }
    sources = {name: fingerprint(path) for name, path in repositories.items()}
    inputs = {"freight": dict(freight["manifest"])}
    for script in CONTROLLER_INPUTS:
        inputs["freight"][script] = sha(ROOT / script)
    for name, selected in SELECTION.items():
        manifest = {}
        for relative in selected:
            path = repositories[name] / relative
            require(path.exists(), "Missing Docker input: " + str(path))
            require(
                not path.is_symlink(), "Snapshot symlinks require explicit handling"
            )
            paths = (
                [path]
                if path.is_file()
                else [
                    p
                    for p in path.rglob("*")
                    if p.is_file()
                    and not any(
                        part in ["target", "node_modules", ".git"] for part in p.parts
                    )
                ]
            )
            for path in paths:
                relative_name = str(path.relative_to(repositories[name]))
                manifest[relative_name] = sha(
                    manifest_file(repositories[name], relative_name)
                )
        inputs[name] = manifest
    for name, manifest in inputs.items():
        destination = snapshot_path(bundle, name)
        destination.mkdir()
        for relative, digest in manifest.items():
            src = manifest_file(repositories[name], relative)
            dst = destination / relative
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, dst)
            # The enclosing bundle is private. Preserve source read/execute
            # bits so capture does not change Docker inputs or defeat caches.
            dst.chmod(src.stat().st_mode & 0o777)
            require(sha(src) == sha(dst) == digest, "Source changed during capture")
        require(
            fingerprint(destination)
            == sources[name]
            == fingerprint(repositories[name]),
            "Source changed during capture",
        )
    write(
        bundle / "source.json",
        {
            "sources": sources,
            "inputs": inputs,
            "snapshots": {
                name: str(snapshot_path(bundle, name)) for name in repositories
            },
            "repositories": {name: str(path) for name, path in repositories.items()},
            "prior_verification": {
                "path": str(args.freight_verification.resolve()),
                "sha256": sha(args.freight_verification),
            },
            "scope": "Exact Dockerfile inputs and source-bound Freight release; live sibling checkouts are read-only.",
        },
    )
    for name in ["edi", "sync"]:
        print("BUILD actual frozen " + name + " Dockerfile", flush=True)
        with (bundle / ("build-" + name + "-image.log")).open("xb") as out:
            result = subprocess.run(
                D
                + [
                    "build",
                    "--progress=plain",
                    "--iidfile",
                    str(bundle / (name + "-image-id")),
                    "-t",
                    "stateset-freight-recovery-" + name + ":qualification",
                    str(snapshot_path(bundle, name)),
                ],
                env={"PATH": os.defpath},
                stdout=out,
                stderr=subprocess.STDOUT,
                timeout=3600,
            )
        require(result.returncode == 0, name + " image build failed; retained log")
        images[name] = (bundle / (name + "-image-id")).read_text().strip()
        for service, manifest in inputs.items():
            for relative, digest in manifest.items():
                require(
                    sha(snapshot_path(bundle, service) / relative) == digest,
                    "Frozen input changed during image build",
                )
    write(bundle / "images.json", {"images": images, "sources": sources})
    wire_source = snapshot_path(bundle, "freight") / "runtime-contracts"
    manifest = capture_wire_source(ROOT, wire_source)
    write(bundle / "wire-source.json", {"manifest": manifest})
    env = dict(
        os.environ,
        CARGO_INCREMENTAL="0",
        CARGO_BUILD_JOBS="2",
        RUSTC_WRAPPER="",
        CARGO_TARGET_DIR=str(args.build_cache.resolve()),
        CARGO_PROFILE_DEV_DEBUG="0",
    )
    with (bundle / "wire-build.log").open("xb") as out:
        result = subprocess.run(
            [
                "cargo",
                "build",
                "--offline",
                "--locked",
                "--manifest-path",
                str(wire_source / "Cargo.toml"),
                "--bin",
                "stateset-freight-runtime-wire",
            ],
            env=env,
            stdout=out,
            stderr=subprocess.STDOUT,
            timeout=2400,
        )
    require(result.returncode == 0, "Wire build failed; retain log and original locks")
    for name, digest in manifest.items():
        require(sha(wire_source / name) == digest, "Wire source changed")
    shutil.copyfile(
        args.build_cache / "debug/stateset-freight-runtime-wire", bundle / "wire"
    )
    (bundle / "wire").chmod(0o755)
    write(
        bundle / "wire.json",
        {
            "binary": {
                "path": str(bundle / "wire"),
                "sha256": sha(bundle / "wire"),
                "bytes": (bundle / "wire").stat().st_size,
            },
            "source_manifest": manifest,
        },
    )
    verify(bundle)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--freight-verification", type=Path)
    parser.add_argument("--freight-source", type=Path)
    parser.add_argument("--edi-repo", type=Path, default=ROOT.parent / "stateset-edi")
    parser.add_argument(
        "--sync-repo", type=Path, default=ROOT.parent / "stateset-sync-server"
    )
    parser.add_argument(
        "--build-cache", type=Path, default=Path("/tmp/freight-runtime-target")
    )
    args = parser.parse_args()
    args.bundle = args.bundle.resolve()
    args.build_cache = args.build_cache.resolve()
    os.umask(0o077)
    if args.verify_only:
        verify(args.bundle)
    else:
        require(
            args.freight_verification and args.freight_source,
            "Supply source-bound Freight verification and input manifest",
        )
        prepare(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
