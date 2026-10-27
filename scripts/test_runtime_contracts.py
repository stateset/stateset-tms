"""The runtime driver may retry only an explicit pre-commit source refusal."""

import copy
import json
import unittest
from unittest.mock import Mock, patch

from runtime_contracts import Runtime


class RateConfirmationRetryTests(unittest.TestCase):
    def setUp(self):
        self.runtime = Runtime.__new__(Runtime)
        self.runtime.report = {}
        self.runtime.secrets = []
        self.runtime.save = Mock()
        self.workspace = {
            "loads": [{"id": "load-owned", "version": 7}],
            "rate_confirmations": [{"id": "original", "load_id": "load-owned"}],
        }
        self.runtime.workspace = Mock(return_value=self.workspace)
        self.body = {"expected_load_version": 7, "terms": "Original agreement", "reason": "Owned test"}
        self.refusal = (409, {"error": {"message": "Sync source check changed; retry refresh"}})
        self.success = (200, {"data": {"id": "new-agreement"}})
        self.runtime.http = Mock()
        self.sleep = patch("runtime_contracts.time.sleep").start()
        self.addCleanup(patch.stopall)

    def invoke(self):
        return self.runtime.rate_confirmation("load-owned", self.body)

    def test_first_success_needs_no_retry(self):
        self.runtime.http.return_value = self.success
        self.assertEqual(self.invoke(), self.success[1]["data"])
        self.runtime.http.assert_called_once()
        self.sleep.assert_not_called()
        self.assertNotIn("source_observation_retries", self.runtime.report)

    def test_refusal_reuses_identical_bytes_key_and_version(self):
        self.runtime.http.side_effect = [self.refusal, self.success]
        self.assertEqual(self.invoke(), self.success[1]["data"])
        calls = self.runtime.http.call_args_list
        self.assertEqual(calls[0], calls[1])
        self.assertEqual(json.loads(calls[0].kwargs["body"]), self.body)
        evidence = self.runtime.report["source_observation_retries"]
        self.assertEqual(len(evidence), 1)
        self.assertEqual(evidence[0]["original_load_version"], 7)
        self.runtime.save.assert_called_once()

    def test_retry_budget_is_finite_and_each_refusal_retained(self):
        self.runtime.http.return_value = self.refusal
        with self.assertRaisesRegex(AssertionError, "exhausted three"):
            self.invoke()
        self.assertEqual(self.runtime.http.call_count, 3)
        self.assertEqual(len(self.runtime.report["source_observation_retries"]), 3)
        self.assertEqual(self.sleep.call_count, 2)

    def test_other_conflicts_are_not_retried(self):
        for message in ("Load version changed", "Source cancelled", "Sync source check changed; retry refresh later"):
            with self.subTest(message=message):
                self.runtime.http.reset_mock()
                self.runtime.http.return_value = (409, {"error": {"message": message}})
                with self.assertRaisesRegex(AssertionError, "Rate confirmation refused"):
                    self.invoke()
                self.runtime.http.assert_called_once()

    def test_uncertain_transport_or_unexpected_http_failure_is_not_retried(self):
        for error in (TimeoutError("uncertain save"), AssertionError("HTTP 500")):
            with self.subTest(error=error):
                self.runtime.http.reset_mock()
                self.runtime.http.side_effect = error
                with self.assertRaises(type(error)):
                    self.invoke()
                self.runtime.http.assert_called_once()

    def test_changed_load_version_is_not_adopted(self):
        changed = copy.deepcopy(self.workspace)
        changed["loads"][0]["version"] = 8
        self.runtime.workspace.side_effect = [self.workspace, changed]
        self.runtime.http.return_value = self.refusal
        with self.assertRaisesRegex(AssertionError, "must not be rebased"):
            self.invoke()
        self.runtime.http.assert_called_once()
        self.assertEqual(self.body["expected_load_version"], 7)

    def test_new_or_modified_agreement_prevents_retry(self):
        for docs in ([{"id": "unexpected", "load_id": "load-owned"}],
                     [{"id": "original", "load_id": "load-owned", "sha256": "changed"}]):
            with self.subTest(docs=docs):
                changed = copy.deepcopy(self.workspace)
                changed["rate_confirmations"] = docs
                self.runtime.workspace.side_effect = [self.workspace, changed]
                self.runtime.http.reset_mock()
                self.runtime.http.return_value = self.refusal
                with self.assertRaisesRegex(AssertionError, "refusing retry"):
                    self.invoke()
                self.runtime.http.assert_called_once()


if __name__ == "__main__":
    unittest.main()
