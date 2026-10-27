# Tender expiry without quiet full-workspace reads

The expiry sweep now checks a small tenant deadline index before requesting a full typed workspace. Quiet tenants avoid transferring and decoding their retained histories, documents, finance and operations records just to discover there is no overdue tender. Expiry still records an exception rather than cancelling transport, and late actual carrier acceptance remains valid.

## Source and scheduling contract

Migration **025**, with business format still **18**, adds `freight_tender_deadlines`. It stores only tenant identity, source revision, readiness and the earliest possible expiry time. Original loads, deadlines, exceptions, business versions, audit and receipts remain in the authoritative tenant aggregate. Migration backfill preserves source JSON, timestamps, physical row revision and original receipts. Back up the whole schema, including this derived table and its functions/trigger.

Source writes invalidate the hint atomically without inspecting the large payload during ingestion. The first subsequent expiry preflight locks the tenant, rechecks readiness, and builds its deadline hint from the current source. Later quiet preflights read only scalar metadata. The source revision survives logical restore; missing or stale metadata cannot silently hide eligible work. A cache rebuild changes no business records. An already prepared preceding writer still invalidates the hint correctly; preceding executables refuse the newer migration ledger at startup.

A positive hint retains the existing full typed preflight and conditional mutation under the tenant lock. The native model rechecks status, exception, precise deadline and version. A concurrent acceptance can make a previously positive hint ineligible; no overdue exception is then committed. Competing expiry calls record one exception/event, and version overflow rolls back the entire business change.

PostgreSQL timestamps have microsecond precision while native timestamps retain nanoseconds. The hint errs one microsecond early; it does not change the stored deadline or the native strict expiry comparison. Fractional leap seconds and unsupported/malformed expiry inputs request a full typed check. A quiet hint validates expiry inputs only: it does not certify unrelated retained models. An ordinary workspace read still rejects malformed history. Memory-mode behavior remains unchanged.

## Verification

The new PostgreSQL scenarios exercise concurrent migration starters, physical backfill preservation, an already prepared old writer, missing-hint recovery, quiet expiry without decoding deliberately malformed unrelated history, timestamp precision, leap-second fallback, malformed expiry refusal, exact format fencing, the actual scalar query plan, reconnects, interleaved acceptance, competing expiries, idempotent idle behavior and atomic version-overflow refusal. Existing late signed carrier acceptance, history snapshot/cold-conflict and lifecycle/finance checks remain in the full suite.

## Memory evidence

The profiler samples the owned PostgreSQL cgroup and postmaster/child process status without adding SQL observer sessions. It continues between requests, including while the controller verifies or compresses returned source data. Final OOM diagnostics are mandatory even if every foreground request succeeds. The first tracer lacked that final guard and missed these gaps; its superficially successful result is explicitly unqualified because the retained kernel evidence records an OOM kill.

| Continuously monitored run | PostgreSQL high-water MiB | Limit hits | OOM kills | Continuous samples | Between-request samples |
| --- | --- | --- | --- | --- | --- |
| Preceding API | 760.54 | 0 | 0 | 315 | 225 |
| Deadline-guard API | 755.63 | 0 | 0 | 280 | 196 |

Both runs use the same retained exact fixture executable and fixed-clock 100/4, 1,000/4 and 1,000/16 shipment/history inputs, two database CPUs, a 768 MiB database limit and two API async threads. The original failing preceding run is retained alongside the favorable preceding repeat; not every preceding run fails. Sampling adds Docker calls and changes cadence, and the runs are not counterbalanced. These observations are not a latency comparison, isolated causal memory estimate or sustained-capacity qualification. Cgroup file/shmem counters overlap and process RSS includes shared pages.