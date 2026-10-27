# Financial-history capacity diagnostic

The [current 600-second image profile](financial-capacity.md) offers ten times the earlier duration at the same rates, fixtures and API/PostgreSQL quotas, with explicit latency/availability budgets. All 2,460 arrivals, 60 writes, 60 original replays and financial/source checks complete; the workspace-read p95 of 7.38 seconds **fails** its declared 5-second target. The other category budgets pass. All response/original bodies remain hash-verified and every initial failure is retained. This does not erase the earlier OOM or intrusive-observer failures below, or establish production capacity.

The preceding [derived-index admission release](derived-index-admission.md) passes a separately declared finite invoice-rich profile at the same API/PostgreSQL limits, with all 246 API arrivals, six commits and six original replays. Its observer computes the same validated original JSON using one SQL aggregation; 11 equivalence cases and six before/after legacy comparisons pass. The current candidate first failed the intrusive-observer profile at 244/246 arrivals; that failure is retained. The actual old `f61e222b` image also OOM-kills under the corrected observer and otherwise matched profile. The original workload and failure below remain historical evidence. Observer work differs, so these records are not a controlled product CPU/latency comparison or production capacity qualification.

## Workload and financial records

The unchanged source `f61e222b` and its actual production Dockerfile image run with a 512 MiB/two-CPU API, a 768 MiB/two-CPU PostgreSQL container, no swap, no allocator tuning, work/read concurrency one, two Tokio threads and two database connections. The serving login is restricted and verifies the schema. This isolated diagnostic uses trust database authentication; the preceding TLS recovery keeps its separate source-bound evidence.

Two large tenants each contain 1,000 shipments, with a separate 100-shipment probe tenant. Three quarters of each tenant's shipments are delivered. Each large tenant retains:

| Records                                               |         Count |
| ----------------------------------------------------- | ------------: |
| Carrier invoices with synthetic review/approval links |           750 |
| Issued customer bills                                 |           750 |
| Prepared settlements                                  |           750 |
| Cash entries and allocations                          |         1,500 |
| Balanced journal entries / lines                      | 3,000 / 6,000 |

Existing stop, correction, appointment and operating histories remain. The [fixture generator](../examples/finance_capacity_fixture.rs) uses the domain's invoice, billing, settlement and cash operations. It constructs explicitly synthetic retained review actors and decisions and verifies their links through the public approval-evidence reader. These generated records do not demonstrate human independence, document authenticity, actual bank reconciliation or external payment execution. The fixture uses uniform USD rates and one primary invoice per delivered load, with alternating full and partial allocations; consolidated invoices, credits, tax and real finance distributions remain outside it.

An independent closed-form cents oracle matches the journal balances for each large tenant:

| Account             | Debit less credit, cents |
| ------------------- | -----------------------: |
| Freight expense     |               93,750,000 |
| Carrier payable     |              −15,625,000 |
| Freight revenue     |             −117,000,000 |
| Accounts receivable |               19,500,000 |
| Cash                |               19,375,000 |

The [traffic driver](../scripts/qualify-api-sustained-envelope.py) schedules full workspace reads, a small catalog probe, health and readiness at one-second intervals, plus one new writer-tenant location observation every ten seconds. Release times are absolute; response completion does not pace the offered traffic. Sixty epochs at offsets 0–59 seconds declare **246 arrivals**. Warm reads and 35-second idle observations bracket the traffic; the recorded dispatch/drain interval is 59.01 seconds because later connections fail immediately after the kill. This is a finite failure diagnostic, not a steady-state capacity measurement.

The client caps outstanding work at 48 and reports unsent arrivals separately. Only one writer observation can be under inspection at a time. Exact stored originals classify its outcome before another observation proceeds; an uncertain response withholds subsequent writes. No uncertain command is automatically resubmitted.

## Observed failure

The API is first observed exited about **23.05 seconds after scheduled traffic begins**, with Docker reporting **OOMKilled=true, exit 137**. Its last observed kernel memory peak is **510.22 MiB**; sampling cannot recover a final cgroup peak after the process has died. PostgreSQL remains running, with an observed 492.76 MiB peak. All 201 resource observations are retained, including live counters and terminal state; pre-kill zero counters do not override the terminal OOM evidence.

| Scheduled request                  | Complete 200 | Busy 503 | Transport failure |
| ---------------------------------- | -----------: | -------: | ----------------: |
| Large workspace read               |           11 |        6 |                43 |
| Small catalog probe                |           11 |        7 |                42 |
| Health                             |           23 |        0 |                37 |
| Readiness                          |           23 |        0 |                37 |
| Location observation actually sent |            3 |        0 |                 1 |

Two later writer arrivals are explicitly withheld after the disconnected fourth observation; **244 of 246 arrivals are sent**. They are not counted as server rejections or silently removed from the offered workload. Fast connection refusals after death make mixed-status median latency unsuitable as a responsiveness measure. The small probe also returns seven snapshot-read-admission 503s (`read_capacity_exceeded`) before the kill, so that requirement fails independently of the eventual OOM. The [direct-serialization investigation](direct-workspace-json.md) verifies these exact response codes; the earlier database-budget label was inaccurate.

All eleven accepted large responses match the complete warmed business response, excluding its generated timestamp. Three writer transitions commit exactly once with matching original receipts and HTTP responses; the fourth is proven unapplied. The final source has no later unexpected changes. Every unrelated financial root remains unchanged, and both read tenants retain their exact logical originals. HTTP original-receipt replay is not run after the API dies. The owned database is dumped and its table-of-contents checked before the exact labelled containers and volume are removed; both published ports are independently checked closed.

## Reuse and next gate

The subsequent [direct staff-response serialization candidate](direct-workspace-json.md) completes the same workload without OOM or transport failures and preserves all six observations and original replays. It still fails 32 catalog requests at the shared snapshot-admission boundary and remains unpromoted. The complete financial workload and catalog-availability gate are retained.

The subsequent [memory-attribution investigation](workspace-memory.md) retains raw process mappings and existing phase logs. Resident memory falls during idle periods and rises during processing. Its isolated five-array ownership trial preserves complete responses but also OOM-kills, so it remains unpromoted; full JSON construction and response-buffer lifetime remain engineering targets.

The generator reads an unmarked typed workspace, or one explicitly marked format 21, from stdin and writes the enriched workspace to stdout and a summary to stderr. It refuses other storage headers and unknown typed fields. The retained historical source fixture has a format-19 marker: preparation checks and removes that marker into a separately captured typed input before generation. This is fixture conversion, not a database migration. An earlier attempt also caught conflicting duplicate delivery gates; the final generator reuses existing gates rather than replacing original history. All failed attempts remain preserved.

The six new negative-control test methods cover fixed arrival times, invalid plans, financial corruption, cash allocations and empty fixtures; they pass alongside the existing nineteen memory-harness methods. The fixture passes strict Clippy and formatting. These **25 ancillary methods are separate from the retained 600 application scenarios and 71 TLS/SCRAM recovery checks**. Application, codec, schema, dependency and frontend inputs remain unchanged; no deployment or default setting changes.