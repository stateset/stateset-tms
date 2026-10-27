# Mixed read/write memory observations

Reducing snapshot read concurrency to one does **not** make the tested 512 MiB API envelope safe for concurrent reads and writes. Adding one actual location update on a separate 1,000-load tenant to the earlier read burst OOM-kills the API with exit 137. The write returns HTTP 200 and commits once before the process dies; six large reads disconnect. The separate read-only success remains valid within its narrower scope.

The same mixed workload at **1 GiB** survives with an observed API kernel high-water mark of **796.45 MiB**. Four large reads complete and four receive explicit retryable 503 busy responses. The small-tenant catalog takes **2.84 seconds**. This establishes finite overload behavior at that tested limit, with a substantial memory and queueing cost. It does not establish minimum sizing, sustained capacity, a production latency objective, or the safety of the application's default two concurrent reads.

Subsequent [allocator environment experiments](api-allocator-envelope.md) retain a failed single-arena health result and a two-arena 512 MiB finite pass with only 8.76 MiB of memory headroom. They leave application and deployment defaults unchanged and establish no general 512 MiB safety guarantee.

## Actual offered workload

Both profiles use the actual qualified nonroot API image, restricted verify-only PostgreSQL login, two database connections, two Tokio threads and two CPUs. PostgreSQL has 768 MiB, two CPUs and an owned disk-backed volume. Memory-plus-swap limits equal memory limits. Authentication is isolated lab trust; production identity and database TLS are outside this resource test.

The unchanged fixtures supply a 1,000-load read tenant, a 100-load probe tenant and a separate writer tenant seeded from the same exact 1,000-load fixture. Each large tenant has 20,000 archived events, 1,000 accepted tenders, 1,000 original carrier submissions and no invoices. Complete warm workspaces and the probe catalog precede 35 seconds idle. A single barrier then offers twelve requests together: eight complete large-tenant workspace reads, one writer location command, one probe catalog, health and readiness. Another 35 seconds idle follows. These are finite simultaneous requests, not a steady arrival stream or a demonstrated internal allocation schedule.

| Observation              | 512 MiB, one snapshot read            | 1 GiB, one snapshot read                        |
| ------------------------ | ------------------------------------- | ----------------------------------------------- |
| API survival             | OOM-killed, exit 137                  | Running through workload and replay             |
| Eight large reads        | Two HTTP 200; six disconnect          | Four HTTP 200; four explicit 503 busy responses |
| Accepted large-read time | 1.54–2.79 seconds                     | 1.51–5.31 seconds                               |
| Small catalog            | HTTP 200, 2.76 seconds                | HTTP 200, 2.84 seconds                          |
| Location write           | HTTP 200, 1.72 seconds                | HTTP 200, 1.44 seconds                          |
| Health/readiness         | HTTP 200, about 3 ms each before kill | HTTP 200, about 17/9 ms                         |
| API kernel peak          | Final crash peak unavailable          | 796.45 MiB                                      |
| PostgreSQL kernel peak   | Process remains running               | 383.62 MiB                                      |
| Proven original replay   | Not submitted after API death         | HTTP 200, 14 ms; physical source rows unchanged |

The successful profile has zero observed memory-limit failures, `under_oom` or OOM kills. Actual kernel memory, swap and CPU controller limits are checked, and the observer rejects missing counters. Health success before a later OOM does not establish survival. The successful profile's four rejected reads retain `Retry-After: 1`, `Cache-Control: no-store` and the declared busy code. No automatic resubmission hides a failure or rejection.

## Original write and recovery evidence

The command specifies the original version, one synthetic observed location and one idempotency key. Exact stored originals and the original receipt establish one incremented load version, the new location and observation timestamps, one appended actor-attributed event, and the matching command fingerprint and original response. Unrelated load fields, other loads, original events, receipts and other business roots remain exact. The read and probe tenants retain their exact original values.

The real location observation also resolves that load's stale-tracking case. The guard permits precisely this derived change: the original case identity, source, ownership and history remain retained; one `source_cleared` history event is appended, the version increments once, and the original and resulting source/evidence hashes verify. Unrelated cases remain exact. The guard validates the actual nine-digit Chrono timestamp format without changing retained timestamp strings.

At 1 GiB the harness first proves commitment from PostgreSQL, then replays only the same command and key. The original response returns with `replayed: true`; physical header/block tuples stay unchanged. An absent original receipt is never treated as permission to blindly resubmit an uncertain write. Each cluster has a checked logical dump and matching `pg_restore --list` before owned resources are removed. Labelled container/network/volume absence and actual refusal on both owned ports are checked separately. These backups are fixture preservation, not another restore qualification.

## Repeat and remaining work

Use [qualify-api-mixed-envelope.py](../scripts/qualify-api-mixed-envelope.py) with the same explicit image/codec bindings and fixture arguments as the [read-only harness](api-memory-envelope.md#reproduce-an-owned-diagnostic). Its plan must additionally declare the writer fixture/command and the precise operations reconciliation. The published plans and both historical harness versions remain available. Preserve prior outputs when selecting new paths or limits.

```sh
python3 -m unittest discover -s scripts -p 'test_api*envelope.py' -v
```

Seventeen harness regression methods cover controller counters and uncertain-write classification, including partial commitment, wrong receipts, changed money/other loads/history, invalid evidence seals and malformed timestamps. These are harness checks, not seventeen additional application acceptance scenarios.