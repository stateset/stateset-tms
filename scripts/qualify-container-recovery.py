#!/usr/bin/env python3
"""Run the inherited actual-service restore drill with all application services in images.

The bundle contains source/input manifests, immutable image IDs and the actual
runtime wire helper. Only fresh labelled containers, volumes and loopback ports
are accepted; no deployment database URL or ambient application config is used.
"""

import argparse
import base64
import copy
import datetime as dt
from decimal import Decimal
import hashlib
import http.cookiejar
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request
from urllib.parse import urlencode
import uuid

from runtime_contracts import ADMIN, ROOT, NoRedirect, fingerprint, require

spec = importlib.util.spec_from_file_location(
    "container_recovery_base", ROOT / "scripts/qualify-recovery.py"
)
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)

INTERNAL = {"postgres": 5432, "sync": 8080, "edi": 8480, "freight": 8090, "web": 3010}


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class ContainerProcess:
    """The existing drill's process lifecycle, backed by an exact owned container."""

    def __init__(self, owner, name, identity, generation):
        self.owner, self.name, self.identity = owner, name, identity
        self.generation, self.saved = generation, False

    def inspect(self):
        item = self.owner.inspect_container(self.identity)
        require(
            item["Image"] == self.owner.images[self.name], "Application image changed"
        )
        return item

    def poll(self):
        item = self.inspect()
        return None if item["State"]["Running"] else int(item["State"]["ExitCode"])

    def send_signal(self, value):
        self.inspect()
        self.owner.command(
            ["docker", "kill", "--signal", signal.Signals(value).name, self.identity],
            label="owned application signal",
        )

    def terminate(self):
        self.send_signal(signal.SIGTERM)

    def kill(self):
        self.send_signal(signal.SIGKILL)

    def wait(self, timeout=None):
        code = int(
            self.owner.command(
                ["docker", "wait", self.identity],
                label="owned application exit",
                timeout=timeout or 60,
            )
        )
        if not self.saved:
            result = subprocess.run(
                [
                    "docker",
                    "--host",
                    "unix:///var/run/docker.sock",
                    "logs",
                    self.identity,
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                timeout=30,
                env={"PATH": os.defpath},
            )
            require(result.returncode == 0, "Owned application logs unavailable")
            log = result.stdout
            path = self.owner.directory / f"{self.name}-{self.generation}.log"
            with path.open("x") as stream:
                stream.write(log + "\n")
            with (self.owner.directory / f"{self.name}.log").open("a") as stream:
                stream.write(log + "\n")
            self.saved = True
        return code


class ContainerNamespaceRuntime(base.Runtime):
    """Keep the inherited restore's super().postgres() on the image adapter.

    The base recovery assertions are unchanged; both cluster generations use
    the same owned namespace and volume provisioning implementation.
    """

    def postgres(self):
        return self.container_postgres()


class ContainerRecovery(base.Recovery, ContainerNamespaceRuntime):
    def __init__(self, args):
        super().__init__(args)
        self.bundle = args.bundle
        self.source = json.loads((self.bundle / "source.json").read_text())
        require(
            fingerprint(ROOT) == self.source["sources"]["freight"],
            "Current Freight source differs from image bundle",
        )
        self.source["snapshots"] = {
            name: str(self.bundle / "workspace" / directory)
            for name, directory in {
                "freight": "stateset-freight",
                "edi": "stateset-edi",
                "sync": "stateset-sync-server",
            }.items()
        }
        image_record = json.loads((self.bundle / "images.json").read_text())
        require(
            image_record["sources"] == self.source["sources"],
            "Image/source bundle mismatch",
        )
        self.images = image_record["images"]
        self.wire = json.loads((self.bundle / "wire.json").read_text())
        self.apps, self.removed = [], set()
        self.generations, self.environment_hashes = {}, {}
        self.pg_generation, self.edi_volume = -1, None
        self.volume_checkpoint, self.volume_archive = None, None
        self.losing = False
        self.ports["web"] = self.free_web_port()
        self.urls["web"] = f"http://127.0.0.1:{self.ports['web']}"
        self.report["scope"] = (
            "actual Freight/EDI/Sync/web image lifecycle, coordinated fresh-cluster and EDI-volume restore; inherited business assertions retained"
        )
        self.report["network_model"] = (
            "Private shared container network namespace, like a local pod; services use internal loopback and original image health ports. Controller ports publish only on127.0.0.1. No ingress/TLS/network-policy qualification."
        )
        self.report["harness"]["scripts/qualify-container-recovery.py"] = sha(__file__)
        shutil.copyfile(__file__, self.directory / Path(__file__).name)
        self.report["images"] = self.images
        self.report["container_generations"] = []
        self.report["ownership_label"] = self.container_label
        self.report["postgres_container_generations"] = []
        self.report["limitations"] += [
            "Original keys/configuration and image cache survive in the controller; this is not lost-host/key/role or off-host recovery.",
            "EDI PostgreSQL storage is exercised. A labelled file canary additionally verifies volume bytes, uid/gid, modes and nanosecond mtimes; no other filesystem backend is certified.",
        ]

    def free_web_port(self):
        from runtime_contracts import free_port

        while True:
            port = free_port()
            if port not in self.ports.values():
                return port

    def inspect_container(self, identity):
        item = json.loads(
            super().command(
                ["docker", "inspect", identity], label="owned container inspection"
            )
        )[0]
        require(
            item["Config"]["Labels"].get("com.stateset.freight.runtime")
            == self.container_label,
            "Refusing an unowned container",
        )
        return item

    def command(self, command, **kwargs):
        if command[:2] == ["docker", "stop"] and command[-1] == self.container:
            self.remove_apps_and_volume()
        return super().command(command, **kwargs)

    def stop(self, name):
        process = self.processes.pop(name)
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
        else:
            process.wait(timeout=5)

    def build(self):
        self.binaries = {}
        for name in ["freight", "edi", "sync"]:
            snapshot = Path(self.source["snapshots"][name])
            require(
                fingerprint(snapshot) == self.source["sources"][name],
                "Frozen source changed",
            )
            for relative, digest in self.source["inputs"][name].items():
                require(
                    sha(snapshot / relative) == digest, "Frozen build input changed"
                )
            require(
                self.command(
                    [
                        "docker",
                        "image",
                        "inspect",
                        self.images[name],
                        "--format",
                        "{{.Id}}",
                    ],
                    label="exact service image",
                )
                == self.images[name],
                "Image identity mismatch",
            )
            identity = self.command(
                [
                    "docker",
                    "create",
                    "--label",
                    "com.stateset.freight.runtime=" + self.container_label,
                    self.images[name],
                ],
                label="owned binary extraction container",
            )
            binary = self.directory / (name + "-image-binary")
            try:
                filename = {
                    "freight": "stateset-freight",
                    "edi": "stateset-edi",
                    "sync": "stateset-sync-server",
                }[name]
                self.command(
                    [
                        "docker",
                        "cp",
                        identity + ":/usr/local/bin/" + filename,
                        str(binary),
                    ],
                    label="actual image binary pin",
                )
            finally:
                self.inspect_container(identity)
                self.command(
                    ["docker", "rm", "--volumes", identity],
                    label="owned extraction cleanup",
                )
            self.binaries[name] = binary
            self.report["builds"][name] = {
                "source": self.source["sources"][name],
                "binary_sha256": sha(binary),
                "image": self.images[name],
            }
        self.binaries["wire"] = self.bundle / "wire"
        require(
            sha(self.binaries["wire"]) == self.wire["binary"]["sha256"],
            "Wire helper changed",
        )
        for path, digest in self.wire["source_manifest"].items():
            require(
                sha(
                    Path(self.source["snapshots"]["freight"])
                    / "runtime-contracts"
                    / path
                )
                == digest,
                "Wire source changed",
            )
        self.report["builds"]["wire"] = {
            "source": fingerprint(
                Path(self.source["snapshots"]["freight"]) / "runtime-contracts"
            ),
            "binary_sha256": sha(self.binaries["wire"]),
        }
        self.save()

    def container_postgres(self):
        self.pg_generation += 1
        image = self.images["postgres"]
        command = [
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
        ]
        for name, internal in INTERNAL.items():
            command += ["-p", f"127.0.0.1:{self.ports[name]}:{internal}"]
        self.container = self.command(
            command + ["-d", image], label="owned shared-namespace PostgreSQL"
        )
        deadline = time.monotonic() + 30
        while True:
            try:
                if self.sql("postgres", "SELECT current_user") == "freight_runtime":
                    break
            except RuntimeError:
                pass
            require(time.monotonic() < deadline, "PostgreSQL startup timed out")
            time.sleep(0.2)
        self.report["postgres"] = {
            "image_id": image,
            "version": self.sql("postgres", "SHOW server_version"),
        }
        self.report["postgres_container_generations"].append(
            {
                "generation": self.pg_generation,
                "id": self.container,
                "system_identifier": self.sql(
                    "postgres", "SELECT system_identifier FROM pg_control_system()"
                ),
                "image": image,
            }
        )
        for database in base.DATABASES:
            self.sql("postgres", "CREATE DATABASE " + database)
        self.edi_volume = (
            f"freight-recovery-{self.container_label}-edi-{self.pg_generation}"
        )
        self.command(
            [
                "docker",
                "volume",
                "create",
                "--label",
                "com.stateset.freight.runtime=" + self.container_label,
                self.edi_volume,
            ],
            label="owned EDI volume",
        )
        self.step("isolated PostgreSQL ready")

    def environment(self, name, config):
        result = copy.deepcopy(config)
        for key, value in result.items():
            value = value.replace(
                f"@127.0.0.1:{self.ports['postgres']}/", "@127.0.0.1:5432/"
            )
            if name != "web":
                for service in ["freight", "edi", "sync"]:
                    value = value.replace(
                        self.urls[service], f"http://127.0.0.1:{INTERNAL[service]}"
                    )
            result[key] = value
        if name == "sync":
            app = json.loads(result["STATESET_APP_CONFIG_JSON"])
            app["server"].update(host="0.0.0.0", port=8080)
            result["STATESET_APP_CONFIG_JSON"] = json.dumps(app)
            result["STATESET_MIGRATIONS_PATH"] = (
                "/usr/local/share/stateset-sync-server/migrations"
            )
        if name == "edi":
            result.update(EDI_BIND_ADDR="0.0.0.0:8480", EDI_DATA_DIR="/data")
        if name == "freight":
            result["FREIGHT_BIND_ADDR"] = "0.0.0.0:8090"
        return {"RUST_LOG": "warn", **result}

    def spawn(self, name, config):
        if (
            self.pg_generation > 0
            and self.volume_checkpoint is not None
            and not self.report.get("edi_volume_restored")
        ):
            require(not self.volume_manifest(), "Fresh EDI volume is not empty")
            self.validate_volume_backup(self.volume_archive)
            self.volume_tool(
                [
                    "tar",
                    "--same-owner",
                    "--acls",
                    "--xattrs",
                    "-C",
                    "/data",
                    "-xf",
                    "-",
                ],
                input_path=self.volume_archive,
            )
            require(
                self.volume_manifest(include_root=True) == self.volume_checkpoint,
                "Restored EDI volume metadata/bytes differ",
            )
            self.report["edi_volume_restored"] = True
            self.step(
                "fresh EDI volume restores original bytes, uid/gid, modes and nanosecond mtimes"
            )
        if name != "web":
            require(
                sha(self.binaries[name])
                == self.report["builds"][name]["binary_sha256"],
                "Image binary pin changed",
            )
        self.configs[name] = copy.deepcopy(config)
        env = self.environment(name, config)
        digest = base.digest(env)
        if name in self.environment_hashes and not (
            name == "freight"
            and config.get("FREIGHT_DEMO") == "false"
            and self.generations[name] == 0
        ):
            require(
                self.environment_hashes[name] == digest,
                "Actual container configuration changed",
            )
        self.environment_hashes[name] = digest
        generation = self.generations.get(name, -1) + 1
        self.generations[name] = generation
        command = [
            "docker",
            "run",
            "-d",
            "--pull=never",
            "--name",
            f"freight-runtime-{self.container_label}-{name}-{generation}",
            "--label",
            "com.stateset.freight.runtime=" + self.container_label,
            "--network",
            "container:" + self.container,
        ]
        if name == "edi":
            command += [
                "--mount",
                "type=volume,source=" + self.edi_volume + ",target=/data",
            ]
        for key, value in env.items():
            command += ["-e", key + "=" + value]
        identity = self.command(
            command + [self.images[name]], label="owned actual application container"
        )
        process = ContainerProcess(self, name, identity, generation)
        self.processes[name] = process
        self.apps.append(process)
        state = process.inspect()
        require(
            state["HostConfig"]["NetworkMode"] == "container:" + self.container,
            "Application escaped owned namespace",
        )
        uid = self.command(
            ["docker", "exec", identity, "id", "-u"],
            label="actual nonroot application uid",
        )
        require(uid != "0", "Application ran as root")
        if name == "edi":
            self.edi_uid = int(uid)
            self.edi_gid = int(
                self.command(
                    ["docker", "exec", identity, "id", "-g"], label="actual EDI gid"
                )
            )
            self.command(
                ["docker", "exec", identity, "test", "-w", "/data"],
                label="actual EDI user owns writable data volume",
            )
        self.report["container_generations"].append(
            {
                "service": name,
                "generation": generation,
                "id": identity,
                "image": self.images[name],
                "uid": uid,
                "actual_environment_sha256": digest,
                "postgres_generation": self.pg_generation,
            }
        )
        self.save()

    def interrupt(self, name):
        super().interrupt(name)
        if name == "edi" and self.pg_generation == 0:
            archive = self.directory / "edi-volume-initial.tar"
            self.capture_volume(archive)
            self.extract_mirror(archive, self.directory / "edi-data")

    def volume_tool(self, command, input_path=None, output_path=None):
        metadata = json.loads(
            super().command(
                ["docker", "volume", "inspect", self.edi_volume],
                label="owned volume inspection",
            )
        )[0]
        require(
            metadata["Labels"].get("com.stateset.freight.runtime")
            == self.container_label,
            "Refusing an unowned volume",
        )
        args = [
            "docker",
            "--host",
            "unix:///var/run/docker.sock",
            "run",
            "--rm",
            "--pull=never",
            "--network",
            "none",
            "--user",
            "0:0",
            "--label",
            "com.stateset.freight.runtime=" + self.container_label,
            "--mount",
            # Helpers must not initialize an empty volume from their image.
            # https://docs.docker.com/engine/storage/volumes/#options-for---mount
            "type=volume,source=" + self.edi_volume + ",target=/data,volume-nocopy",
            "--entrypoint",
            command[0],
        ]
        if input_path is not None:
            args.append("-i")
        args += [self.images["postgres"], *command[1:]]
        incoming = Path(input_path).open("rb") if input_path else None
        outgoing = Path(output_path).open("xb") if output_path else subprocess.PIPE
        try:
            result = subprocess.run(
                args,
                stdin=incoming,
                stdout=outgoing,
                stderr=subprocess.PIPE,
                env={"PATH": os.defpath, "LC_ALL": "C"},
                timeout=60,
            )
            require(
                result.returncode == 0,
                "Owned volume tool failed: "
                + result.stderr.decode(errors="replace")[-1500:],
            )
            return result.stdout
        finally:
            if incoming:
                incoming.close()
            if output_path:
                outgoing.close()

    def capture_volume(self, destination):
        self.volume_tool(
            [
                "tar",
                "--format=pax",
                "--pax-option=delete=atime,delete=ctime",
                "--numeric-owner",
                "--acls",
                "--xattrs",
                "-C",
                "/data",
                "-cf",
                "-",
                ".",
            ],
            output_path=destination,
        )

    def archive_manifest(self, data):
        result = {}
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
            for item in archive.getmembers():
                require(
                    (item.isfile() or item.isdir())
                    and not Path(item.name).is_absolute()
                    and ".." not in Path(item.name).parts,
                    "Unsafe volume archive entry",
                )
                name = str(Path(item.name))
                entry = {
                    "kind": "file" if item.isfile() else "directory",
                    "mode": item.mode,
                    "uid": item.uid,
                    "gid": item.gid,
                    "mtime_ns": int(
                        Decimal(item.pax_headers.get("mtime", str(item.mtime)))
                        * 1000000000
                    ),
                }
                if item.isfile():
                    entry.update(
                        bytes=item.size,
                        sha256=hashlib.sha256(
                            archive.extractfile(item).read()
                        ).hexdigest(),
                    )
                result[name] = entry
        return result

    def volume_manifest(self, include_root=False):
        data = self.volume_tool(
            [
                "tar",
                "--format=pax",
                "--pax-option=delete=atime,delete=ctime",
                "--numeric-owner",
                "-C",
                "/data",
                "-cf",
                "-",
                ".",
            ]
        )
        return {
            name: value
            for name, value in self.archive_manifest(data).items()
            if include_root or name != "."
        }

    def extract_mirror(self, archive_path, destination):
        require(not destination.exists(), "Preserve prior EDI mirror")
        data = archive_path.read_bytes()
        self.archive_manifest(data)
        destination.mkdir()
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
            archive.extractall(destination)
            for item in sorted(
                archive.getmembers(),
                key=lambda value: len(Path(value.name).parts),
                reverse=True,
            ):
                ns = int(
                    Decimal(item.pax_headers.get("mtime", str(item.mtime))) * 1000000000
                )
                os.utime(destination / item.name, ns=(ns, ns))

    def validate_volume_backup(self, path):
        require(
            path.parent == self.directory / "backups"
            and path.is_file()
            and not path.is_symlink(),
            "EDI volume backup must be an owned regular file",
        )
        expected = self.report["edi_volume_archive"]
        require(
            path.stat().st_size == expected["bytes"]
            and sha(path) == expected["sha256"],
            "EDI volume archive integrity failure",
        )
        require(
            self.archive_manifest(path.read_bytes()) == self.volume_checkpoint,
            "EDI volume archive manifest changed",
        )

    def validate_backups(self, backups):
        super().validate_backups(backups)
        if self.pg_generation == 0 and self.volume_checkpoint is None:
            canary = self.directory / "edi-data/owned-recovery-canary.bin"
            path = self.directory / "edi-canary-transfer.tar"
            with path.open("xb") as stream:
                with tarfile.open(
                    fileobj=stream, mode="w", format=tarfile.PAX_FORMAT
                ) as archive:
                    item = tarfile.TarInfo(canary.name)
                    info = canary.stat()
                    item.uid, item.gid = self.edi_uid, self.edi_gid
                    item.mode = 0o600
                    item.size = info.st_size
                    item.pax_headers["mtime"] = str(
                        Decimal(info.st_mtime_ns) / 1000000000
                    )
                    with canary.open("rb") as data:
                        archive.addfile(item, data)
            self.volume_tool(
                ["tar", "--same-owner", "-C", "/data", "-xf", "-"], input_path=path
            )
            self.volume_archive = self.directory / "backups/edi-volume.tar"
            self.capture_volume(self.volume_archive)
            self.volume_checkpoint = self.archive_manifest(
                self.volume_archive.read_bytes()
            )
            expected = self.files_before
            require(
                set(expected) == set(self.volume_checkpoint) - {"."},
                "Volume/mirror inventory mismatch",
            )
            for name, item in expected.items():
                actual = self.volume_checkpoint[name]
                require(
                    item["mode"] == actual["mode"] and item["kind"] == actual["kind"],
                    "Volume/mirror mode/type mismatch",
                )
                if item["kind"] == "file":
                    require(
                        item["sha256"] == actual["sha256"]
                        and item["mtime_ns"] == actual["mtime_ns"],
                        "Volume/mirror bytes/mtime mismatch",
                    )
            self.report["edi_volume_checkpoint"] = self.volume_checkpoint
            self.report["edi_volume_archive"] = {
                "sha256": sha(self.volume_archive),
                "bytes": self.volume_archive.stat().st_size,
            }
            damaged = self.directory / "backups/corrupt-edi-volume.tar"
            shutil.copyfile(self.volume_archive, damaged)
            with damaged.open("r+b") as stream:
                stream.seek(-1, 2)
                value = stream.read(1)
                stream.seek(-1, 2)
                stream.write(bytes([value[0] ^ 1]))
            try:
                self.validate_volume_backup(damaged)
            except AssertionError as error:
                require(
                    str(error) == "EDI volume archive integrity failure",
                    "Volume rejection failed for another reason",
                )
            else:
                raise AssertionError("Corrupt EDI volume archive was accepted")
            require(
                self.volume_manifest(include_root=True) == self.volume_checkpoint,
                "Rejected volume input changed originals",
            )
            self.step(
                "corrupt EDI-volume archive is refused before restore or original volume loss"
            )

    def remove_apps_and_volume(self):
        for process in self.apps:
            if process.identity in self.removed:
                continue
            require(
                process.poll() is not None,
                "Refusing database loss with live application container",
            )
            process.wait(timeout=5)
            self.command(
                ["docker", "rm", "--volumes", process.identity],
                label="owned application removal",
            )
            self.removed.add(process.identity)
        if self.edi_volume:
            if self.losing:
                require(
                    self.volume_archive is not None
                    and sha(self.volume_archive)
                    == self.report["edi_volume_archive"]["sha256"],
                    "Cannot lose unverified EDI volume",
                )
            metadata = json.loads(
                super().command(
                    ["docker", "volume", "inspect", self.edi_volume],
                    label="owned volume removal identity",
                )
            )[0]
            require(
                metadata["Labels"].get("com.stateset.freight.runtime")
                == self.container_label,
                "Refusing unowned volume removal",
            )
            self.command(
                ["docker", "volume", "rm", self.edi_volume],
                label="owned EDI volume removal",
            )
            self.edi_volume = None

    def backup_and_lose(self):
        self.interrupt("web")
        self.losing = True
        try:
            super().backup_and_lose()
        finally:
            self.losing = False
        require(self.edi_volume is None, "Original EDI volume survived loss")

    def mutate(self, path, body, key=None, expected=200, token=None):
        if not path.endswith("/portal-offers") or expected != 200:
            return super().mutate(path, body, key, expected, token)
        key = key or str(uuid.uuid4())
        observations = {
            "path": path,
            "key": key,
            "body_sha256": base.digest(body),
            "attempts": [],
        }
        self.report.setdefault("portal_source_retry_observations", []).append(
            observations
        )
        for _ in range(5):
            result = self.http(
                "freight",
                "/v1/" + path,
                method="POST",
                body=body,
                expected=(200, 409),
                token=token,
                headers={"Idempotency-Key": key},
            )
            observations["attempts"].append(
                result.get("error", {"result": "committed"})
            )
            if "data" in result:
                return result["data"]
            require(
                result
                == {"error": {"message": "Sync source check changed; retry refresh"}},
                "Refuse every other conflict/uncertain outcome",
            )
            time.sleep(0.2)
        raise AssertionError("Source observation did not stabilize")

    def approval_records(self):
        workspace = self.workspace()
        events = {item["id"]: item for item in workspace["events"]}
        records = {}
        for invoice in workspace["invoices"]:
            evidence = invoice["approval_control"].get("approval_evidence")
            if evidence is None:
                continue
            review, decision = (
                events[evidence["entry_review_id"]],
                events[evidence["approval_event_id"]],
            )
            require(
                invoice["status"] == "approved"
                and review["kind"] == "invoice.entry_reviewed"
                and decision["kind"] == "invoice.approve",
                "Original approval kinds",
            )
            require(
                review["entity_id"] == invoice["id"] == decision["entity_id"]
                and decision["details"]["entry_review_id"] == review["id"],
                "Original approval links",
            )
            require(
                evidence["revision"]
                == review["details"]["revision"]
                == decision["details"]["revision"],
                "Original review revision",
            )
            require(
                evidence["entry_reviewed_by"]
                == review["actor"]
                == "runtime:entry-reviewer"
                and evidence["approved_by"]
                == decision["actor"]
                == "runtime:independent-checker",
                "Original independent actors",
            )
            require(
                evidence["entry_review_note"] == review["details"]["note"]
                and evidence["approval_note"] == decision["details"]["note"]
                and evidence["evidence_reference"]
                == review["details"]["evidence_reference"],
                "Original findings/reference",
            )
            records[invoice["id"]] = evidence
        require(records, "Real independently approved invoice required")
        deferred = self.http("freight", "/v1/workspace?history=deferred")
        require(
            {
                i["id"]: i["approval_control"].get("approval_evidence")
                for i in deferred["invoices"]
                if i["id"] in records
            }
            == records,
            "Compatible deferred originals",
        )
        compact = self.http(
            "freight",
            "/v1/workspace?history=deferred",
            headers={"X-Freight-Approval-Details": "deferred"},
        )
        by_id = {invoice["id"]: invoice for invoice in compact["invoices"]}
        for identity, evidence in records.items():
            invoice = by_id[identity]
            summary = {
                key: value
                for key, value in evidence.items()
                if key
                not in ["entry_review_note", "approval_note", "evidence_reference"]
            }
            require(
                invoice["approval_control"]["approval_evidence"] == summary,
                "Exact compact summary",
            )
            selected = self.http(
                "freight", "/v1/invoices/" + invoice["id"] + "/approval-evidence"
            )
            require(
                selected["tenant_id"] == "demo"
                and selected["access"]["subject"] == "runtime:entry-reviewer"
                and selected["invoice_id"] == invoice["id"]
                and selected["approval_evidence"] == records[invoice["id"]],
                "Scoped selected original approval",
            )
        return records

    def checks(self):
        super().checks()
        self.original_approvals = self.approval_records()
        self.report["approval_readback_modes"] = [
            "default_full",
            "history_deferred_compatible",
            "opt_in_compact",
            "selected_original_details",
        ]
        self.report["invoice_approval_evidence"] = {"before": self.original_approvals}
        self.step(
            "actual independently approved invoice retains exact full/deferred/compact/selected evidence"
        )
        self.verify_image_health()

    def verify_image_health(self):
        for name in ["sync", "edi", "freight"]:
            self.wait(
                lambda name=name: self.processes[name]
                .inspect()["State"]
                .get("Health", {})
                .get("Status")
                == "healthy",
                "actual default image health: " + name,
                90,
            )
        require(
            self.web_call("/api/freight/access")["tenant_id"] == "demo",
            "Web gateway unavailable",
        )
        self.step(
            "actual application image healthchecks and protected web gateway pass with original image health configuration",
            postgres_generation=self.pg_generation,
        )

    def verify_save_resolution(self):
        super().verify_save_resolution()
        restored = self.approval_records()
        require(
            restored == self.original_approvals, "Restored original approval changed"
        )
        self.report["invoice_approval_evidence"]["restored"] = restored
        self.step(
            "fresh cluster retains exact original approval actors, findings, references and decisions"
        )
        self.verify_restored_console()

    def console_source_snapshot(self):
        # One PostgreSQL statement gives originals and physical rewrite markers
        # the same snapshot. Text preserves original JSON numbers and file bytes.
        return self.rows("freight_runtime", """
            SELECT json_build_object(
                'workspace',freight_workspace_originals('demo')::text,
                'headers',(SELECT json_agg(r) FROM (
                    SELECT ctid::text AS tuple_id,xmin::text AS row_version,to_jsonb(w)::text AS original
                    FROM freight_workspaces w WHERE tenant_id='demo' ORDER BY tenant_id
                ) r),
                'blocks',(SELECT json_agg(r) FROM (
                    SELECT ctid::text AS tuple_id,xmin::text AS row_version,to_jsonb(b)::text AS original
                    FROM freight_workspace_blocks b WHERE tenant_id='demo' ORDER BY root,key
                ) r),
                'files',(SELECT json_agg(r) FROM (
                    SELECT ctid::text AS tuple_id,xmin::text AS row_version,to_jsonb(f)::text AS original
                    FROM freight_document_content f WHERE tenant_id='demo' ORDER BY document_id
                ) r)
            ) AS snapshot
        """)[0]["snapshot"]

    def verify_restored_console(self):
        # Only derived rows in this owned synthetic database are removed.
        # Keep original image configuration, roles and workers unchanged.
        self.sql("freight_runtime", "DELETE FROM freight_console_state WHERE tenant_id='demo'")
        attempts = self.report.setdefault("console_recovery_observations", [])
        collections = {"billing": ["bills"], "payables": ["settlements"],
                       "integrations": ["outbox", "inbox"]}
        for attempt in range(3):
            before = self.console_source_snapshot()
            source = json.loads(before["workspace"])
            observations = []
            for view, roots in collections.items():
                expected = sum(len(source[name]) for name in roots)
                require(expected > 0, "Restored console fixture must be nonempty: " + view)
                args = {"view": view, "limit": 1}
                first = self.http("freight", "/v1/console?" + urlencode(args))
                selected = [(name, row) for name in roots for row in first[name]]
                require(len(selected) == 1, "Restored console did not return one bounded original")
                name, row = selected[0]
                search = self.http("freight", "/v1/console?" + urlencode(dict(args, q=row["id"])))
                following = None
                if first["next_cursor"] is not None:
                    following = self.http("freight", "/v1/console?" + urlencode(dict(args, cursor=first["next_cursor"])))
                observations.append({"view": view, "expected_total": expected,
                                     "first": first, "search": search, "following": following,
                                     "selected_collection": name, "selected_id": row["id"]})
            after = self.console_source_snapshot()
            record = {"attempt": attempt + 1, "canonical_before_sha256": base.digest(before),
                      "canonical_after_sha256": base.digest(after),
                      "canonical_rows_unchanged": before == after, "views": observations}
            attempts.append(record)
            if before != after:
                # Retain actual concurrent worker changes. Repeat read-only
                # observation, never a business command or failed assertion.
                continue
            cursors = 0
            for observation in observations:
                view = observation["view"]
                roots = collections[view]
                first = observation["first"]
                require(first["tenant_id"] == "demo" and first["view"] == view
                        and first["limit"] == 1 and first["total"] == observation["expected_total"],
                        "Restored console totals differ from canonical originals")
                search = observation["search"]
                hits = [(name, row["id"]) for name in roots for row in search[name]]
                require(search["tenant_id"] == "demo" and search["view"] == view
                        and search["limit"] == 1 and search["total"] >= 1
                        and hits == [(observation["selected_collection"], observation["selected_id"])],
                        "Restored literal search lost its selected original")
                following = observation["following"]
                if following is not None:
                    cursors += 1
                    rows = [(name, row["id"]) for name in roots for row in following[name]]
                    require(following["tenant_id"] == "demo" and following["view"] == view
                            and following["limit"] == 1 and following["total"] == first["total"] and len(rows) == 1
                            and rows != [(observation["selected_collection"], observation["selected_id"])],
                            "Restored keyset cursor repeated or lost a record")
            require(cursors > 0, "Restored console fixture did not exercise a keyset cursor")
            self.step("restored actual image rebuilds nonempty bounded console pages and literal search after cache loss without rewriting canonical originals",
                      views=list(collections), keyset_cursors=cursors,
                      canonical_snapshot_sha256=record["canonical_before_sha256"])
            return
        raise AssertionError("Concurrent canonical writes prevented a stable restored console observation")

    def web_call(self, path, body=None):
        headers = {"Origin": self.urls["web"]}
        data = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(body, separators=(",", ":")).encode()
        request = urllib.request.Request(
            self.urls["web"] + path, data=data, headers=headers
        )
        try:
            with self.web_client.open(request, timeout=10) as response:
                return json.loads(response.read())
        except urllib.error.HTTPError as error:
            detail = self.safe(error.read(4096).decode("utf-8", errors="replace"))
            raise RuntimeError(f"Web {path}: HTTP {error.code}: {detail}") from error

    def start_web(self):
        self.web_config = {
            "FREIGHT_API_URL": "http://127.0.0.1:8090",
            "FREIGHT_DEMO": "false",
            "FREIGHT_WEB_AUTH_MODE": "credential",
            "FREIGHT_WEB_ORIGIN": self.urls["web"],
            "FREIGHT_COOKIE_SECURE": "false",
            "FREIGHT_SESSION_SECRET": "owned-container-recovery-session-secret-at-least-32-characters",
            "FREIGHT_SAVE_JOURNAL_KEYS_JSON": json.dumps(
                {
                    "active": "container-v1",
                    "keys": {
                        "container-v1": base64.urlsafe_b64encode(b"3" * 32)
                        .decode()
                        .rstrip("=")
                    },
                }
            ),
        }
        self.spawn("web", self.web_config)
        self.web_client = urllib.request.build_opener(
            urllib.request.ProxyHandler({}),
            NoRedirect(),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
        )
        self.wait(
            lambda: self.web_call("/api/session", {"token": ADMIN}),
            "actual web session",
            60,
        )
        access = self.web_call("/api/freight/access")
        self.journal_owner = json.dumps(
            ["credential", "", access["tenant_id"], access["access"]["subject"], ""],
            separators=(",", ":"),
        )
        self.journal_intent = {
            "key": "owned-container-sealed-original",
            "path": "customers",
            "body": {
                "name": "Owned recovery original",
                "contact": "owned@example.test",
            },
            "message": "Retain this original through recovery",
            "createdAt": dt.datetime.now(dt.timezone.utc).isoformat(),
        }
        self.original_sealed = self.web_call(
            "/api/freight/save-journal",
            {
                "action": "seal",
                "owner": self.journal_owner,
                "intent": self.journal_intent,
            },
        )["data"]
        self.step(
            "actual web image retains authenticated session and original sealed save before loss"
        )

    def restore(self):
        super().restore()
        self.spawn("web", self.web_config)
        self.wait(
            lambda: self.web_call("/api/freight/access"),
            "restored original web session",
            60,
        )
        access = self.web_call("/api/freight/access")
        require(
            access["tenant_id"] == "demo"
            and access["access"]["subject"] == "runtime:entry-reviewer",
            "Restored web session owner changed",
        )
        opened = self.web_call(
            "/api/freight/save-journal",
            {
                "action": "open",
                "owner": self.journal_owner,
                "sealed": self.original_sealed,
            },
        )["data"]
        require(opened == self.journal_intent, "Restored original sealed save changed")
        self.step(
            "fresh web container accepts original signed session and opens exact original sealed save with retained keys"
        )
        self.verify_image_health()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.bundle = args.bundle.resolve()
    require(not args.output.exists(), "Preserve earlier reports")
    source = json.loads((args.bundle / "source.json").read_text())
    args.sync_repo, args.edi_repo = [
        args.bundle / "workspace" / directory
        for directory in ["stateset-sync-server", "stateset-edi"]
    ]
    args.build_dir, args.postgres_image = args.bundle, "pinned"
    os.umask(0o077)
    run = ContainerRecovery(args)
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
                sha(ROOT / relative) == digest, "Recovery harness changed during run"
            )
        for name in ["freight", "edi", "sync"]:
            require(
                fingerprint(Path(source["snapshots"][name])) == source["sources"][name],
                "Frozen service source drift",
            )
            require(
                sha(run.binaries[name]) == run.report["builds"][name]["binary_sha256"],
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
        with args.output.open("x") as stream:
            stream.write((run.directory / "report.json").read_text())
        print("REPORT", args.output, flush=True)
    return 0 if run.report["result"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
