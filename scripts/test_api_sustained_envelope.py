"""Negative controls for offered-load accounting and financial fixture corruption."""

import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "sustained_test", Path(__file__).with_name("qualify-api-sustained-envelope.py")
)
sustained = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sustained)


class SustainedControls(unittest.TestCase):
    def test_release_times_are_fixed_and_exclude_duration_boundary(self):
        self.assertEqual(sustained.schedule(3, 1000), [0, 1, 2])
        self.assertEqual(sustained.schedule(2, 750), [0, 0.75, 1.5])
        self.assertEqual(len(sustained.schedule(60, 1000)), 60)
        self.assertEqual(len(sustained.schedule(60, 10000)), 6)

    def test_ambiguous_or_unbounded_arrival_plans_refuse(self):
        for duration, interval in [
            (True, 1000),
            (0, 1000),
            (601, 1000),
            (1.0, 1000),
            (60, 0),
            (60, True),
            (60, 1000.0),
            (60, 60001),
        ]:
            with self.subTest(duration=duration, interval=interval), self.assertRaises(
                AssertionError
            ):
                sustained.schedule(duration, interval)

    def fixture(self):
        value = {
            "loads": [{"id": "l1"}],
            "invoices": [
                {"id": "i1", "load_id": "l1", "status": "approved", "amount_cents": 101}
            ],
            "bills": [{"id": "b1"}],
            "settlements": [{"id": "s1"}],
            "cash_entries": [
                {
                    "external_reference": "bank1",
                    "amount_cents": 101,
                    "allocations": [{"target_id": "b1", "amount_cents": 101}],
                }
            ],
            "journal": [
                {
                    "id": "j1",
                    "lines": [
                        {"account": "cash", "debit_cents": 101, "credit_cents": 0},
                        {
                            "account": "accounts_receivable",
                            "debit_cents": 0,
                            "credit_cents": 101,
                        },
                    ],
                }
            ],
        }
        expected = {
            "counts": {root: len(items) for root, items in value.items()},
            "account_balances_cents": {"cash": 101, "accounts_receivable": -101},
        }
        return value, expected

    def test_valid_integer_finance_passes(self):
        sustained.verify_finance(*self.fixture())

    def test_balanced_but_wrong_accounts_unbalanced_entries_and_fractional_cents_refuse(
        self,
    ):
        original, expected = self.fixture()
        for change in [
            lambda v: v["journal"][0]["lines"][0].update(account="freight_expense"),
            lambda v: v["journal"][0]["lines"][0].update(debit_cents=102),
            lambda v: v["journal"][0]["lines"][0].update(debit_cents=101.0),
            lambda v: v["journal"][0]["lines"][0].update(credit_cents=1),
        ]:
            value = copy.deepcopy(original)
            change(value)
            with self.assertRaises(AssertionError):
                sustained.verify_finance(value, expected)

    def test_cash_allocation_and_original_cardinality_damage_refuse(self):
        original, expected = self.fixture()
        for change in [
            lambda v: v["cash_entries"][0]["allocations"][0].update(
                target_id="missing"
            ),
            lambda v: v["cash_entries"][0]["allocations"][0].update(amount_cents=100),
            lambda v: v["invoices"].clear(),
        ]:
            value = copy.deepcopy(original)
            change(value)
            with self.assertRaises(AssertionError):
                sustained.verify_finance(value, expected)

    def test_empty_fixture_cannot_be_finance_qualification(self):
        value, expected = self.fixture()
        expected["counts"]["invoices"] = 0
        with self.assertRaises(AssertionError):
            sustained.verify_finance(value, expected)


if __name__ == "__main__":
    unittest.main()
