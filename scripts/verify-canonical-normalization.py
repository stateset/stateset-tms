#!/usr/bin/env python3
"""Check codec compatibility against actual native originals and retained frames.

Byte/value/scale/refusal checks only. CLI timings from a separate component do
not establish the running application's performance or resource requirements.
"""
import argparse
import datetime as dt
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile

from runtime_contracts import ROOT, fingerprint, require

spec = importlib.util.spec_from_file_location("codec_compatibility", ROOT / "scripts/compare-storage-codecs.py")
compatibility = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compatibility)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def binding(path):
    return {"path": str(path.resolve()), "sha256": sha(path)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--preceding-build", type=Path, required=True)
    parser.add_argument("--current-binary", type=Path, required=True)
    parser.add_argument("--native-report", type=Path, default=ROOT / "docs/evidence/native-storage-after.json.gz")
    parser.add_argument("--configurations", type=Path, default=ROOT / "docs/evidence/block-decoder-configurations.json")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(not args.output.exists(), "Preserve preceding evidence")
    directory = Path(tempfile.mkdtemp(prefix="freight-normalization-compatibility-"))
    paths = [Path(__file__).resolve(), ROOT / "scripts/compare-storage-codecs.py", ROOT / "scripts/profile-storage-codec.py", ROOT / "scripts/profile-storage-records.py", ROOT / "scripts/runtime_contracts.py"]
    report = {"result": "running", "scope": __doc__, "started_at": dt.datetime.now(dt.timezone.utc).isoformat(), "source": fingerprint(ROOT), "artifacts": str(directory), "harness": {str(p.relative_to(ROOT)): sha(p) for p in paths}, "corpus": [], "configurations": [], "refusals": [], "source_controls": []}

    def execute(binary, mode, source, label, span=64, target=65536, expected=True):
        stdout, stderr = directory / (label + ".stdout"), directory / (label + ".stderr")
        command = [str(binary), mode] + ([str(span), str(target)] if mode == "pack" else [])
        with source.open("rb") as data, stdout.open("xb") as out, stderr.open("xb") as err:
            p = subprocess.run(command, stdin=data, stdout=out, stderr=err, env={**os.environ, "LC_ALL": "C"})
        require(p.returncode == (0 if expected else 1), "Unexpected codec acceptance/refusal: " + label)
        require(expected or stdout.stat().st_size == 0 and stderr.stat().st_size > 0, "Partial output or unclean refusal: " + label)
        return {"exit_code": p.returncode, "stdout": binding(stdout), "stderr": binding(stderr)}

    def pair(source, label, span, target=65536, golden=None):
        values = {}
        for variant, binary in binaries.items():
            frame = execute(binary, "pack", source, label + "-" + variant + "-pack", span, target)
            values[variant] = {"frame": frame}
        require(values["preceding"]["frame"]["stdout"]["sha256"] == values["current"]["frame"]["stdout"]["sha256"], "Frame bytes changed: " + label)
        old_frame = Path(values["preceding"]["frame"]["stdout"]["path"])
        for variant, binary in binaries.items():
            values[variant]["restored"] = execute(binary, "unpack", old_frame, label + "-" + variant + "-unpack")
        require(values["preceding"]["restored"]["stdout"]["sha256"] == values["current"]["restored"]["stdout"]["sha256"], "Restored bytes changed: " + label)
        compatibility.same_value(compatibility.read_json(source.read_bytes()), compatibility.read_json(Path(values["current"]["restored"]["stdout"]["path"]).read_bytes()))
        if golden:
            for variant in binaries:
                require(values[variant]["frame"]["stdout"]["sha256"] == golden["frame"]["sha256"] and values[variant]["restored"]["stdout"]["sha256"] == golden["restored"]["sha256"], "Historical golden changed")
        return {"span": span, "target_bytes": target, "source": binding(source), "results": values}

    try:
        preceding = json.loads(args.preceding_build.read_text())
        binaries = {"preceding": Path(preceding["binary"]["path"]), "current": args.current_binary.resolve()}
        require(preceding["result"] == "pass" and sha(binaries["preceding"]) == preceding["binary"]["sha256"], "Preceding codec drift")
        report.update(binaries={k: binding(p) for k, p in binaries.items()}, preceding_build=binding(args.preceding_build), native_report=binding(args.native_report), configuration_report=binding(args.configurations))
        run = json.loads(gzip.decompress(args.native_report.read_bytes()))
        require(run["result"] == "measured" and run["no_oom_or_limit_hits"] and run["builds"]["freight"]["source"] == preceding["source"], "Native original source mismatch")
        originals = {}
        for w in run["workloads"]:
            for sample in w["saves"]:
                require(sample["verification"] == "pass", "Unverified native save")
                for name in ["source_before", "source_after"]:
                    archive = sample[name]
                    raw = compatibility.archived(archive)
                    if archive["raw_sha256"] not in originals:
                        source = directory / f"native-{w['shipments']}-{len(originals)}.json"
                        source.write_bytes(raw)
                        originals[archive["raw_sha256"]] = {"source": source, "shipments": w["shipments"], "archives": []}
                    originals[archive["raw_sha256"]]["archives"].append(archive)
        require(len(originals) == 12, "Native corpus cardinality changed")
        for n, original in enumerate(originals.values()):
            for span in [16, 64]:
                report["corpus"].append({**pair(original["source"], f"native-{n}-{span}", span), "shipments": original["shipments"], "archives": original["archives"]})
            print("VERIFIED native original", n, original["shipments"], flush=True)
        matrix = json.loads(args.configurations.read_text())
        require(matrix["result"] == "pass", "Unqualified configuration matrix")
        config_source = Path(matrix["source"]["path"])
        require(sha(config_source) == matrix["source"]["sha256"], "Configuration input drift")
        for n, case in enumerate(matrix["cases"]):
            report["configurations"].append(pair(config_source, "configuration-" + str(n), case["span"], case["target_bytes"], case["results"]["preceding"]))
        raw = compatibility.records.encode({"a": [{"id": "same", "x": False} for _ in range(65)], "r": {f"literal/雪{n}": [None, True, n] for n in range(500)}, "scalar": "literal", "empty": []}).encode()
        source = directory / "refusal-source.json"
        source.write_bytes(raw)
        frame = execute(binaries["preceding"], "pack", source, "refusal-fixture")
        for n, (name, expected, raw) in enumerate(compatibility.corruptions(compatibility.read_json(Path(frame["stdout"]["path"]).read_bytes()))):
            source = directory / (name + ".json")
            source.write_bytes(raw)
            values = {variant: execute(binary, "unpack", source, f"refusal-{n}-{variant}", expected=expected) for variant, binary in binaries.items()}
            if expected:
                require(values["preceding"]["stdout"]["sha256"] == values["current"]["stdout"]["sha256"], "Positive corruption control changed")
            report["refusals"].append({"name": name, "expected_accept": expected, "input": binding(source), "results": values})
        for n, raw in enumerate([b'{"x":{"a":1,"\\u0061":2}}', b'{"x":"\\ud800"}', b'{"x":1e999999999999999999999999}', b'{"x":true} trailing', ('{"x":' + '[' * 256 + '0' + ']' * 256 + '}').encode()]):
            source = directory / f"invalid-source-{n}.json"
            source.write_bytes(raw)
            report["source_controls"].append({"input": binding(source), "results": {variant: execute(binary, "pack", source, f"source-control-{n}-{variant}", expected=False) for variant, binary in binaries.items()}})
        require(fingerprint(ROOT) == report["source"], "Application source changed")
        require(all(sha(ROOT / p) == h for p, h in report["harness"].items()), "Compatibility harness changed")
        require(all(binding(p) == report["binaries"][k] for k, p in binaries.items()), "Binary changed")
        report["result"] = "pass"
    except BaseException as error:
        report.update(result="failed", error=f"{type(error).__name__}: {error}")
        print(report["error"], flush=True)
    finally:
        report["finished_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
        with args.output.open("x") as out:
            out.write(json.dumps(report, indent=2) + "\n")
        print("REPORT", args.output, flush=True)
    return 0 if report["result"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
