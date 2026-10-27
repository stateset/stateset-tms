#!/usr/bin/env python3
"""Independently check the scrape and trace emitted by observability_fixture.

Requires the Python prometheus-client package; this is a qualification-only tool.
"""
import argparse
import hashlib
import importlib.metadata
import json
import math
from pathlib import Path
import re

from prometheus_client.parser import text_string_to_metric_families


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scrape", required=True, type=Path)
    parser.add_argument("--trace", required=True, type=Path)
    args = parser.parse_args()
    raw, trace = args.scrape.read_text(), args.trace.read_text()
    for private in ["must-not-appear", "never-log-this-id", "fixture-monitor-synthetic-credential", "fixture-admin-synthetic-credential"]:
        assert private not in raw and private not in trace, "Private fixture value leaked"
    assert re.search(r'route="?/v1/documents/\{id\}/content"?', trace), "Missing safe route trace"
    families = list(text_string_to_metric_families(raw))
    samples = [sample for family in families for sample in family.samples]
    assert len(families) == 20, "Unexpected metric families"
    assert len(samples) == 97, "Missing or unexpected samples"
    diagnostic_expected = {
        ("freight_admission_pending", (("lane", "snapshot"),)): 0,
        ("freight_admission_pending", (("lane", "catalog"),)): 0,
        ("freight_admission_pending_limit", (("lane", "snapshot"),)): 64,
        ("freight_admission_pending_limit", (("lane", "catalog"),)): 64,
        ("freight_admission_tenant_pending_limit", (("lane", "snapshot"),)): 16,
        ("freight_admission_tenant_pending_limit", (("lane", "catalog"),)): 16,
        ("freight_admission_available_permits", (("lane", "snapshot"),)): 2,
        ("freight_admission_available_permits", (("lane", "catalog"),)): 1,
        ("freight_admission_closed", (("lane", "snapshot"),)): 0,
        ("freight_admission_closed", (("lane", "catalog"),)): 0,
        ("freight_admission_snapshot_available", (("lane", "snapshot"),)): 1,
        ("freight_admission_snapshot_available", (("lane", "catalog"),)): 1,
        ("freight_admission_snapshot_available", (("lane", "write"),)): 1,
        ("freight_workspace_admission_configured", ()): 0,
        ("freight_write_admission_requests", ()): 0,
        ("freight_write_admission_limit", ()): 64,
        ("freight_write_admission_tenant_limit", ()): 16,
        ("freight_database_pool_configured", ()): 0,
    }
    diagnostic_samples = {
        (sample.name, tuple(sorted(sample.labels.items()))): sample.value
        for sample in samples
        if sample.name.startswith(("freight_admission_", "freight_workspace_admission_", "freight_write_admission_", "freight_database_pool_"))
    }
    assert diagnostic_samples == diagnostic_expected, "Configured/disabled lane and pool semantics changed"
    assert len({family.name for family in families}) == len(families), "Metric families must remain grouped and unique"
    for family in families:
        if family.name.startswith(("freight_admission_", "freight_workspace_admission_", "freight_write_admission_", "freight_database_pool_")):
            assert family.type == "gauge"
    identities = [(sample.name, tuple(sorted(sample.labels.items()))) for sample in samples]
    assert len(identities) == len(set(identities)), "Duplicate metric sample"
    assert all(math.isfinite(sample.value) and sample.value >= 0 for sample in samples)
    counts = [s for s in samples if s.name == "freight_http_requests_total"]
    assert len(counts) == 4 and sum(s.value for s in counts) == 4
    assert all(s.labels["method"] == "GET" and set(s.labels) == {"route", "method", "outcome"} for s in counts)
    assert {(s.labels["route"], s.labels["outcome"], s.value) for s in counts} == {
        ("/v1/access", "2xx", 1), ("/v1/access", "4xx", 1), ("/v1/access", "5xx", 1),
        ("/v1/documents/{id}/content", "4xx", 1),
    }
    bounds = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, float("inf")]
    for count in counts:
        buckets = [s for s in samples if s.name == "freight_http_response_headers_duration_seconds_bucket" and all(s.labels[k] == v for k, v in count.labels.items())]
        buckets.sort(key=lambda s: float(s.labels["le"]))
        assert [float(s.labels["le"]) for s in buckets] == bounds
        assert all(a.value <= b.value for a, b in zip(buckets, buckets[1:]))
        assert buckets[-1].value == count.value
        histogram_count = [s for s in samples if s.name == "freight_http_response_headers_duration_seconds_count" and s.labels == count.labels]
        assert len(histogram_count) == 1 and histogram_count[0].value == count.value
        histogram_sum = [s for s in samples if s.name == "freight_http_response_headers_duration_seconds_sum" and s.labels == count.labels]
        assert len(histogram_sum) == 1
    assert next(s.value for s in samples if s.name == "freight_http_in_flight") == 0
    assert next(s.value for s in samples if s.name == "freight_process_draining") == 1
    print(json.dumps({
        "result": "pass", "metric_families": len(families), "samples": len(samples), "requests": 4,
        "parser": "prometheus-client", "parser_version": importlib.metadata.version("prometheus-client"),
        "scrape_sha256": hashlib.sha256(args.scrape.read_bytes()).hexdigest(),
        "trace_sha256": hashlib.sha256(args.trace.read_bytes()).hexdigest(),
        "checker_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
    }, indent=2))


if __name__ == "__main__":
    main()
