# Integration queue deadlines

Configured integration workers previously fetched and rewrote the entire tenant workspace even when both queues were empty. The inbound claim returned a JSON null through an unconditional update; outbound dispatch also committed an unchanged coordination pass. These writes could incidentally reconcile operations work, change the physical row and generate WAL without delivering anything. The earlier capacity fixture configured no integration tenants, so its idle checks did not cover this behavior.

Migration 020 adds `inbox_due_at`, `edi_due_at` and `sync_due_at` to the existing tenant row. A database [BEFORE trigger](https://www.postgresql.org/docs/16/sql-createtrigger.html) derives them whenever queue data or the derived columns are written. Explicit attempts to overwrite the deadline columns are recomputed from the authoritative JSON. The migration backfills only metadata, preserving original JSON and `updated_at`. Storage format remains 17: the additional metadata does not change business records, and the trigger also maintains it for a still-running format-17 writer. Startup still refuses unknown migration history.

An empty queue has no deadline. A pending item without a retry time is immediately eligible. Future retry times and live leases sleep until their deadline; an in-flight item without a lease expiry remains unreclaimable under the existing contract. Held, completed and native-only records do not wake dispatch. EDI and Sync deadlines remain separate. Unsupported timestamp forms, malformed queues and invalid timestamp offsets conservatively request a full typed check, retaining the original JSON rather than discarding it or failing the backfill.

Workers now check these scalar columns under the format guard before entering the full workspace path. They bind the Freight process UTC time, matching claim eligibility rather than assuming the database server clock agrees. PostgreSQL can keep large JSON values [outside the main row using TOAST](https://www.postgresql.org/docs/16/storage-toast.html); the idle query does not select or inspect that JSON. Memory mode checks the current queue fields under its read guard without cloning the workspace. These are scheduling probes, not business reads: ordinary workspace reads and every real claim retain strict decoding and integrity checks.

A positive probe is advisory. Claims still recheck the current job, source, ordering, coordination and lease under the tenant lock. If another worker wins first, the loser rolls back instead of updating an unchanged aggregate. Expired carrier-coordination leases still become held with original attempted transmission evidence. Late inbound and outbound completions with an obsolete lease also roll back without physical writes. A callback arriving just after a negative probe is available on the next normal poll; no in-memory cache or process-specific wakeup is required.

## Reproduce the configured-worker comparison

The baseline patch reconstructs the exact source preceding this change. Build and run both versions with the same current Rust toolchain, dependencies, fixture bytes, PostgreSQL limits and worker configuration:

```bash
mkdir -p /tmp/freight-integration-before
cp Cargo.toml Cargo.lock /tmp/freight-integration-before/
cp -a src migrations /tmp/freight-integration-before/
patch --directory=/tmp/freight-integration-before -p1 < docs/evidence/integration-wakeups-baseline.patch
python3 scripts/qualify-integration-idle.py \
  --baseline-source /tmp/freight-integration-before --seconds 35 \
  --output /tmp/freight-integration-idle.json
```

Use a new baseline directory and output path. The driver verifies the reconstructed baseline fingerprint against the retained recovery report, builds locked/offline debug binaries, uses a separate baseline build directory, verifies the running migration inventory, and preserves both actual executables in its private artifact directory. It creates only labelled disposable PostgreSQL and loopback Freight processes. An optional `--baseline-binary` uses the exact historically attested binary instead of rebuilding it. The default fixture has 1,000 shipments with original pickup/gate/tender records, archived events and retained operations histories. Its configured upstream is an unused loopback URL: this is an idle-worker experiment, not delivery qualification.

Both phases retain the same fixture bytes. A database trigger counts committed workspace updates; complete physical/logical snapshots include `xmin`, `ctid`, `updated_at` and original data hashes. The recommended 35-second observation includes normal periodic sweeps; shorter allowed runs do not establish behavior across a complete interval. The experiment records a deadline query plan and observed cluster WAL, without asserting zero WAL or inferring a production capacity/latency improvement. The API has two async threads and no CPU/memory quota; PostgreSQL has two CPUs, 768 MiB memory and a 512 MiB data tmpfs. Functional workloads and recovery are qualified separately.

## Retained local observations

## Transaction and deployment limits

The [PostgreSQL regressions](../tests/integration_wakeups.rs) include an idle poll while a tenant writer holds the row, unsupported retained business data without an idle decode, future retry/lease behavior, two independent workers queued at the same row lock, original attempt history, expired lease recovery, exact physical-row preservation after obsolete completions, carrier coordination, reconnect, original JSON/timestamp preservation during backfill, invalid timestamp offsets, and an isolated schema whose SQL `now()` returns the year 2000, proving the former database-clock predicate would suppress ready inbound/outbound work while the process-clock probe runs it. Independent PostgreSQL fixtures serialize around the database-wide startup advisory lock; their worker/claim/recovery races remain concurrent within each scenario. The memory regression preserves an unreconciled original workspace and avoids creating an absent tenant.

Back up the whole database/schema and allow the existing migration lock/statement budgets for the one-time backfill. The new columns, functions and trigger are part of the recovery inventory. Do not disable the trigger or manually edit derived deadlines. Migration history remains checksummed; code predating migration 020 refuses that newer history on restart even though the business format remains 17.

For the subsequent snapshot-admission release, first reverse [read overload protection](read-overload.md) using its source patch in an isolated copy, then follow this report's existing reconstruction steps. Its original report, source and patch attestations remain historical.

Current source reconstruction now begins with the [bounded-dispatch reversal](bounded-dispatch-turns.md), then the [process-drain reversal](process-drain.md), then the [tenant-worker reversal](tenant-integration-workers.md), then the [integration-response reversal](integration-response-budgets.md), then the [runtime database-budget reversal](database-budgets.md), then follows the snapshot-overload and earlier reversal steps already described here. Historical performance reports and original patches retain their existing hashes.
