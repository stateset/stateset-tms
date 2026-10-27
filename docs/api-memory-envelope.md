# API memory envelope observations

A 512 MiB API limit exposes an actual failure with the current image's default two concurrent snapshot reads. During a simultaneous burst of eight large workspace reads, a small-tenant catalog read, health and readiness requests, the API is OOM-killed with exit 137. Nine requests disconnect. Health and readiness return 200 before the kill; those responses alone do not establish survival through the workload.

With the existing `FREIGHT_READ_CONCURRENCY` setting changed to one, the same offered burst completes all eleven requests with HTTP 200. The API's observed kernel high-water mark is **332.59 MiB**, with zero memory-limit failures, `under_oom` or OOM kills. The last large response takes **5.19 seconds**, and the small catalog probe **1.29 seconds**. This is a finite read-only overload result, with a visible queueing tradeoff. The application default remains two.

The subsequent [mixed read/write test](api-mixed-memory-envelope.md) OOM-kills the same 512 MiB API even with one snapshot read. At 1 GiB that finite workload survives, peaks at 796.45 MiB and rejects four of eight large reads explicitly. The read-only result above establishes no general 512 MiB safety guarantee.

## Workload and limits

| Setting          | Both measured profiles                                                                                    |
| ---------------- | --------------------------------------------------------------------------------------------------------- |
| API              | Actual qualified nonroot image, 512 MiB memory, no swap allowance, two CPUs                               |
| PostgreSQL       | Actual PostgreSQL 16 image, 768 MiB memory, no swap allowance, two CPUs, owned disk-backed volume         |
| Serving          | Restricted database login, `FREIGHT_MIGRATIONS=verify`, two database connections, two Tokio async threads |
| Large tenant     | 1,000 loads, 20,000 archived events, 1,000 accepted tenders, 1,000 carrier submissions, **zero invoices** |
| Probe tenant     | Separate 100-load fixture and a warmed indexed one-record catalog query                                   |
| Arrival pattern  | One simultaneous burst: eight large workspace GETs, one probe catalog GET, health and readiness           |
| Observation      | Sequential warm reads, 35 seconds idle before and after the burst; normal periodic workers enabled        |
| Changed variable | `FREIGHT_READ_CONCURRENCY=2` → `1`                                                                        |

Kernel memory, swap and CPU controller values are checked against requested Docker limits. The observer supports cgroup v1 and v2 and rejects missing counters. It includes idle intervals, observes actual container exit/OOM state, and preserves unfavorable outcomes. Short nonprivileged counter-reading processes and the default image healthcheck consume the same cgroup budgets. The host uses v1; the observed CPU/memory values and controller units remain recorded explicitly.

| Observed outcome    | Two snapshot reads                          | One snapshot read                          |
| ------------------- | ------------------------------------------- | ------------------------------------------ |
| Eight large reads   | Eight disconnected without an HTTP response | Eight HTTP 200, 654–5,191 ms               |
| Small catalog probe | Disconnected                                | HTTP 200, 1,291 ms                         |
| Health/readiness    | HTTP 200, about 486 ms                      | HTTP 200, about 4 ms                       |
| API survival        | OOM-killed, exit 137                        | Running until owned cleanup                |
| API workload peak   | Final peak unavailable after kill           | 332.59 MiB                                 |
| Original rows       | Exact values and physical tuples unchanged  | Exact values and physical tuples unchanged |

The failing run's last observed live high-water value was 329.52 MiB. It is not the peak at the crash: the process exited before another in-container counter read. Docker's final OOM state is retained. PostgreSQL stays running without observed OOM in both cases. Each fixture is logically backed up, checked with the matching `pg_restore --list`, then only the uniquely labelled containers/volume are removed. Every owned port is checked for actual connection refusal.

The one-read run also records the effective image environment and actual restricted PostgreSQL sessions before warm-up. These additional observations and every harness revision are retained. The comparison does not claim a controlled latency improvement or a tail-latency bound.

## Reproduce an owned diagnostic

Use exact cached source-bound API/PostgreSQL images, a matching codec binary/build binding, and the archived fixtures. A new plan must name the actual fixture paths and match the image/source, memory limit and read-concurrency setting. Keep previous plans and outputs intact.

```sh
python3 scripts/qualify-api-envelope.py \
  --plan /absolute/path/to/new-plan.json \
  --api-binding /absolute/path/to/api-image-build-binding.json \
  --codec /absolute/path/to/source-bound-codec \
  --codec-binding /absolute/path/to/codec-build-binding.json \
  --large /absolute/path/to/workspace-1000.json \
  --probe /absolute/path/to/workspace-100.json \
  --api-memory-mib 512 --read-concurrency 1 \
  --output /tmp/freight-envelope-new.json
```

The harness accepts no deployment database connection or operator credential. It creates a new labelled cluster/volume, runs the actual schema-only image job, creates a restricted serving login, and seeds exact canonical frames. PostgreSQL uses trust authentication in this isolated lab; Clerk, public HTTPS and database TLS are outside this resource profile. The matching source's separate TLS/SCRAM recovery qualification remains separate evidence.

Ten [counter regression tests](../scripts/test_api_envelope.py) cover both controller versions, missing/duplicate counters, OOM/limit-pressure detection, real memory/swap/CPU limits and native units. A real SIGTERM smoke interrupts an owned child after setup, retains unchanged originals and a checked logical archive, and confirms complete labelled-resource cleanup. The intentional interrupted result is not counted as a capacity pass. After an abrupt controller kill, inspect only the ownership label in that run's report before handling remaining resources.

```sh
python3 -m unittest discover -s scripts -p test_api_envelope.py -v
```

## Remaining capacity work