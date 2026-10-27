#!/usr/bin/env python3
"""Fixed-rate synthetic invoice/history traffic on source-bound resource limits.

Records server responses separately from unsent client arrivals. This finite
local diagnostic establishes neither production capacity nor a service SLO.
"""

from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.util
from pathlib import Path
import threading
import time
import urllib.parse

spec = importlib.util.spec_from_file_location(
    "sustained_mixed", Path(__file__).with_name("qualify-api-mixed-envelope.py")
)
mixed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mixed)
base, exact, require = mixed.base, mixed.exact, mixed.require


def schedule(duration, interval_ms):
    require(type(duration) is int and 1 <= duration <= 600, "Invalid duration")
    require(
        type(interval_ms) is int and 100 <= interval_ms <= 60000, "Invalid interval"
    )
    return [n / 1000 for n in range(0, duration * 1000, interval_ms)]


def verify_finance(value, expected):
    """Independent integer ledger and allocation controls, beyond object counts."""
    require(
        set(expected["counts"])
        == {"loads", "invoices", "bills", "settlements", "cash_entries", "journal"},
        "Incomplete financial fixture declaration",
    )
    require(
        all(type(n) is int and n > 0 for n in expected["counts"].values()),
        "Empty or ambiguous financial fixture",
    )
    for root, count in expected["counts"].items():
        require(len(value[root]) == count, "Financial cardinality changed: " + root)
    accounts = {}
    seen = set()
    for entry in value["journal"]:
        require(entry["id"] not in seen, "Duplicate journal identity")
        seen.add(entry["id"])
        debit, credit = 0, 0
        for line in entry["lines"]:
            d, c = line["debit_cents"], line["credit_cents"]
            require(
                type(d) is int
                and type(c) is int
                and d >= 0
                and c >= 0
                and (d == 0) != (c == 0),
                "Invalid journal cents",
            )
            debit += d
            credit += c
            accounts[line["account"]] = accounts.get(line["account"], 0) + d - c
        require(debit == credit and debit > 0, "Unbalanced journal")
    require(accounts == expected["account_balances_cents"], "Account cents differ")
    invoice_loads = {v["load_id"] for v in value["invoices"]}
    require(len(invoice_loads) == len(value["invoices"]), "Duplicate primary invoice")
    require(
        all(
            v["status"] == "approved" and v["amount_cents"] > 0
            for v in value["invoices"]
        ),
        "Invalid synthetic liability",
    )
    targets = {v["id"] for root in ["bills", "settlements"] for v in value[root]}
    references = set()
    for cash in value["cash_entries"]:
        require(
            cash["external_reference"] not in references, "Duplicate bank reference"
        )
        references.add(cash["external_reference"])
        require(
            all(v["target_id"] in targets for v in cash["allocations"]),
            "Unknown allocation",
        )
        require(
            sum(v["amount_cents"] for v in cash["allocations"])
            == cash["amount_cents"]
            > 0,
            "Unbalanced cash allocation",
        )


WRITE_VERIFICATION = {"per_write", "post_traffic_chain"}


def classify_chain(before, after, writes, classify):
    """Classify sequential writes from one before and one after original.

    `writes` holds (body, key, data) in send order; data is the 200 response
    load, or None for a 503. Each step rebuilds the intermediate original from
    the final events/receipts and the response load, then applies the same
    pairwise classifier; the last rebuilt original must equal `after` exactly.
    """
    committed = [w for w in writes if w[2] is not None]
    require(
        len(after["events"]) == len(before["events"]) + len(committed),
        "Event count differs from committed writes",
    )
    outcomes, current, appended = [], before, 0
    for body, key, data in writes:
        following = dict(current)
        if data is not None:
            appended += 1
            following["loads"] = [data] + current["loads"][1:]
            following["events"] = after["events"][: len(before["events"]) + appended]
            following["receipts"] = dict(current["receipts"])
            require(key in after["receipts"], "Committed write lacks a receipt")
            following["receipts"][key] = after["receipts"][key]
            if "operations_cases" in after:
                following["operations_cases"] = after["operations_cases"]
        outcome = classify(current, following, body, key)
        require(
            outcome == ("committed_once" if data is not None else "unapplied"),
            "Write outcome differs from its HTTP response",
        )
        outcomes.append(outcome)
        current = following
    require(
        exact.encode(current) == exact.encode(after),
        "Final original differs from the classified write chain",
    )
    return outcomes


class SustainedEnvelope(base.Envelope):
    def __init__(self, args):
        super().__init__(args)
        self.tokens["writer"] = base.secrets.token_urlsafe(36)
        self.secrets.append(self.tokens["writer"])
        self.report["scope"] = __doc__
        for path in [Path(__file__).resolve(), Path(mixed.__file__).resolve()]:
            self.report["harness"][str(path.relative_to(base.ROOT))] = base.sha(path)
        self.traffic = self.plan["sustained"]
        for key in ["read", "probe", "health", "ready", "write"]:
            schedule(
                self.traffic["duration_seconds"], self.traffic[key + "_interval_ms"]
            )
        require(
            type(self.traffic["max_outstanding"]) is int
            and 1 <= self.traffic["max_outstanding"] <= 64,
            "Invalid client bound",
        )
        # per_write verifies each complete original before the next write may
        # start; post_traffic_chain verifies the same chain once traffic ends.
        self.write_verification = self.traffic.get("write_verification", "per_write")
        require(
            self.write_verification in WRITE_VERIFICATION,
            "Invalid write verification",
        )
        self.report.update(
            arrivals=[],
            write_observations=[],
            capacity_slo="not established",
            write_verification=self.write_verification,
        )
        self.write_guard = threading.Lock()
        self.writer_uncertain = False
        self.writer_before = self.writer_current = None
        self.writer_verified = False
        for path in [args.large, args.probe]:
            item = self.plan["fixture_finance"][str(path)]
            require(base.sha(path) == item["sha256"], "Financial fixture changed")
            verify_finance(exact.decode(path.read_bytes()), item)

    def seed_fixtures(self):
        return super().seed_fixtures() + [("writer", self.args.large)]

    original = mixed.MixedEnvelope.original
    classify_original_write = mixed.MixedEnvelope.classify_original_write

    def verify_sources_after(self):
        try:
            self.verify_final_sources()
        except Exception as error:
            self.report.update(
                result="failed",
                source_verification_error=self.safe(
                    type(error).__name__ + ": " + str(error)
                ),
            )

    def verify_final_sources(self):
        for tenant in ["large", "probe"]:
            if tenant not in getattr(self, "expected", {}):
                continue
            value = self.original(tenant, "final-original-" + tenant)
            require(
                hashlib.sha256(self.encode_original(value).encode()).hexdigest()
                == self.expected[tenant]["source_sha256"],
                "Untouched finance tenant changed",
            )
        if self.writer_current is not None:
            value = self.original("writer", "final-original-writer")
            if self.writer_verified:
                require(
                    value == self.writer_current,
                    "Writer changed after last observed transaction",
                )
            else:
                pending = self.report["write_observations"][-1]
                pending["final_classification"] = self.classify_original_write(
                    self.writer_current, value, pending["body"], pending["key"]
                )
            verify_finance(value, self.plan["fixture_finance"][str(self.args.large)])
        self.report["original_financial_sources_verified_after"] = True

    def observe_write(self, name):
        try:
            return self._observe_write(name)
        except BaseException:
            # A failed original/receipt verification must prevent the next write.
            self.writer_uncertain = True
            raise

    def _observe_write(self, name):
        if self.writer_uncertain:
            return {"name": name, "not_sent": "preceding_write_outcome_uncertain"}
        load = self.writer_current["loads"][0]
        body = {
            "action": "location",
            "expected_version": load["version"],
            "location": "Synthetic sustained observation " + name,
        }
        key = "sustained-original-" + name
        path = "/v1/loads/" + urllib.parse.quote(load["id"], safe="") + "/commands"
        observation = {"name": name, "body": body, "key": key, "path": path}
        self.report["write_observations"].append(observation)
        self.report["writes_started"] = True
        self.writer_verified = False
        if self.write_verification == "post_traffic_chain":
            body["expected_version"] = self.writer_version
            return self._send_chained_write(name, observation)
        result = self.request("writer", path, name, body=body, key=key)
        after = self.original("writer", name + "-original")
        outcome = self.classify_original_write(self.writer_current, after, body, key)
        observation["classification"] = outcome
        if result["status"] == 200 and "error" not in result:
            require(outcome == "committed_once", "Successful write lacks commitment")
            response = after["receipts"][key]["response"]
            require(
                exact.decode(base.read_artifact(result["response"]))
                == {"data": response, "replayed": False},
                "Write response differs from original",
            )
        elif "error" in result:
            self.writer_uncertain = True
        else:
            require(
                result["status"] == 503 and outcome == "unapplied",
                "Non-success write changed source",
            )
        self.writer_current = after
        self.writer_verified = True
        return result

    def _send_chained_write(self, name, observation):
        """Send one write and check only its own response; any response that
        does not prove commitment or a clean 503 stops all later writes."""
        body, key = observation["body"], observation["key"]
        result = self.request("writer", observation["path"], name, body=body, key=key)
        if "error" in result:
            self.writer_uncertain = True
            return result
        if result["status"] == 503:
            observation["response_load"] = None
            return result
        require(result["status"] == 200, "Unexpected chained write status")
        value = exact.decode(base.read_artifact(result["response"]))
        load = value["data"]
        require(
            value["replayed"] is False
            and load["version"] == body["expected_version"] + 1
            and load["location"] == body["location"],
            "Chained write response does not match its command",
        )
        observation["response_load"] = load
        self.writer_version = load["version"]
        self.writer_verified = True
        return result

    def verify_chained_writes(self):
        """Classify every sent write against one complete after-traffic original."""
        sent = [o for o in self.report["write_observations"] if "response_load" in o]
        require(
            len(sent) == len(self.report["write_observations"]),
            "A chained write outcome is unknown",
        )
        after = self.original("writer", "writer-after-traffic")
        outcomes = classify_chain(
            self.writer_before,
            after,
            [(o["body"], o["key"], o.pop("response_load")) for o in sent],
            self.classify_original_write,
        )
        for observation, outcome in zip(sent, outcomes):
            observation["classification"] = outcome
        self.writer_current = after

    def checks(self):
        stable = {}
        for tenant in ["large", "probe", "writer"]:
            result = self.request(tenant, "/v1/workspace", "warm-" + tenant)
            self.report["requests"].append(result)
            require(
                result["status"] == 200 and "error" not in result,
                "Warm financial workspace failed",
            )
            stable[tenant] = self.stable(result)
        self.writer_before = self.writer_current = self.original(
            "writer", "writer-before"
        )
        self.writer_verified = True
        self.writer_version = self.writer_current["loads"][0]["version"]
        catalog_path = "/v1/loads?limit=1&q=" + urllib.parse.quote(
            self.expected["probe"]["id"]
        )
        warm = self.request("probe", catalog_path, "warm-catalog")
        self.report["requests"].append(warm)
        require(warm["status"] == 200, "Warm catalog failed")
        catalog = exact.decode(base.read_artifact(warm["response"]))
        before = self.physical()
        time.sleep(35)
        self.healthy()
        require(before == self.physical(), "Idle original financial rows changed")
        self.step("financial workspaces warm;35second idle retains physical originals")
        specs = []
        duration = self.traffic["duration_seconds"]
        for kind in ["read", "probe", "health", "ready", "write"]:
            for n, offset in enumerate(
                schedule(duration, self.traffic[kind + "_interval_ms"])
            ):
                specs.append((offset, kind, f"arrival-{kind}-{n:04}"))
        specs.sort()
        bound = threading.BoundedSemaphore(self.traffic["max_outstanding"])
        started = time.monotonic()
        self.report["traffic_started_monotonic"] = started
        self.report["declared_sustained"] = self.traffic
        futures = []

        def send(kind, name, arrival):
            arrival["actual_started"] = time.monotonic()
            arrival["start_lateness_ms"] = (
                arrival["actual_started"] - arrival["planned_at"]
            ) * 1000
            try:
                if kind == "write":
                    return self.observe_write(name)
                tenant, path = (
                    ("large", "/v1/workspace")
                    if kind == "read"
                    else ("probe", catalog_path if kind == "probe" else "/" + kind)
                )
                return self.request(tenant, path, name)
            finally:
                if kind == "write":
                    self.write_guard.release()
                bound.release()

        with ThreadPoolExecutor(max_workers=self.traffic["max_outstanding"]) as pool:
            for offset, kind, name in specs:
                remaining = started + offset - time.monotonic()
                if remaining > 0:
                    time.sleep(remaining)
                arrival = {
                    "name": name,
                    "kind": kind,
                    "planned_at": started + offset,
                    "scheduler_at": time.monotonic(),
                }
                self.report["arrivals"].append(arrival)
                if kind == "write" and self.writer_uncertain:
                    arrival["not_sent"] = "preceding_write_outcome_uncertain"
                    continue
                if kind == "write" and not self.write_guard.acquire(blocking=False):
                    arrival["not_sent"] = "preceding_write_still_observing"
                    continue
                if not bound.acquire(blocking=False):
                    if kind == "write":
                        self.write_guard.release()
                    arrival["not_sent"] = "client_outstanding_limit"
                    continue
                futures.append((name, pool.submit(send, kind, name, arrival)))
            # Retain all responses before asserting contracts, including worker exceptions.
            for name, future in futures:
                try:
                    result = future.result()
                except BaseException as error:
                    result = {
                        "name": name,
                        "status": 0,
                        "harness_error": self.safe(
                            type(error).__name__ + ": " + str(error)
                        ),
                    }
                self.report["requests"].append(result)
        self.report["traffic_finished_monotonic"] = time.monotonic()
        self.report["arrival_outcomes"] = {
            "planned": len(specs),
            "sent": len(futures),
            "not_sent": sum("not_sent" in a for a in self.report["arrivals"]),
        }
        self.save()
        time.sleep(35)
        self.healthy()
        if self.write_verification == "post_traffic_chain":
            self.verify_chained_writes()
            self.step("chained writes verified against one complete original")
        accepted = committed = 0
        for result in self.report["requests"]:
            if not result["name"].startswith("arrival-"):
                continue
            require(
                "error" not in result
                and "harness_error" not in result
                and "not_sent" not in result,
                "Transport/client validation failed",
            )
            if result["status"] == 503:
                value = exact.decode(base.read_artifact(result["response"]))
                require(
                    result["retry_after"] == "1"
                    and result["cache_control"] == "no-store"
                    and value["error"]["code"]
                    in {
                        "read_capacity_exceeded",
                        "workspace_capacity_exceeded",
                        "database_temporarily_unavailable",
                        "write_capacity_exceeded",
                    },
                    "Unexpected busy response",
                )
                require(
                    result["name"].startswith(
                        ("arrival-read-", "arrival-write-", "arrival-probe-")
                    ),
                    "Health/readiness unavailable",
                )
                continue
            require(result["status"] == 200, "Unexpected server status")
            if result["name"].startswith("arrival-read-"):
                require(
                    self.stable(result) == stable["large"],
                    "Incomplete/changed financial response",
                )
                accepted += 1
            elif result["name"].startswith("arrival-probe-"):
                require(
                    exact.decode(base.read_artifact(result["response"]))
                    == catalog,
                    "Catalog changed",
                )
            elif result["name"].startswith("arrival-write-"):
                committed += 1
        self.report.update(accepted_large_reads=accepted, successful_writes=committed)
        require(accepted > 0 and committed > 0, "No useful read/write progress")
        require(
            not any("not_sent" in a for a in self.report["arrivals"]),
            "Client dropped offered arrivals; workload not fully delivered",
        )
        # Only proven originals are replayed, after scheduled traffic has drained.
        for observation in self.report["write_observations"]:
            if observation.get("classification") != "committed_once":
                continue
            before = self.physical()
            result = self.request(
                "writer",
                observation["path"],
                observation["name"] + "-replay",
                body=observation["body"],
                key=observation["key"],
            )
            self.report["requests"].append(result)
            receipt = self.writer_current["receipts"][observation["key"]]
            require(
                result["status"] == 200
                and exact.decode(base.read_artifact(result["response"]))
                == {"data": receipt["response"], "replayed": True},
                "Original replay failed",
            )
            require(self.physical() == before, "Replay rewrote originals")
        self.step(
            "fixedrate financial workload and exact originals verified; no productioncapacity claim"
        )


if __name__ == "__main__":
    raise SystemExit(base.main(SustainedEnvelope))
