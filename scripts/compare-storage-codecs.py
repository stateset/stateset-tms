#!/usr/bin/env python3
"""Compare block codecs against retained native outcomes and corruption cases.

Checks byte-identical frames and decoded outputs, then counterbalances separate
native CLI CPU/RSS observations. No PostgreSQL observer runs here. This does not
qualify application storage, typed Workspace allocation or deployment capacity.
"""
import argparse
import copy
import datetime as dt
from decimal import Decimal
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from runtime_contracts import ROOT, fingerprint, require


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


records = module("record_probe", ROOT / "scripts/profile-storage-records.py")
resources = module("codec_resources", ROOT / "scripts/profile-storage-codec.py")
sha, binding = resources.sha, resources.binding


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "Duplicate original JSON member")
        result[key] = value
    return result


def read_json(raw):
    return json.loads(raw, parse_float=Decimal, object_pairs_hook=unique)


def same_value(a, b):
    require(type(a) is type(b), "Original JSON type changed")
    if isinstance(a, dict):
        require(a.keys() == b.keys(), "Original JSON members changed")
        for key in a:
            same_value(a[key], b[key])
    elif isinstance(a, list):
        require(len(a) == len(b), "Original JSON cardinality changed")
        for x, y in zip(a, b):
            same_value(x, y)
    elif isinstance(a, Decimal):
        require(a.as_tuple() == b.as_tuple(), "Original decimal scale/value changed")
    else:
        require(a == b, "Original JSON value changed")


def archived(value):
    path = Path(value["path"])
    require(sha(path) == value["sha256"], "Archived source drift")
    raw = gzip.decompress(path.read_bytes())
    require(hashlib.sha256(raw).hexdigest() == value["raw_sha256"], "Raw original drift")
    return raw


def checked_pair(binaries, source, span, directory, label, order):
    frames, restored, results = {}, {}, []
    for variant in order:
        frames[variant] = directory / f"{label}-{variant}-frame.json"
        item = resources.measure(binaries[variant], "pack", span, source, frames[variant], directory,
                                 f"{label}-{variant}-pack")
        item.update(variant=variant, span=span)
        results.append(item)
    require(sha(frames["preceding"]) == sha(frames["current"]), "Packed frame changed")
    for variant in reversed(order):
        restored[variant] = directory / f"{label}-{variant}-restored.json"
        # Both decoders consume the exact retained-encoder frame, regardless of
        # encoder output equality, rather than accepting a private new layout.
        item = resources.measure(binaries[variant], "unpack", span, frames["preceding"],
                                 restored[variant], directory, f"{label}-{variant}-unpack")
        item.update(variant=variant, span=span)
        results.append(item)
    require(sha(restored["preceding"]) == sha(restored["current"]), "Decoded output changed")
    same_value(read_json(source.read_bytes()), read_json(restored["current"].read_bytes()))
    return results


def corruptions(frame):
    """Explicit negative cases independent of the codec's implementation."""
    cases = []

    def case(name, change):
        candidate = copy.deepcopy(frame)
        change(candidate)
        cases.append((name, False, records.encode(candidate).encode()))

    case("future_version", lambda p: p.update(version=2))
    case("unsupported_span", lambda p: p["config"].update(span=3))
    case("unsupported_target", lambda p: p["config"].update(target_bytes=0))
    case("unknown_envelope_field", lambda p: p.update(future_layout=True))
    case("unknown_root_field", lambda p: p["roots"]["scalar"].update(future_shape=True))
    case("missing_collection_root", lambda p: p["roots"].pop("a"))
    case("missing_scalar_root", lambda p: p["roots"].pop("scalar"))
    case("missing_root_manifest_entry", lambda p: p["root_names"].pop())
    case("duplicate_root_manifest_entry", lambda p: p["root_names"].append(p["root_names"][0]))
    case("reordered_root_manifest", lambda p: p["root_names"].reverse())
    case("wrong_collection_count", lambda p: p["roots"]["a"].update(size=66))
    case("wrong_scalar_type", lambda p: p["roots"]["scalar"].update(scalar=False))
    case("wrong_scalar_count", lambda p: p["roots"]["scalar"].update(size=1))
    case("collection_scalar_injection", lambda p: p["roots"]["a"].update(scalar="lost"))
    case("missing_block", lambda p: p["blocks"].pop())
    case("duplicate_block", lambda p: p["blocks"].append(copy.deepcopy(p["blocks"][0])))
    case("empty_block", lambda p: p["blocks"][0].update(items=[]))
    case("wrong_address", lambda p: p["blocks"][0].update(key="invalid"))
    case("unknown_block_root", lambda p: p["blocks"][0].update(root="not present"))
    case("zero_array_ordinal", lambda p: p["blocks"][0]["items"][0].update(position=0))
    case("extreme_array_ordinal", lambda p: p["blocks"][0]["items"][0].update(position=2**64-1))
    case("named_array_record", lambda p: p["blocks"][0]["items"][0].update(name="wrong"))
    case("array_item_order", lambda p: p["blocks"][0]["items"].reverse())

    def object_block(p):
        return next(b for b in p["blocks"] if b["root"] == "r" and len(b["items"]) > 1)

    case("object_item_order", lambda p: object_block(p)["items"].reverse())
    case("object_record_position", lambda p: object_block(p)["items"][0].update(position=1))
    case("missing_object_member", lambda p: object_block(p)["items"].pop())

    def duplicate_record(p):
        blocks = [b for b in p["blocks"] if b["root"] == "a"]
        require(len(blocks) == 2, "Expected fixture block boundary")
        blocks[1]["items"].append(copy.deepcopy(blocks[0]["items"][0]))

    case("duplicate_record_across_blocks", duplicate_record)

    def subdivide(p):
        block = p["blocks"][0]
        require(block["root"] == "a" and len(block["items"]) == 64, "Expected splittable fixture")
        new = copy.deepcopy(block)
        new.update(key=block["key"]+"1", items=block["items"][32:])
        block.update(key=block["key"]+"0", items=block["items"][:32])
        p["blocks"].append(new)

    case("unnecessary_subdivision", subdivide)
    raw = records.encode(frame)
    require('"id":"same",' in raw, "Missing duplicate-member target")
    cases.append(("duplicate_payload_json_member", False,
                  raw.replace('"id":"same",', '"id":"same","id":"same",', 1).encode()))
    cases.append(("duplicate_root_json_member", False,
                  raw.replace('"roots":{', '"roots":{"scalar":{"kind":"string","size":0,"scalar":"literal"},', 1).encode()))
    cases.append(("valid_frame", True, raw.encode()))
    p = copy.deepcopy(frame)
    p["blocks"].reverse()
    cases.append(("valid_reordered_blocks", True, records.encode(p).encode()))
    cases.append(("valid_wire_whitespace", True, (" \n"+raw.replace(',"roots":', ',\n "roots":')+"\n ").encode()))
    cases.append(("valid_unicode_escapes", True, raw.replace("雪", r"\u96ea").encode()))
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--preceding-receipt", type=Path, default=ROOT/"docs/evidence/storage-blocks-source.json")
    parser.add_argument("--storage-report", type=Path, default=ROOT/"docs/evidence/storage-blocks-postgres.json")
    parser.add_argument("--current-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repetitions", type=int, choices=range(1, 11), default=3)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Preserve evidence; select a new output path")
    directory = Path(tempfile.mkdtemp(prefix="freight-block-decoder-"))
    paths = [Path(__file__).resolve(), ROOT/"scripts/profile-storage-codec.py",
             ROOT/"scripts/profile-storage-records.py", ROOT/"scripts/runtime_contracts.py"]
    paths += [ROOT/"storage-codec"/name for name in ["Cargo.toml", "Cargo.lock", "src/lib.rs", "src/main.rs"]]
    report = {"result":"running", "scope":__doc__, "started_at":dt.datetime.now(dt.timezone.utc).isoformat(),
              "artifacts":str(directory), "source":fingerprint(ROOT),
              "controllers_and_codec":{str(p.relative_to(ROOT)):binding(p) for p in paths},
              "corpus":[], "refusal_cases":[], "resource_trials":[]}
    try:
        preceding = json.loads(args.preceding_receipt.read_text())
        run = json.loads(args.storage_report.read_text())
        require(preceding["result"] == "pass" and run["result"] == "measured" and run["no_oom_or_limit_hits"],
                "Preceding qualification must pass")
        require(run["source"] == report["source"] == preceding["production_core"], "Production source drift")
        binaries = {"preceding":Path(preceding["native_codec"]["path"]), "current":args.current_binary.resolve()}
        require(sha(binaries["preceding"]) == preceding["native_codec"]["sha256"] == run["native_codec"]["sha256"],
                "Preceding executable drift")
        report.update(preceding_receipt=binding(args.preceding_receipt), storage_report=binding(args.storage_report),
                      binaries={k:binding(p) for k,p in binaries.items()}, repetitions=args.repetitions)
        originals = {}
        initial_sources = {}
        for w in run["workloads"]:
            for sample in w["samples"]:
                for name in ["native_before", "native_after"]:
                    value = sample[name]
                    raw = archived(value)
                    identity = value["raw_sha256"]
                    if identity not in originals:
                        source = directory/f"source-{w['shipments']}-{len(originals)}.json"
                        source.write_bytes(raw)
                        originals[identity] = {"shipments":w["shipments"], "source":binding(source), "archives":[]}
                    originals[identity]["archives"].append(value)
                    initial_sources.setdefault(w["shipments"], Path(originals[identity]["source"]["path"]))
        report["native_save_outcomes_covered"] = sum(len(w["samples"]) for w in run["workloads"])
        for n, original in enumerate(originals.values()):
            source = Path(original["source"]["path"])
            for span in [16, 64]:
                order = ["preceding", "current"] if n%2 == 0 else ["current", "preceding"]
                results = checked_pair(binaries, source, span, directory, f"corpus-{n}-{span}", order)
                report["corpus"].append({**original, "span":span, "pack_order":order, "results":results})
            print("VERIFIED CORPUS", original["shipments"], n, flush=True)

        # Both implementations run in each repetition; startup, process I/O and
        # compiler effects are included. No unrelated sampler runs concurrently.
        for shipments, source in initial_sources.items():
            for repetition in range(args.repetitions):
                for span in ([16,64] if repetition%2 == 0 else [64,16]):
                    order = ["preceding", "current"] if repetition%2 == 0 else ["current", "preceding"]
                    results = checked_pair(binaries, source, span, directory,
                                           f"resources-{shipments}-{span}-{repetition}", order)
                    for result in results:
                        result.update(shipments=shipments, repetition=repetition)
                    report["resource_trials"].extend(results)
                    print("MEASURED", shipments, span, repetition, flush=True)

        fixture = {"a":[{"id":"same", "n":Decimal("1.2300"), "x":False} for _ in range(65)],
                   "r":{f"literal/雪{n}":[None,True,n] for n in range(500)}, "scalar":"literal", "empty":[]}
        raw_source = directory/"refusal-source.json"
        raw_source.write_text(records.encode(fixture))
        fixture_frame = directory/"refusal-frame.json"
        with raw_source.open('rb') as stdin, fixture_frame.open('xb') as stdout:
            result = subprocess.run([str(binaries['preceding']), 'pack', '64', '65536'], stdin=stdin, stdout=stdout, capture_output=False, check=True)
        frame = read_json(fixture_frame.read_bytes())
        for name, expected, raw in corruptions(frame):
            source = directory/(name+".json")
            source.write_bytes(raw)
            outcomes = {}
            for variant, binary in binaries.items():
                output, error = directory/(name+"-"+variant+".stdout"), directory/(name+"-"+variant+".stderr")
                with source.open('rb') as stdin, output.open('xb') as stdout, error.open('xb') as stderr:
                    result = subprocess.run([str(binary), 'unpack'], stdin=stdin, stdout=stdout, stderr=stderr, env={**os.environ,'LC_ALL':'C'})
                require((result.returncode == 0) == expected, f"Unexpected acceptance/refusal: {name}/{variant}")
                if not expected:
                    require(result.returncode == 1 and output.stat().st_size == 0 and error.stat().st_size > 0,
                            "Decoder failed without a clean refusal or emitted partial data")
                outcomes[variant] = {"exit_code":result.returncode, "stdout":binding(output), "stderr":binding(error)}
            if expected:
                require(outcomes['preceding']['stdout']['sha256'] == outcomes['current']['stdout']['sha256'], "Positive refusal-control output changed")
                same_value(fixture, read_json(Path(outcomes['current']['stdout']['path']).read_bytes()))
            report['refusal_cases'].append({'name':name, 'expected_accept':expected, 'input':binding(source), 'outcomes':outcomes})
        require(all(sha(Path(v['path'])) == v['sha256'] for v in report['controllers_and_codec'].values()), 'Controller/codec source drift')
        require(all(sha(p) == report['binaries'][k]['sha256'] for k,p in binaries.items()), 'Binary drift')
        require(fingerprint(ROOT) == report['source'], 'Final production source drift')
        require(binding(args.preceding_receipt) == report['preceding_receipt'] and binding(args.storage_report) == report['storage_report'], 'Historical report drift')
        report['result'] = 'pass'
    except BaseException as error:
        report.update(result='failed', error=f'{type(error).__name__}: {error}')
        print(report['error'], file=sys.stderr, flush=True)
    finally:
        report['finished_at'] = dt.datetime.now(dt.timezone.utc).isoformat()
        with args.output.open('x') as output:
            output.write(json.dumps(report,indent=2)+'\n')
        print('REPORT',args.output,flush=True)
    return 0 if report['result'] == 'pass' else 1


if __name__ == '__main__':
    sys.exit(main())
