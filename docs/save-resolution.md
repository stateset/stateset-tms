# Resolving an uncertain original save

The subsequent [browser save journal](browser-save-journal.md) restores these original requests after reload or tab reopening, while retaining this server-side resolution contract. Browser memory-only limits below describe this release’s historical UI.

**Resolve original save** finishes an uncertain shared-workspace request using the same tenant transaction lock as normal application. If its receipt already contains a completed result, resolution returns that original result. Otherwise, resolution records a durable stop for the original request. A later copy cannot apply it. An already completed shipment/customer/financial change is retained; any subsequent operational reversal uses its existing domain workflow.

Open **Stop waiting for this save** in the recovery panel and enter a resolution note. The server records the verified actor, note, original path and time when it creates a stop. Repeating resolution retains the first decision, including its original note and time. The original input/key remain held while resolution itself is uncertain. Checking the original request can also recognize a previously recorded stop through HTTP 409 with `error.code = request_stopped`.

## Transaction contract

| First transaction to acquire the tenant lock | Resolution result | Delayed original |
| --- | --- | --- |
| Original save commits | `completed`, with its original saved result | Replays the existing result |
| Resolution records a stop | `stopped`, with immutable resolution evidence | Rejected before business application |

Repeated resolution returns the existing decision without rewriting the workspace. A stopped receipt occupies the same key/fingerprint namespace as successful receipts. A different original path, entity or body conflicts; it cannot overwrite either outcome. A deliberately new request uses a new key and still passes its normal validation, version, qualification and source checks.

The resolver changes receipt/audit state; it never submits the original business operation. It does not retract upstream work or erase separate canonical-source observations that happened before the original business transaction. For example, stopping an unapplied queue-retry request leaves already queued or running carrier work under its existing lifecycle.

The API is `POST /v1/save-recovery/resolve`:

```json
{
  "request_key": "the-original-Idempotency-Key",
  "path": "customers",
  "body": {"name": "Original customer", "contact": "ops@example.test"},
  "note": "Resolve the interrupted original before another decision"
}
```

It accepts only canonical paths for the root workspace's supported saves: load create/plan/commands/settlement/POD review, Sync/native iCommerce import, native shipment tracking, external native handoff review, invoice create/resolve/approve/entry review, customer/carrier/driver/equipment create/update, and inbound/outbound queue retry. The server maps paths to the actual registered operations and their existing role capabilities. It ignores no client-supplied actor/tenant/operation override: unsupported envelope fields, paths, queries and encodings are rejected. Viewer and partner roles cannot resolve saves; operations, finance and integration staff additionally need the original route's authority. The original key/body prove which request is being resolved; no caller-supplied success/failure claim is accepted.

The completed result is an original receipt, not a claim that the shipment still has that old version. The browser always refreshes current workspace data afterward. A failed refresh displays **Save completed** or **Original save stopped** as appropriate and retries only the read. A failed resolution preserves uncertainty and its original request. Browser metadata remains local to the page instance; durable browser restoration remains open work.

## Storage and deployment

Migration **023** advances the business format from **17 to 18**. Successful legacy receipts retain their existing fields. New stopped receipts hold an empty result plus explicit stop metadata; every replay path checks that metadata before returning business data. The migration preserves original business values and workspace timestamps, and refuses ambiguous non-null preexisting `stopped` receipt extensions rather than adopting them as new decisions.

Older releases must not interpret a stop as an empty success or apply a delayed request. The existing registry/row guards and migration ledger fence them from format 18. Follow the [storage deployment procedure](storage.md): back up complete databases/files/configuration, drain traffic and workers, apply the upgrade with the compatible release, and verify restoration and retained queues before resuming. This source reversal is not a database downgrade. No deployment database was migrated by these tests.

## Qualification

The release passes **362 distinct tests**: 229 active Rust, 53 PostgreSQL, eight actual EDI codec, 17 Node and all 55 production-browser scenarios without internal retries. PostgreSQL qualification combines a partial full run with a corrected storage-format/tail subset; the initial future-ledger fixture failure remains retained. The new resolution scenario also passes on PostgreSQL 16 and is counted once. Formatting, all-target Clippy with warnings denied, the API build and the production web build pass. The separate restore drill passes 43 checks; catalog SQL qualification passes 58 phases, including marker-only format-18 preservation. All 111 preceding evidence files remain byte-identical.

The earlier [interrupted-save](save-recovery.md) and [shipment confirmation](shipment-confirmations.md) records retain their original sources and scope. This release does not establish production operator outcomes, certified public carrier transport, deployment capacity or completion of the full platform objective.
