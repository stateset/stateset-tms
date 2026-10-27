"""Path containment regressions for the local recovery bundle preparer.

Run with python3 -m unittest discover -s scripts -p test_prepare_container_recovery.py.
These tests use private temporary files and never start Docker or application services.
"""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "prepare_container_recovery",
    Path(__file__).with_name("prepare-container-recovery.py"),
)
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class ManifestContainment(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="freight-recovery-paths-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name).resolve()
        self.root = self.directory / "source"
        self.root.mkdir()
        (self.root / "Cargo.toml").write_text('[package]\nname = "owned-fixture"\n')
        self.outside = self.directory / "outside.txt"
        self.outside.write_text("owned outside canary\n")

    def metadata(self, manifest=None, evidence=None):
        source = prepare.fingerprint(self.root)
        files = manifest or {"Cargo.toml": prepare.sha(self.root / "Cargo.toml")}
        record = self.directory / "verification.json"
        record.write_text(
            json.dumps(
                {
                    "result": "pass",
                    "source": source,
                    "evidence": evidence or {},
                    "images": {"api": "unused", "web": "unused", "postgres": "unused"},
                }
            )
        )
        inputs = self.directory / "inputs.json"
        inputs.write_text(json.dumps({"source": source, "manifest": files}))
        return SimpleNamespace(
            bundle=self.directory / "new-bundle",
            freight_verification=record,
            freight_source=inputs,
        )

    def refuse_capture(self, args, reason):
        with patch.object(prepare, "ROOT", self.root):
            with patch.object(prepare.subprocess, "run") as docker:
                with self.assertRaisesRegex(AssertionError, reason):
                    prepare.prepare(args)
        docker.assert_not_called()
        self.assertFalse(args.bundle.exists())
        self.assertEqual(self.outside.read_text(), "owned outside canary\n")

    def test_regular_nested_file_is_accepted(self):
        (self.root / "nested").mkdir()
        file = self.root / "nested/input"
        file.write_text("owned nested input")
        self.assertEqual(prepare.manifest_file(self.root, "nested/input"), file)

    def test_unsafe_names_are_refused(self):
        for name in [
            "../outside.txt",
            str(self.outside),
            "",
            ".",
            "./Cargo.toml",
            "nested/../Cargo.toml",
            "nested//input",
            "bad\0name",
            None,
            0,
        ]:
            with self.subTest(name=name):
                with self.assertRaisesRegex(AssertionError, "Unsafe manifest path"):
                    prepare.manifest_file(self.root, name)

    def test_file_and_directory_symlinks_are_refused(self):
        (self.root / "file-link").symlink_to(self.outside)
        (self.root / "directory-link").symlink_to(
            self.directory, target_is_directory=True
        )
        for name in ["file-link", "directory-link/outside.txt"]:
            with self.subTest(name=name):
                with self.assertRaisesRegex(
                    AssertionError, "Snapshot symlinks require explicit handling"
                ):
                    prepare.manifest_file(self.root, name)

    def test_aliased_snapshot_root_is_refused(self):
        alias = self.directory / "root-link"
        alias.symlink_to(self.root, target_is_directory=True)
        with self.assertRaisesRegex(
            AssertionError, "Snapshot root must be a canonical"
        ):
            prepare.manifest_file(alias, "Cargo.toml")

    def test_directory_is_not_a_manifest_file(self):
        (self.root / "directory").mkdir()
        with self.assertRaisesRegex(AssertionError, "Manifest input must be a regular"):
            prepare.manifest_file(self.root, "directory")

    def test_capture_refuses_traversal_before_any_output_or_build(self):
        args = self.metadata({"../outside.txt": prepare.sha(self.outside)})
        self.refuse_capture(args, "Unsafe manifest path")

    def test_capture_refuses_symlink_before_any_output_or_build(self):
        (self.root / "file-link").symlink_to(self.outside)
        args = self.metadata({"file-link": prepare.sha(self.outside)})
        self.refuse_capture(args, "Snapshot symlinks require explicit handling")

    def test_capture_refuses_escaped_evidence_before_any_output_or_build(self):
        args = self.metadata(
            evidence={
                "escaped": {
                    "path": "../outside.txt",
                    "sha256": prepare.sha(self.outside),
                    "bytes": self.outside.stat().st_size,
                }
            }
        )
        self.refuse_capture(args, "Unsafe manifest path")

    def test_frozen_controller_imports_with_its_complete_runtime_dependencies(self):
        # Functional manifests can predate the optional native backend. The
        # controller must capture its Python dependencies independently.
        scripts = self.root / 'scripts'
        scripts.mkdir()
        for name in prepare.CONTROLLER_INPUTS:
            (scripts / Path(name).name).write_bytes(Path(__file__).with_name(Path(name).name).read_bytes())
        args = self.metadata({name: prepare.sha(self.root / name) for name in
                              ('Cargo.toml', 'scripts/runtime_contracts.py', 'scripts/qualify-recovery.py')})
        args.sync_repo = self.directory / 'sync'
        args.edi_repo = self.directory / 'edi'
        for repo in (args.sync_repo, args.edi_repo):
            repo.mkdir()
            (repo / 'Cargo.toml').write_text('[package]\nname="synthetic-sibling"\n')
        with patch.object(prepare, 'ROOT', self.root), \
                patch.object(prepare, 'SELECTION', {'edi': ['Cargo.toml'], 'sync': ['Cargo.toml']}), \
                patch.object(prepare.subprocess, 'run', side_effect=RuntimeError('stop before image build')):
            with self.assertRaisesRegex(RuntimeError, 'stop before image build'):
                prepare.prepare(args)
        snapshot = prepare.snapshot_path(args.bundle, 'freight')
        # Use a fresh isolated interpreter so existing imports cannot hide a
        # dependency missing from the frozen bundle.
        code = '''import sys, shutil, importlib.util
from types import SimpleNamespace
sys.path.insert(0, sys.argv[1])
from runtime_contracts import Runtime
for i, name in enumerate(["qualify-container-recovery.py", "qualify-restricted-recovery.py", "qualify-tls-recovery.py"]):
    spec = importlib.util.spec_from_file_location("frozen_controller_" + str(i), sys.argv[1] + "/" + name)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    if hasattr(module, "required_recovery_checks"):
        assert sum(module.required_recovery_checks().values()) == 58
r = Runtime(SimpleNamespace())
assert len(r.report["harness_sources"]) == 2
shutil.rmtree(r.directory)
'''
        result = subprocess.run([sys.executable, '-I', '-c', code, str(snapshot / 'scripts')],
                                env={'PATH': os.defpath}, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in prepare.CONTROLLER_INPUTS:
            self.assertEqual((snapshot / name).read_bytes(), (self.root / name).read_bytes())


class WireCaptureTests(unittest.TestCase):
    def source(self, root):
        for name in ["Cargo.toml", "Cargo.lock", "src/main.rs"]:
            path = root / "runtime-contracts" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("Original wire input: " + name)

    def test_capture_preserves_source_read_and_execute_permissions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.source(root)
            modes = {'Cargo.toml': 0o640, 'Cargo.lock': 0o750, 'src/main.rs': 0o600}
            for name, mode in modes.items():
                (root / 'runtime-contracts' / name).chmod(mode)
            target = root / 'snapshot/runtime-contracts'
            prepare.capture_wire_source(root, target)
            self.assertEqual({name: (target / name).stat().st_mode & 0o777 for name in modes}, modes)

    def test_mode_only_conflict_retains_captured_bytes_inode_and_permissions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.source(root)
            source = root / 'runtime-contracts/Cargo.toml'
            source.chmod(0o640)
            target = root / 'snapshot/runtime-contracts'
            prepare.capture_wire_source(root, target)
            captured = target / 'Cargo.toml'
            before = (captured.read_bytes(), captured.stat().st_ino,
                      captured.stat().st_mtime_ns, captured.stat().st_mode)
            source.chmod(0o600)
            with self.assertRaisesRegex(AssertionError, 'conflicts'):
                prepare.capture_wire_source(root, target)
            self.assertEqual((captured.read_bytes(), captured.stat().st_ino,
                              captured.stat().st_mtime_ns, captured.stat().st_mode), before)

    def test_new_and_already_captured_inputs_preserve_bytes_inodes_and_times(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.source(root)
            target = root / "snapshot/runtime-contracts"
            first = prepare.capture_wire_source(root, target)
            before = {
                name: (
                    (target / name).read_bytes(),
                    (target / name).stat().st_ino,
                    (target / name).stat().st_mtime_ns,
                )
                for name in first
            }
            self.assertEqual(prepare.capture_wire_source(root, target), first)
            after = {
                name: (
                    (target / name).read_bytes(),
                    (target / name).stat().st_ino,
                    (target / name).stat().st_mtime_ns,
                )
                for name in first
            }
            self.assertEqual(before, after)

    def test_conflicting_captured_input_is_retained_and_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.source(root)
            target = root / "snapshot/runtime-contracts"
            target.mkdir(parents=True)
            conflict = target / "Cargo.toml"
            conflict.write_text("Retain this failed snapshot")
            before = conflict.stat()
            with self.assertRaisesRegex(AssertionError, "conflicts"):
                prepare.capture_wire_source(root, target)
            self.assertEqual(conflict.read_text(), "Retain this failed snapshot")
            self.assertEqual(conflict.stat().st_ino, before.st_ino)
            self.assertEqual(conflict.stat().st_mtime_ns, before.st_mtime_ns)

    def test_destination_and_source_symlinks_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            self.source(root)
            outside = root / "outside"
            outside.mkdir()
            target = root / "linked"
            target.symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(AssertionError, "symlink"):
                prepare.capture_wire_source(root, target)
            with self.assertRaisesRegex(AssertionError, "symlink"):
                prepare.capture_wire_source(root, target / "nested")
            self.assertFalse((outside / "nested").exists())
            source = root / "runtime-contracts/Cargo.toml"
            source.unlink()
            source.symlink_to(root / "runtime-contracts/Cargo.lock")
            with self.assertRaisesRegex(AssertionError, "symlink"):
                prepare.capture_wire_source(root, root / "snapshot")


if __name__ == "__main__":
    unittest.main()
