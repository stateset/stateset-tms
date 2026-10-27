# Allocator environment experiments

The same 512 MiB mixed read/write workload that OOM-kills the default-allocator API survives one trial with `MALLOC_ARENA_MAX=2`. Its observed kernel high-water mark is **503.24 MiB**, leaving **8.76 MiB** beneath the limit. Four large reads complete and four receive explicit busy responses. Health takes 782 ms, the small catalog 2.88 seconds and the location write 1.71 seconds. This is a finite experimental result with very little memory headroom, not a production resource recommendation.

The earlier single-arena trial survives at **406.15 MiB** but returns HTTP 503 from `/health`, so it **fails** the declared overload contract. Both outcomes, including that unfavorable availability result, remain preserved. Neither experiment changes the application, image, Dockerfile, Compose settings or deployment defaults.

## Measured comparison

The [mixed workload](api-mixed-memory-envelope.md) offers eight complete 1,000-load tenant reads, a location command on a separate 1,000-load tenant, a 100-load tenant catalog query, health and readiness together. Exact large fixtures include 20,000 archived events, 1,000 accepted tenders, 1,000 original carrier submissions and no invoices. Each run has complete sequential warm reads and 35 seconds idle before and after its twelve-request burst.

All three 512 MiB profiles use the same actual source-bound nonroot API image, read concurrency one, two Tokio threads, two database connections, two CPUs and no swap allowance. The owned PostgreSQL volume has a 768 MiB/two-CPU budget. The experiments explicitly record the actual image's GNU libc **2.36**, requested allocator environment and absence of competing `GLIBC_TUNABLES`. The source, image, fixtures, application admission and resource limits stay fixed.

| Observation              | Default allocator, 512 MiB    | One arena, 512 MiB                   | Two arenas, 512 MiB                       |
| ------------------------ | ----------------------------- | ------------------------------------ | ----------------------------------------- |
| Declared overload result | Failed: actual OOM            | Failed: health 503                   | Passed this finite trial                  |
| API state before cleanup | OOM-killed, exit 137          | Running                              | Running                                   |
| Eight large reads        | Two 200; six disconnect       | Four 200; four busy 503              | Four 200; four busy 503                   |
| Accepted large-read time | 1.54–2.79 seconds             | 2.24–6.34 seconds                    | 1.57–5.48 seconds                         |
| Catalog probe            | 200, 2.76 seconds             | 200, 3.56 seconds                    | 200, 2.88 seconds                         |
| Actual location write    | 200, 1.72 seconds             | 200, 2.28 seconds                    | 200, 1.71 seconds                         |
| Health                   | 200, 3 ms before kill         | **503, 1.04 seconds**                | 200, 782 ms                               |
| Readiness                | 200, 3 ms before kill         | 200, 10 ms                           | 200, 4 ms                                 |
| API kernel peak          | Final crash peak unavailable  | 406.15 MiB                           | 503.24 MiB                                |
| Proven original replay   | Not submitted after API death | Not submitted after contract failure | 200, 8 ms; physical source rows unchanged |

All accepted complete business bodies, retained original writes, command fingerprints/response receipts, unchanged other tenants and the precise stale-tracking case resolution are checked. Both allocator trials record zero observed kernel memory-limit failures, `under_oom` and OOM kills. Checked logical dumps and matching `pg_restore --list` precede complete owned-resource cleanup; labelled container/network/volume absence and actual port refusal are verified separately.

`/health` and `/ready` share the database readiness check in this source. The single-arena health 503 is an actual unfavorable response, not proof that its process died. The separate `/live` endpoint is not offered in this protocol. The retained observations do not establish the internal cause of that health result.

## Interpretation and reproduction

GNU libc documents `MALLOC_ARENA_MAX` as a process-start setting for the allocation arena limit. It does **not** limit total memory. Arena internals were not instrumented here, and the experiments do not attribute the original OOM to one allocator mechanism. See the [GNU libc 2.36 documentation](https://sourceware.org/glibc/manual/2.36/html_node/Memory-Allocation-Tunables.html).

Use the [mixed diagnostic](../scripts/qualify-api-mixed-envelope.py) with its exact image/codec bindings, fixture arguments and a new output path. Its plan can additionally supply integer `api.malloc_arena_max` of 1, 2 or 4; omission leaves the image's allocator environment unchanged. The harness checks the effective setting, refuses competing tunables and records actual libc identity. These are controlled lab settings; no production secret or database URL is accepted.

```sh
python3 -m unittest discover -s scripts -p 'test_api*envelope.py' -v
```

The current suite has **19 harness regression methods**: the earlier seventeen counter/original-write guards plus two allocator-plan controls, including rejection of booleans, strings and unsupported values. These are harness regressions, not additional application acceptance scenarios.