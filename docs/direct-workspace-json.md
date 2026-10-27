# Direct staff-response serialization experiment

The current [derived-index admission release](derived-index-admission.md), source `6b520248`, integrates the direct serializer with bounded catalog admission and tenant coordination for cold index builders after complete 610-scenario and 71-check recovery qualification. Candidate `59a8dae3` below remains an unpromoted historical experiment, including its failed catalog gate.

The later [catalog-admission experiment](catalog-read-admission.md) restores all 60 catalog queries in three financial-load runs. That candidate was not promoted because complete browser qualification exposed row contention; both earlier workload delivery failures remain recorded.

## Change and contract checks

The [preceding ownership trial](workspace-memory.md) still built a full JSON tree for every retained record and failed with OOM. This candidate serializes passthrough history and finance directly from their typed collections. Derived shipment, invoice, bill, settlement and quote views retain their existing calculations. An explicit root-field whitelist preserves private receipt/operating-state omission and the existing HTML redactions. The response is fully encoded before HTTP success; it does not stream a partially validated workspace.

Both the old Value projection and the new encoder use the same owned snapshot/worker pipeline, retaining admission through real snapshot commit and CPU completion. The typed workspace remains live during encoding; the large passthrough JSON tree is avoided. Encoded HTTP buffers can still outlive processing admission, and the job limit still does not bound total bytes or memory across other stores/processes/replicas.

All **100 active library tests pass**, with 32 database-only cases explicitly unrun in this stage. Three new guards compare all four complete/deferred history and approval combinations with the independent old projection, verify private-root omission and failure propagation, and check real worker ownership after cancellation and encoder failure. Existing selector tests now exercise the new encoder. Strict all-target Clippy and formatting pass. These are candidate-stage checks, not a new execution of the preceding 600 application scenarios or 71 recovery checks.

## Original financial workload

The workload keeps the exact [1,000/100-shipment financial fixtures](finance-capacity.md), including 750 carrier invoices, bills and settlements, 1,500 cash entries and 3,000 journal entries per large tenant. The API remains limited to 512 MiB/two CPUs, PostgreSQL to 768 MiB/two CPUs, without swap or allocator tuning. Work/read concurrency remains one, with two Tokio threads and two database connections. Default error logging is retained; no native build or extra process sampler runs during this profile.

The fixed schedule offers 246 arrivals: 60 full workspace reads, 60 small catalog queries, 60 health calls, 60 readiness calls and six physical location observations. All are sent; none are silently dropped or withheld. Warm checks and idle observations remain.

| Result                                         |   Observation |
| ---------------------------------------------- | ------------: |
| Observed API kernel memory peak                |    460.95 MiB |
| Observed PostgreSQL kernel memory peak         |    479.28 MiB |
| OOM, memory-limit faults, transport failures   | None observed |
| Complete large reads / explicit busy responses |       39 / 21 |
| Small catalog successes / busy responses       |       28 / 32 |
| Health / readiness successes                   |  60 / 60 each |
| Physical observations committed exactly once   |             6 |
| Exact original HTTP receipt replays            |             6 |

The original application OOM-killed under this workload; this candidate survives the finite run. That does not establish a RAM bound, production sizing, sustained capacity, a service objective or a controlled CPU/latency improvement. Its image packages the new native executable on the pinned previous runtime image; the actual unchanged production Dockerfile, broader application/browser/PostgreSQL suites and complete recovery remain qualification gates for any release.

Every accepted large response matches the complete warm business result. Strict per-write classification retains exact versions, events, receipts, case history and financial originals; both untouched tenants retain their logical source exactly. The harness checks physical tuples before and after each original replay; the independent audit rechecks receipt bodies and logical originals, rather than claiming independently retained per-replay physical snapshots. Checked database dump/TOC and labelled-resource cleanup remain archived.

## Why the declared profile still fails

Every one of the 32 catalog refusals has code **`read_capacity_exceeded`**. `Store::load_catalog` obtains the same `read_permit` as whole-workspace processing before beginning SQL. With read concurrency one and a sustained large-response queue, small indexed queries can exhaust that shared admission deadline. These refused catalog requests do not reach their catalog SQL. The response bodies do not establish database-pool contention as their cause.

The original failed profile's seven catalog 503s have the same admission code. The earlier guide called them database-budget responses; that explanatory label is corrected while preserving the original reports and before-document snapshot. The base overload driver permits explicit probe-busy responses, so it reports its narrower contracts as passing. The separately declared requirement that all small catalog queries succeed remains **failed**. Neither a green base harness result nor the absence of OOM closes that requirement.