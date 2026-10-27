# Control tower reads with retained stop history

The subsequent [full-workspace history optimization](workspace-history-performance.md) extends borrowed gate selection to the staff workspace and adds first-active appointment lookup. This document and its original measurements remain historical; the subsequent comparison retains complete HTTP responses and its own resource tradeoffs.

The control tower selects current gate observations and elapsed appointments once per source calculation, preserving original correction, shipment/tender and clock boundaries. The history-heavy in-process comparison is substantially faster with identical outputs; HTTP latency and memory observations remain mixed.

The operating source calculation previously selected current gate events repeatedly from the tenant's retained history and scanned every appointment for each shipment. The [control tower](../src/operations.rs) now prepares borrowed gate and elapsed-appointment lookups once within each immutable source calculation. The [gate index](../src/stops.rs) retains the original global supersession rule, exact shipment/tender boundaries and original record order. Execution consistency and timelines use those same selected gates. No lookup survives a workspace snapshot or clock evaluation.

## Inputs and measurements

The [fixture](../examples/stop_history_profile.rs) extends the preceding operations profile with original gate observations, explicit correction chains, earlier tender identities, cancelled windows and active appointments. It uses a fixed clock and deterministic IDs. Half the shipments have a current delivery arrival; earlier tenders' arrivals remain separate. Named and unowned cases, deferred work, source-cleared history and twenty notes per case remain in the fixture.

| Shipments | Earlier tender identities per shipment | Notes per case |
| --------- | -------------------------------------- | -------------- |
| 100       | 4                                      | 20             |
| 1,000     | 4                                      | 20             |
| 5,000     | 4                                      | 20             |
| 1,000     | 32                                     | 20             |

The [runner](../scripts/profile-stop-history.py) performs an untimed warmup and seven timed samples for each of six phases: source signals, reconciliation readiness, complete board encoding, first-page encoding, summary encoding and unchanged reconciliation. Fixture generation, result comparison and retained-workspace checks are outside the timed interval. Timing includes construction and disposal of temporary lookup data. No lookup is precomputed outside the measured source calculation.

The optimized run must use the same fixture and runner bytes, compiler, host identity, clock, workload dimensions and sample count. Every phase result hash and complete fixture hash must match the original. Reads and unchanged reconciliation must leave retained records unchanged. A source edit during either measurement invalidates that run. All samples, including unfavorable results, remain in the reports.

```bash
python3 scripts/profile-stop-history.py \
  --output /tmp/freight-stop-index-before.json
# After applying and checking the implementation:
python3 scripts/profile-stop-history.py \
  --baseline /tmp/freight-stop-index-before.json \
  --output /tmp/freight-stop-index-after.json
```

These are typed synthetic records, not captured customer operations or a complete qualification of reachable shipment histories. In-process measurements exclude PostgreSQL transfer/decoding, HTTP, browser rendering, native partner traffic and rich finance histories. They run on the recorded shared host without an imposed API memory quota. Their medians and ranges describe finite observations, not deployment capacity or a service objective.

## HTTP and correctness gates

The existing [HTTP/PostgreSQL workload](service-capacity.md) is also run before and after the change, using its process-sampling driver, 1,000 shipments, sixteen requested reads per wave and identical generated fixture bytes. Its checks include expected issue counts, tenant boundaries, retained original records, version races, unchanged receipt replay, idle revisions and PostgreSQL memory diagnostics. API memory and small-tenant probes remain part of the result even if their timings regress.

```bash
python3 scripts/profile-service-reads.py --sizes 1000 \
  --output /tmp/freight-stop-index-http-before.json
python3 scripts/profile-service-reads.py --sizes 1000 \
  --baseline /tmp/freight-stop-index-http-before.json \
  --output /tmp/freight-stop-index-http-after.json
```

The three [new source regressions](../tests/operations_stop_sources.rs) pass against both implementations. They preserve exact shipment/tender boundaries, the selected correction's chronology, and new projections after clock, appointment, arrival or tender changes. Full Rust/PostgreSQL checks, production-browser regressions, actual EDI codecs and coordinated Freight/EDI/Sync recovery remain required before qualification.

## Recorded measurements

| Shipments / earlier tenders | Source signals    | Full board JSON    | First page JSON    | Unchanged reconciliation |
| --------------------------- | ----------------- | ------------------ | ------------------ | ------------------------ |
| 100 / 4                     | 4.29 → 1.28       | 16.12 → 12.20      | 15.53 → 10.44      | 13.44 → 9.25             |
| 1,000 / 4                   | 810.18 → 11.40    | 874.60 → 134.43    | 814.85 → 105.35    | 804.84 → 101.27          |
| 5,000 / 4                   | 42,975.06 → 66.12 | 45,725.34 → 838.93 | 42,637.76 → 570.35 | 42,602.72 → 525.43       |
| 1,000 / 32                  | 7,483.67 → 36.91  | 7,579.15 → 163.61  | 8,451.60 → 134.88  | 9,063.37 → 129.12        |

The largest fixture retains 62,500 gate events, 30,000 appointments and about 111 MB of workspace JSON. The many-tender fixture retains 68,500 gate events and 34,000 appointments. Entire retained-workspace hashes remain unchanged. The reports also retain summary and reconciliation-readiness timings, every sample and result hash. This is a substantial reduction in measured history-selection CPU within the control tower.

The separate initial HTTP pair is mixed. Eight-client operations p95 increased from 1,263.8 to 1,432.9 ms, the small-tenant probe changed from 745.9 to 727.4 ms, full-workspace p95 increased from 1,339.5 to 1,509.2 ms, and sampled maximum API RSS fell from 550.5 to 504.1 MiB. These unfavorable read observations remain in the comparison.

| HTTP observation              | Original repeats   | Indexed repeats    |
| ----------------------------- | ------------------ | ------------------ |
| Eight-client operations p95   | 1,262.8–1,313.0 ms | 1,203.8–1,254.7 ms |
| Mixed-wave full-workspace p95 | 1,381.3–1,408.5 ms | 1,212.3–1,222.8 ms |
| Small-tenant probe p95        | 738.9–772.1 ms     | 674.1–680.6 ms     |
| Reconciliation request p95    | 3,169.7–3,368.1 ms | 3,922.6–3,960.3 ms |
| Maximum sampled API RSS       | 519.7–593.3 MiB    | 567.5–600.0 MiB    |

These are ranges of individual run observations, not pooled percentiles or confidence intervals. Read timings generally improved in the repeats, reconciliation regressed and memory ranges overlap. The initial regression and repeated variability prevent a uniform HTTP latency or memory-improvement claim. This HTTP fixture has 1,000 shipments and 2,000 gate events; it is separate from the dense correction-history fixture above.

## Qualification and source binding

To remeasure the original implementation, apply the reversal in an isolated source copy, then restore the frozen example and profiling runner from this release. Those two harness files are outside the application core fingerprint. Check the restored original core and both harness hashes against the before report before running; use separate output paths to preserve prior evidence.
