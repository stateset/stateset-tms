"""Negative controls for uncertain-write classification from stored originals."""

import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "mixed_envelope", Path(__file__).with_name("qualify-api-mixed-envelope.py")
)
mixed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mixed)


class WriteClassificationTests(unittest.TestCase):
    def setUp(self):
        self.key = "original-observation"
        self.body = {
            "action": "location",
            "expected_version": 4,
            "location": "Observed depot",
        }
        self.before = {
            "loads": [
                {
                    "id": "load1",
                    "version": 4,
                    "location": "Origin",
                    "sell_cents": 317,
                    "updated_at": "2026-10-18T09:00:00Z",
                    "last_tracking_at": "2026-10-18T09:00:00Z",
                },
                {"id": "untouched", "version": 7},
            ],
            "events": [{"id": "retained-original"}],
            "receipts": {},
            "invoices": [{"id": "retained-invoice", "amount_cents": 317}],
        }
        self.after = copy.deepcopy(self.before)
        self.after["loads"][0].update(
            version=5,
            location=self.body["location"],
            updated_at="2026-10-18T10:00:00Z",
            last_tracking_at="2026-10-18T10:00:00Z",
        )
        self.after["events"].append(
            {
                "entity_id": "load1",
                "kind": "load.location",
                "actor": "envelope:operator",
                "message": "Location updated: Observed depot",
            }
        )
        signature = hashlib.sha256(
            json.dumps(
                ["load.command", "load1", self.body],
                sort_keys=True,
                separators=(",", ":"),
            ).encode()
        ).hexdigest()
        self.after["receipts"][self.key] = {
            "fingerprint": signature,
            "response": copy.deepcopy(self.after["loads"][0]),
        }

    def classify(self, value):
        return mixed.classify_write(self.before, value, self.body, self.key)

    def test_exact_unapplied_and_committed_originals_are_distinguished(self):
        self.assertEqual(self.classify(copy.deepcopy(self.before)), "unapplied")
        self.assertEqual(self.classify(self.after), "committed_once")

    def test_partial_commit_without_receipt_is_refused(self):
        self.after["receipts"].clear()
        with self.assertRaises(AssertionError):
            self.classify(self.after)

    def test_receipt_without_business_commit_is_refused(self):
        value = copy.deepcopy(self.before)
        value["receipts"] = self.after["receipts"]
        with self.assertRaises(AssertionError):
            self.classify(value)

    def test_wrong_fingerprint_or_original_response_is_refused(self):
        for field, value in [("fingerprint", "0" * 64), ("response", {"version": 5})]:
            with self.subTest(field=field):
                after = copy.deepcopy(self.after)
                after["receipts"][self.key][field] = value
                with self.assertRaises(AssertionError):
                    self.classify(after)

    def test_unrelated_money_load_history_and_receipt_changes_are_refused(self):
        for target in [
            "money",
            "other_load",
            "history",
            "extra_receipt",
            "duplicate_version",
        ]:
            with self.subTest(target=target):
                after = copy.deepcopy(self.after)
                if target == "money":
                    after["invoices"][0]["amount_cents"] += 1
                elif target == "other_load":
                    after["loads"][1]["version"] += 1
                elif target == "history":
                    after["events"][0]["id"] = "rewritten"
                elif target == "extra_receipt":
                    after["receipts"]["extra"] = {}
                else:
                    after["loads"][0]["version"] += 1
                with self.assertRaises(AssertionError):
                    self.classify(after)

    def test_nine_digit_actual_timestamps_validate_without_truncating_source(self):
        value = "2026-10-19T03:45:30.439093068+00:00"
        mixed.validate_instant(value)
        self.assertEqual(value, "2026-10-19T03:45:30.439093068+00:00")
        for invalid in [
            "2026-10-19T03:45:30.1234567890Z",
            "2026-10-19",
            "2026-13-19T03:45:30Z",
        ]:
            with self.subTest(invalid=invalid), self.assertRaises(
                (AssertionError, ValueError)
            ):
                mixed.validate_instant(invalid)

    def tracking_case(self):
        source = {"kind": "tracking_stale", "load_id": "load1"}
        value = {
            "id": "original-tracking-case",
            "source": source,
            "source_sha256": mixed.case_digest(source),
            "status": "open",
            "cycle": 1,
            "history": [{"action": "original-note"}],
            "version": 3,
            "updated_at": "2026-10-18T09:00:00Z",
            "resolved_at": None,
            "deferred_until": None,
        }
        value["evidence_sha256"] = mixed.case_digest(value)
        return value

    def test_exact_tracking_resolution_retains_original_history_and_seals(self):
        old = self.tracking_case()
        current = copy.deepcopy(old)
        current.update(
            status="resolved",
            resolved_at="2026-10-18T10:00:00Z",
            updated_at="2026-10-18T10:00:00Z",
            version=4,
        )
        current["history"].append(
            {
                "action": "source_cleared",
                "actor": "system:operations",
                "at": current["updated_at"],
                "cycle": 1,
                "source": None,
                "note": "The original source condition no longer applies; this is not evidence of an external delivery or payment",
            }
        )
        current["evidence_sha256"] = mixed.case_digest(
            {k: v for k, v in current.items() if k != "evidence_sha256"}
        )
        mixed.verify_tracking_resolution([old], [current], "load1")
        for target in ["history", "seal", "unrelated", "unresolved"]:
            with self.subTest(target=target):
                changed = copy.deepcopy(current)
                if target == "history":
                    changed["history"][0]["action"] = "rewritten"
                elif target == "seal":
                    changed["evidence_sha256"] = "0" * 64
                elif target == "unrelated":
                    changed["source"]["load_id"] = "other"
                else:
                    changed = old
                with self.assertRaises(AssertionError):
                    mixed.verify_tracking_resolution([old], [changed], "load1")


if __name__ == "__main__":
    unittest.main()
