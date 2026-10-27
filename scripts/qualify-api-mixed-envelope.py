#!/usr/bin/env python3
"""Observe one real location write overlapping a finite cross-tenant read burst.

Uses only owned synthetic fixtures. A disconnected write is classified from exact
stored originals and its original receipt, never blindly resubmitted.
"""

from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import secrets
import threading
import time
import urllib.parse

spec = importlib.util.spec_from_file_location(
    "mixed_base", Path(__file__).with_name("qualify-api-envelope.py")
)
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
exact, require = base.exact, base.require


def validate_instant(value):
    # Python 3.8 accepts microseconds; Chrono emits up to nine fractional digits.
    # Validate without changing any retained timestamp or comparison value.
    match = re.fullmatch(
        r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?(Z|[+-]\d{2}:\d{2})",
        value,
    )
    require(match is not None, "Invalid observation timestamp")
    fraction = "." + match[2][:6].ljust(6, "0") if match[2] else ""
    dt.datetime.fromisoformat(match[1] + fraction + match[3].replace("Z", "+00:00"))


def case_digest(value):
    return hashlib.sha256(
        json.dumps(
            value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
        ).encode()
    ).hexdigest()


def verify_tracking_resolution(before, after, load_id):
    """Only the observed load's retained stale-tracking case may resolve."""
    require(len(before) == len(after), "Operations case cardinality changed")
    for old, current in zip(before, after):
        if old == current:
            require(
                not (
                    old["source"]["kind"] == "tracking_stale"
                    and old["source"]["load_id"] == load_id
                    and old["status"] != "resolved"
                ),
                "Observed tracking case did not resolve",
            )
            continue
        require(
            old["source"]["kind"] == "tracking_stale"
            and old["source"]["load_id"] == load_id
            and old["status"] != "resolved",
            "Unrelated operations case changed",
        )
        expected = dict(old)
        expected.update(
            status="resolved",
            resolved_at=current["updated_at"],
            updated_at=current["updated_at"],
            deferred_until=None,
            version=old["version"] + 1,
            history=old["history"]
            + [
                {
                    "action": "source_cleared",
                    "actor": "system:operations",
                    "at": current["updated_at"],
                    "cycle": old["cycle"],
                    "note": "The original source condition no longer applies; this is not evidence of an external delivery or payment",
                    "source": None,
                }
            ],
        )
        for case in [old, current]:
            unsealed = {k: v for k, v in case.items() if k != "evidence_sha256"}
            require(
                case["evidence_sha256"] == case_digest(unsealed)
                and case["source_sha256"] == case_digest(case["source"]),
                "Operations evidence seal mismatch",
            )
        expected["evidence_sha256"] = current["evidence_sha256"]
        require(
            expected == current, "Tracking resolution rewrote retained case evidence"
        )


def classify_write(before, after, body, key):
    """Require an atomic, exact original or the one declared location mutation."""
    if exact.encode(before) == exact.encode(after):
        require(key not in after["receipts"], "Unapplied write has a receipt")
        return "unapplied"
    require(set(before) == set(after), "Workspace roots changed")
    for root in before:
        if root not in {"loads", "events", "receipts", "operations_cases"}:
            require(
                exact.encode(before[root]) == exact.encode(after[root]),
                "Write changed unrelated originals: " + root,
            )
    require(len(before["loads"]) == len(after["loads"]), "Load cardinality changed")
    require(before["loads"][1:] == after["loads"][1:], "Unrelated loads changed")
    original, changed = before["loads"][0], after["loads"][0]
    if "operations_cases" in before:
        verify_tracking_resolution(
            before["operations_cases"], after["operations_cases"], original["id"]
        )
    require(original["version"] == body["expected_version"], "Wrong original version")
    require(set(original) == set(changed), "Load fields changed")
    for field in original:
        if field not in {"version", "location", "updated_at", "last_tracking_at"}:
            require(
                original[field] == changed[field],
                "Unrelated load field changed: " + field,
            )
    require(
        changed["version"] == original["version"] + 1
        and changed["location"] == body["location"],
        "Location/version mismatch",
    )
    for field in ["updated_at", "last_tracking_at"]:
        require(changed[field] != original[field], "Observation time not updated")
        validate_instant(changed[field])
    require(
        len(after["events"]) == len(before["events"]) + 1
        and after["events"][:-1] == before["events"],
        "Original event history changed",
    )
    event = after["events"][-1]
    require(
        event["entity_id"] == original["id"]
        and event["kind"] == "load.location"
        and event["actor"] == "envelope:operator"
        and event["message"] == "Location updated: " + body["location"],
        "New event does not match declared command",
    )
    require(
        set(after["receipts"]) == set(before["receipts"]) | {key}
        and key not in before["receipts"],
        "Unexpected receipt set",
    )
    for old_key in before["receipts"]:
        require(
            before["receipts"][old_key] == after["receipts"][old_key],
            "Old receipt changed",
        )
    receipt = after["receipts"][key]
    signature = hashlib.sha256(
        json.dumps(
            ["load.command", original["id"], body],
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    require(
        receipt["fingerprint"] == signature
        and receipt["response"] == changed
        and not receipt.get("stopped"),
        "Original receipt mismatch",
    )
    return "committed_once"


class MixedEnvelope(base.Envelope):
    def __init__(self, args):
        super().__init__(args)
        require(
            self.plan["workload"].get("operations_reconciliation")
            == "Only the writer load's original stale-tracking case may resolve; exact original history, source hashes and evidence seals verified",
            "Declare the actual operations reconciliation before a new run",
        )
        require(
            self.plan["workload"].get("mixed_write")
            == {
                "tenant": "writer",
                "fixture": str(args.large),
                "action": "location",
                "count": 1,
                "retry": "only after stored receipt proves commitment",
            },
            "Mixed workload plan mismatch",
        )
        self.tokens["writer"] = secrets.token_urlsafe(36)
        self.secrets.append(self.tokens["writer"])
        self.report["scope"] = __doc__
        self.report["harness"][str(Path(__file__).resolve().relative_to(base.ROOT))] = (
            base.sha(__file__)
        )
        self.key = "mixed-envelope-original-location-v1"
        self.body = None
        self.writer_before = None
        self.report["write_classification"] = "not_started"

    def seed_fixtures(self):
        return super().seed_fixtures() + [("writer", self.args.large)]

    def original(self, tenant, name):
        value, encoded = self.read_original_document(
            self.sql(
                "freight_runtime",
                "SELECT freight_workspace_originals("
                + base.literal(tenant)
                + ")::text",
            ).encode()
        )
        path = self.directory / (name + ".json")
        artifact = base.write_artifact(path, encoded.encode(), self.evidence_compression)
        self.report.setdefault("original_snapshots", []).append(
            dict(artifact, tenant=tenant)
        )
        return value

    def classify_original_write(self, before, after, body, key):
        worker = getattr(self, "original_json", None)
        if worker is not None:
            return worker.classify_write(before, after, body, key)
        return classify_write(before, after, body, key)

    def verify_sources_after(self):
        # Record a verification failure while still preserving a checked database
        # archive and cleaning owned resources. Never mutate the baseline hashes.
        try:
            for tenant in ["large", "probe"]:
                if tenant not in getattr(self, "expected", {}):
                    continue
                value = self.original(tenant, "final-original-" + tenant)
                require(
                    hashlib.sha256(self.encode_original(value).encode()).hexdigest()
                    == self.expected[tenant]["source_sha256"],
                    "Untouched tenant changed",
                )
            if "writer" in getattr(self, "expected", {}):
                after = self.original("writer", "final-original-writer")
                if self.writer_before is not None and self.body is not None:
                    outcome = self.classify_original_write(
                        self.writer_before, after, self.body, self.key
                    )
                else:
                    require(
                        hashlib.sha256(self.encode_original(after).encode()).hexdigest()
                        == self.expected["writer"]["source_sha256"],
                        "Unstarted writer changed",
                    )
                    outcome = "not_started"
                self.report["write_classification"] = outcome
                response = next(
                    (r for r in self.report["requests"] if r["name"] == "burst-write"),
                    None,
                )
                if response and response["status"] == 200:
                    require(
                        outcome == "committed_once",
                        "Successful response lacks stored commitment",
                    )
                    require(
                        json.loads(base.read_artifact(response["response"]))
                        == {
                            "data": after["receipts"][self.key]["response"],
                            "replayed": False,
                        },
                        "Response differs from original stored receipt",
                    )
            self.report["untouched_tenant_originals_unchanged_after"] = True
        except Exception as error:
            self.report.update(
                result="failed",
                source_verification_error=type(error).__name__ + ": " + str(error),
            )

    def checks(self):
        stable = {}
        for tenant in ["large", "probe", "writer"]:
            result = self.request(tenant, "/v1/workspace", "warm-" + tenant)
            self.report["requests"].append(result)
            require(
                result["status"] == 200 and "error" not in result,
                "Warm workspace failed",
            )
            stable[tenant] = self.stable(result)
        path = "/v1/loads?limit=1&q=" + urllib.parse.quote(self.expected["probe"]["id"])
        warm = self.request("probe", path, "warm-catalog")
        self.report["requests"].append(warm)
        require(warm["status"] == 200, "Warm catalog failed")
        catalog = json.loads(base.read_artifact(warm["response"]))
        self.writer_before = self.original("writer", "writer-before")
        load = self.writer_before["loads"][0]
        require(
            load["status"] == "in_transit" and load["sync_order_id"] is None,
            "Fixture cannot isolate a location observation",
        )
        self.body = {
            "action": "location",
            "expected_version": load["version"],
            "location": "Synthetic mixed-envelope observed location",
        }
        self.report["declared_write"] = {
            "tenant": "writer",
            "load_id": load["id"],
            "body": self.body,
            "idempotency_key": self.key,
        }
        before = self.physical()
        self.report["physical_before_sha256"] = hashlib.sha256(
            before.encode()
        ).hexdigest()
        time.sleep(35)
        self.healthy()
        require(self.physical() == before, "Idle workers changed originals")
        self.step("three warm exact tenants and35second idle retain originals")
        specs = [("large", "/v1/workspace", f"burst-large-{n}") for n in range(8)] + [
            ("probe", path, "burst-probe"),
            ("probe", "/health", "burst-health"),
            ("probe", "/ready", "burst-ready"),
        ]
        barrier = threading.Barrier(len(specs) + 2)
        write_path = (
            "/v1/loads/" + urllib.parse.quote(load["id"], safe="") + "/commands"
        )
        self.report["writes_started"] = True
        with ThreadPoolExecutor(max_workers=len(specs) + 1) as pool:
            futures = [pool.submit(self.request, *item, barrier) for item in specs]
            futures.append(
                pool.submit(
                    self.request,
                    "writer",
                    write_path,
                    "burst-write",
                    barrier,
                    body=self.body,
                    key=self.key,
                )
            )
            barrier.wait(timeout=15)
            results = [f.result() for f in futures]
        self.report["requests"].extend(results)
        self.report["burst_completed"] = True
        time.sleep(35)
        # Diagnose commitment even if API died. No retry on an absent receipt.
        after = self.original("writer", "writer-after-burst")
        outcome = self.classify_original_write(self.writer_before, after, self.body, self.key)
        self.report["write_classification"] = outcome
        self.healthy()
        accepted = 0
        for result in results:
            require("error" not in result, "Request transport failed")
            if result["name"].startswith("burst-large-"):
                if result["status"] == 200:
                    require(
                        self.stable(result) == stable["large"],
                        "Read tenant output changed",
                    )
                    accepted += 1
                else:
                    error = json.loads(base.read_artifact(result["response"]))
                    require(
                        result["status"] == 503
                        and result["retry_after"] == "1"
                        and result["cache_control"] == "no-store"
                        and error["error"]["code"]
                        in {
                            "read_capacity_exceeded",
                            "workspace_capacity_exceeded",
                            "database_temporarily_unavailable",
                        },
                        "Unexpected read rejection",
                    )
            else:
                require(result["status"] == 200, "Write or probe failed")
                if result["name"] == "burst-probe":
                    require(
                        json.loads(base.read_artifact(result["response"])) == catalog,
                        "Probe data changed",
                    )
        require(
            accepted > 0 and outcome == "committed_once",
            "No useful read/write progress",
        )
        physical = self.physical()
        replay = self.request(
            "writer", write_path, "receipt-proven-replay", body=self.body, key=self.key
        )
        self.report["requests"].append(replay)
        require(
            replay["status"] == 200 and "error" not in replay, "Receipt replay failed"
        )
        require(
            json.loads(base.read_artifact(replay["response"]))
            == {"data": after["receipts"][self.key]["response"], "replayed": True},
            "Replay differs from original stored response",
        )
        require(
            self.physical() == physical, "Receipt replay rewrote physical originals"
        )
        self.report.update(
            accepted_large_reads=accepted, receipt_replay_preserves_physical_source=True
        )
        self.step("mixed finite burst and receipt-proven replay verified without OOM")


if __name__ == "__main__":
    raise SystemExit(base.main(MixedEnvelope))
