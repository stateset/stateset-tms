# Full workspace reads with retained stop history

The subsequent [original-history workspace improvement](deferred-history.md) adds compact staff refreshes and on-demand original histories, preserving this earlier comparison and its source-bound measurements.

The staff workspace now prepares current-gate and first-active appointment lookups once per immutable projection. The actual full-workspace HTTP comparison is faster with identical decoded responses, while unpaced dense API memory use increases. The observations and limitations below are retained together.

The staff workspace previously selected current gate events separately for stop execution, the execution timeline and chronology attention on each shipment. Each selection rebuilt a global supersession set and scanned retained gates. Stop execution also searched every appointment for each route stop. The change reuses the existing borrowed gate index and a first-active appointment lookup inside one immutable workspace read. Single-shipment selectors, correction rules, original record order and returned history remain unchanged.

The appointment lookup must preserve the existing first-match rule, including duplicate active input rows. It must not silently choose the newest date or last row. Appointment and gate lookups are discarded before retained arrays move into the encoded response; they never survive a workspace snapshot.

## Measurement protocol

The [fixture](../examples/workspace_history_fixture.rs) extends the preceding HTTP fixture with earlier tender gate identities, explicit current-pickup correction chains, retained appointment rows and closed delivery gate visits on half the shipments. A gate visit does not advance custody or delivery state. Existing native pickup observations, current legacy tender snapshots, twenty operating events per shipment and retained case follow-ups remain in the data.

| Shipments | History depth | Gate events | Appointment rows |
| --------- | ------------- | ----------- | ---------------- |
| 100       | 4             | 1,100       | 500              |
| 1,000     | 4             | 11,000      | 5,000            |
| 1,000     | 16            | 35,000      | 17,000           |

Each depth adds one earlier-tender gate, one linked current-pickup correction and one inactive appointment per shipment. A separate active delivery appointment remains. All current visits have closed dwell so changing live dwell minutes cannot be hidden by comparison normalization. These are typed synthetic records; earlier tender identities do not include a complete historical carrier/contract graph, and cancelled appointment labels are not a qualification of their command history. Actual lifecycle and recovery checks are separate gates.

The [runner](../scripts/profile-workspace-history.py) builds the actual Freight API and generator with the locked offline release profile. It owns a fresh PostgreSQL 16 container and loopback API process. Each fixture receives one untimed warmup and seven timed full `/v1/workspace` HTTP reads at concurrency one. Timing covers the HTTP request through receipt of the complete response body, including database transfer, decoding, projection and encoding. Python validation, hashing and archival are outside the timed interval.

Every response retains all original gate, appointment, tender, native observation, event, service and correction rows. A separate Python selector checks exact shipment/tender boundaries, supersession, first active appointments, selected arrival/departure identities and known timeline counts. The full decoded response hash must match between samples and implementations after removing only `operations.generated_at`, whose timestamp is validated and retained separately. Complete raw responses and input fixtures are archived as deterministic gzip files with their own hashes. Physical workspace revisions and retained database bytes must remain unchanged.

Host, compiler, runner, supporting harnesses, fixture bytes, workload dimensions and sample counts must match. Application or harness edits during measurement invalidate the run. Linux process observations and PostgreSQL OOM diagnostics remain part of qualification, including unfavorable resource results.

```bash
python3 scripts/profile-workspace-history.py \
  --output /tmp/freight-workspace-index-before.json
# After applying and checking the implementation:
python3 scripts/profile-workspace-history.py \
  --baseline /tmp/freight-workspace-index-before.json \
  --output /tmp/freight-workspace-index-after.json
```

## Correctness and compatibility gates

Three new workspace regressions pass against both the original and indexed handlers. They cover correction and tender scope, first active appointments, retained row order, refreshed snapshots, no current tender, cancelled shipments, chronology conflicts and damaged movement evidence. Full Rust/PostgreSQL checks, actual EDI codecs, production-browser scenarios and coordinated actual Freight/EDI/Sync recovery pass with the source bindings below. This change adds no persisted field, schema migration or new access capability.

## Recorded HTTP results

Milliseconds, median of seven release-build full-workspace HTTP reads; bracketed values are the observed minimum–maximum:

| Shipments / depth | Original                        | Indexed                      | Response bytes |
| ----------------- | ------------------------------- | ---------------------------- | -------------- |
| 100 / 4           | 105.23 [96.06–108.74]           | 84.82 [78.68–89.13]          | 2,457,854      |
| 1,000 / 4         | 2,809.68 [2,709.07–3,108.78]    | 895.31 [849.31–946.12]       | 24,672,407     |
| 1,000 / 16        | 14,336.32 [12,846.14–16,287.65] | 1,489.93 [1,450.82–1,688.76] | 47,460,407     |

These payloads retain the original returned history; no response page, record or evidence is truncated to obtain the improvement. The largest response remains about 47.5 MB. This change reduces repeated selection CPU; payload size, whole-tenant transfer/decoding and browser rendering remain costs.

## Memory and request cadence

Sampled maximum API RSS in MiB, including warmup observations:

| Shipments / depth | Original | Indexed |
| ----------------- | -------- | ------- |
| 100 / 4           | 77.7     | 80.4    |
| 1,000 / 4         | 649.2    | 604.0   |
| 1,000 / 16        | 736.4    | 974.5   |

The densest unpaced workload improves latency but increases sampled RSS from **736.4 to 974.5 MiB**; observed high-water memory rises from about 783.4 to 1,021.7 MiB. This is an unfavorable retained resource observation, not a memory improvement.

These observations support cadence-dependent worker/allocator residency as a contributor. They do not establish allocation ownership, prove absence of leaks or justify a production memory bound. The faster closed-loop requests arrive more frequently; adding idle time is a diagnostic control, not a replacement for the unpaced workload. No allocator or runtime policy is changed in the product. Sustained arrivals, concurrent dense reads, API memory quotas and allocation attribution remain further work.

## Ordinary mixed read/write workload

The separate existing 1,000-shipment HTTP/PostgreSQL pair runs all 109 timed requests per variant: 102 HTTP 200 responses and seven expected version conflicts. Its fixtures have 2,000 gate events and no appointment history, so this is a different workload from the dense comparison. Both variants pass tenant, original-record, idle-write, replay, version-race and database-OOM guards. All observations are retained:

| Wave / kind / clients                                          | Original p95 ms | Indexed p95 ms |
| -------------------------------------------------------------- | --------------- | -------------- |
| operations-read / operations / 1                               | 460.3           | 300.9          |
| operations-read / operations / 4                               | 877.5           | 715.5          |
| operations-read / operations / 8                               | 1,769.9         | 1,304.8        |
| mixed-tenants-and-health / health / 8                          | 324.0           | 202.3          |
| mixed-tenants-and-health / operations / 8                      | 1,444.0         | 862.6          |
| mixed-tenants-and-health / probe / 8                           | 1,072.5         | 639.0          |
| mixed-tenants-and-health / workspace / 8                       | 1,651.0         | 1,113.3        |
| same-load-version-race / competing-version / 8                 | 2,719.4         | 2,646.1        |
| serialized-writes-and-reconciliation / distinct-load-write / 5 | 3,302.5         | 3,814.3        |
| serialized-writes-and-reconciliation / reconcile / 5           | 4,300.1         | 3,100.1        |

Ordinary-workload sampled API maximum RSS is 656.2 → 628.8 MiB. Read observations improve, while distinct-write p95 increases from 3,302.5 to 3,814.3 ms. These finite shared-host runs do not establish statistical confidence, causal attribution for every category, uniform latency/memory improvement, throughput or a service-level objective.

## Qualification and source binding