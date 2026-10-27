# Operations projection performance

The subsequent [original-history workspace improvement](deferred-history.md) adds compact staff refreshes and on-demand original histories, preserving this earlier comparison and its source-bound measurements.

The newer [full-workspace history comparison](workspace-history-performance.md) measures the actual staff HTTP response with original gate/appointment history. It retains this earlier report and records both faster responses and memory/write tradeoffs.

The newer [retained stop-history comparison](stop-history-performance.md) extends this work to gate corrections, earlier tenders and elapsed appointments. It preserves this report's original measurements and source bindings. The new in-process gains and mixed HTTP observations have separate evidence.

The control tower uses a request-local typed projection and indexed case/load lookups. It verifies every retained case, including off-page history, once per read; the summary does not build discarded issue JSON, and paginated reads render only the selected rows. Unchanged reconciliation checks a borrowed case before cloning it. No integrity result is cached across workspace reads, and no history is pruned.

This addresses measured CPU and allocation costs in the current aggregate model. It does not normalize PostgreSQL storage or establish production capacity. Storage format remains 17; persisted records, digests, queue order/cursors, source checks, and public response shapes are unchanged.

## Reproduce

```bash
python3 scripts/profile-operations.py --output /tmp/operations-profile.json
# Compare a later implementation with an original report:
python3 scripts/profile-operations.py \
  --baseline /tmp/operations-profile.json \
  --output /tmp/operations-profile-after.json
```

On Linux, requires Rust 1.90+, Python 3, and the cached locked Cargo dependencies. The runner builds the `operations_profile` example using `--release --offline --locked`, with two build jobs and no compiler wrapper/incremental compilation. Each phase has an untimed warmup followed by seven timed samples. Reports retain every sample, min/median/max, compiler/host/binary/source fingerprints, fixture size/hash, and result hashes. JSON phases include projection, encoding, and a response checksum; result comparisons and teardown occur outside the timed interval. Existing output reports are preserved. The fixed clock and fixture generator make before/after records deterministic; comparisons reject changed compiler/host identity, fixture definitions, samples, inputs, or projection results. The runner also rejects application, fixture, or runner edits during sampling.

The workloads contain 100, 1,000, and 5,000 in-transit shipments with tracking history, 10% reported exceptions, 80% refreshed tracking conditions, named/unassigned cases, bounded deferrals, retained source-cleared cases, and 20 notes per case. A separate 1,000-shipment workload retains 100 notes per case. Detection, claim, and closure records add to those note counts. These are generated synthetic typed records with verified full-case digests, not captured customer data or a claim of carrier certification. Fixed timestamps measure actual program work without waiting for wall-clock deadlines.

## Recorded comparison

| Shipments | Notes per case | First page before (ms) | After (ms) | Ratio |
| --------- | -------------- | ---------------------- | ---------- | ----- |
| 100       | 20             | 23.5                   | 6.2        | 3.81× |
| 1,000     | 20             | 260.6                  | 64.0       | 4.07× |
| 5,000     | 20             | 2,126.3                | 335.2      | 6.34× |
| 1,000     | 100            | 674.3                  | 226.7      | 2.97× |

All recorded phases for the 5,000-shipment workload:

| Phase                    | Before median (ms) | After median (ms) |
| ------------------------ | ------------------ | ----------------- |
| Source signals           | 8.7                | 8.5               |
| Reconciliation readiness | 472.6              | 320.0             |
| No-op reconciliation     | 428.9              | 317.0             |
| Full board JSON          | 1,274.6            | 432.8             |
| First page JSON          | 2,126.3            | 335.2             |
| Summary JSON             | 1,005.4            | 324.1             |

First-page and summary medians decreased in all four recorded workloads. One source-signal median increased slightly; the raw samples retain that counter-result. These observations do not establish a uniform throughput gain.

These are in-process timings on the recorded shared host. They exclude PostgreSQL reads/JSONB decoding, transaction locks/writes, HTTP, authentication, browser rendering, worker contention, and external services. Small-sample medians are observations, not latency objectives or statistically established tail bounds. The fixtures do not model large gate/native-carrier/document/financial collections; those paths need separate measurement. Cold-start behavior and multi-tenant concurrency are also unqualified.

## Correctness and next gates

The recorded version predates the [bounded snapshot read path](read-path.md) and subsequent [tenant-aware admission](tenant-read-admission.md). Its measurements and source fingerprints remain historical; follow those reports for their measured versions and current scheduling qualification.

For this checkout, first reverse the [integration queue deadline change](integration-wakeups.md) in an isolated source copy before applying this report's historical reconstruction patch. The original patches and report hashes remain unchanged; the two-stage reconstruction must match the report's original source fingerprint.

For the subsequent snapshot-admission release, first reverse [read overload protection](read-overload.md) using its source patch in an isolated copy, then follow this report's existing reconstruction steps. Its original report, source and patch attestations remain historical.

Current source reconstruction now begins with the [bounded-dispatch reversal](bounded-dispatch-turns.md), then the [process-drain reversal](process-drain.md), then the [tenant-worker reversal](tenant-integration-workers.md), then the [integration-response reversal](integration-response-budgets.md), then the [runtime database-budget reversal](database-budgets.md), then follows the snapshot-overload and earlier reversal steps already described here. Historical performance reports and original patches retain their existing hashes.
