# Operations control tower

The subsequent [original-history workspace improvement](deferred-history.md) adds compact staff refreshes and on-demand original histories, preserving this earlier comparison and its source-bound measurements.

**Control tower** turns current operating facts into a queue of owned work. It shows what needs attention, the responsible subject, the next follow-up time, and retained source/decision history. It does not change custody, invent tracking, approve POD, authorize a source change, revise an agreement, or execute a payment.

## Detection and recovery

The queue derives issues from these authoritative records:

| Condition                               | Evidence                                                                                       | Operating action                                                                                                   |
| --------------------------------------- | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Reported shipment exception             | Current exception and its original report event                                                | Investigate and resolve the actual shipment exception                                                              |
| Unverifiable or contradictory execution | Current tender's retained movement/gate/service evidence                                       | Recover original records or correct the actual mistaken source                                                     |
| Delivered route with missing service    | Physical delivery plus unconfirmed intermediate stop IDs                                       | Obtain actual service confirmations; preserve the delivery fact                                                    |
| Tender response overdue                 | Current offered tender and elapsed response deadline                                           | Review communication/response and coordinate any necessary release                                                 |
| Tracking needs follow-up                | In-transit status and missing/old tracking watermark under the tenant policy                   | Obtain an actual observation; taking ownership does not refresh tracking                                           |
| POD follow-up                           | Delivered status and missing, pending, rejected, or unavailable consolidated delivery evidence | Obtain original POD or arrange the separate finance review                                                         |
| Stop window elapsed without arrival     | Active facility-confirmed window, no recorded arrival, and no physical service completion      | Check actual arrival with the facility/carrier; no ETA or physical lateness is inferred                            |
| Commerce source review                  | Retained active source-review ID/digest                                                        | Review canonical evidence and obtain the appropriate customer transport decision                                   |
| Carrier conflict/release                | Open tender incident or pending original release request                                       | Reconcile custody and actual carrier coordination                                                                  |
| Inbound/outbound failure                | First retained error in an active recovery state, or a held integration job, with original document/destination | Inspect recovery; the same issue stays tracked through pending, sending, processing, and retry states |

Each issue has a stable ID derived from load, condition kind, and its tender/appointment/incident/job subject. Different causes remain separate. Source fingerprints include the relevant facts rather than a changing display age or every unrelated load version. The original source snapshot is retained when detection, source changes, or recurrence is recorded.

[First-failure detection](integration-failure-detection.md) opens integration work on the first retained failed attempt, before the retry limit is exhausted. A never-attempted pending job without an error creates no failure issue. Clearing the error to schedule another attempt preserves the existing issue and ownership; a terminal source outcome clears this recovery condition. Closure records that queue condition only: acceptance by an EDI gateway is not carrier acceptance, and a document routed to finance review still requires its separate review.

The normal Store transaction reconciles work after successful source mutations, together with the new source state. Idempotent replay does not manufacture another detection or source change. A background worker additionally checks existing tenants every 30 seconds for time-based conditions, avoiding writes when no reconciliation is needed. Explicit **Refresh issues** reconciles for operations staff; viewers refresh the current read projection. Source-derived issues can appear before an existing workspace is indexed; their unrecorded historical detection time remains unknown.

Closure follows the current source condition. Operators cannot mark an active problem “resolved” from the queue. For example, a critical missing-stop case stays active after ownership and rejects deferral; an actual service confirmation clears the condition and permits the existing safe Sync workflow. A handoff retry alone is not success: retained failure work stays active until the job leaves recovery states. A terminal `local_only`, superseded, ignored, or accepted outcome may end that particular condition without proving external transport delivery. Original queues remain authoritative for the exact outcome.

Source-cleared history remains available. If the same condition recurs, a new cycle is recorded under its stable case ID, the previous owner/deadline/deferral are cleared, and new responsibility must be claimed. A changed source wakes deferred work while preserving an active owner. Critical holds are never deferred or hidden by ownership.

## Named handling

1. Open the shipment or integration source and review the actual condition.
2. Choose **Take ownership**. The server assigns the authenticated subject, never a client-supplied person. Record findings and a UTC follow-up within seven days.
3. Add notes or update your follow-up as evidence arrives. Other operations staff may add a note without taking ownership or changing your deadline.
4. For noncritical work, defer to a future time within 24 hours and no later than the next follow-up. Deferred work remains available in its own view; expiry, source changes, or explicit resumption makes it actionable again.
5. Release your own responsibility after coordination. An administrator can explicitly release an unavailable colleague with a reason. This does not assign responsibility to someone who has not claimed it.
6. Complete the original operating workflow. Source-verified closure preserves the original ownership, notes, source snapshots, and recording actors/times.

The views include needs action, all active, assigned to me, unassigned, deferred, source cleared, and all history. Search matches shipment reference, issue title, or condition summary. Pages contain up to 100 issues, with 25 in the UI. Cursors bind tenant, authenticated subject, filters, page size, and current membership/order; changed queues require refresh instead of silently skipping work. Lists omit long histories; open **View issue history** to fetch them separately. Automatic refresh pauses while multiple pages are displayed so review position is retained. Source/version checks protect an open review even when other work changes behind it.

## Access and API

Staff `workspace.read` permits queue and case-history reads. `operations.write` permits human handling and explicit reconciliation. `operations.configure` is administrator-only and controls the tenant tracking freshness interval, default 240 minutes and configurable from 15 to 10,080 minutes. Policy changes require a reviewed policy version and reason; the original/current values and named actor are audited. Finance-only, integration-only, and viewer roles cannot claim work. Partner roles cannot access staff cases, notes, owners, metrics, or policy. Clerk organization-bound requests and live authorization apply before case replay.

| Method | Route                                | Contract                                                                                                                                  |
| ------ | ------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------- |
| GET    | `/v1/operations`                     | Strict optional `view`, `search`, `limit`, `cursor`; defaults to actionable/25; returns issue summaries, metrics, policy, and next cursor |
| GET    | `/v1/operations/issues/{id}`         | Current source plus retained complete case/history, or 404 within the authenticated tenant                                                |
| POST   | `/v1/operations/issues/{id}/actions` | Source/version-reviewed action below; idempotency is also bound to the authenticated human subject                                        |
| POST   | `/v1/operations/reconcile`           | Body exactly `{}`; rechecks current sources without changing their business state                                                         |
| POST   | `/v1/operations/policy`              | Administrator; `{expected_version, tracking_stale_minutes, note}`                                                                         |

Example claim body:

```json
{
  "expected_version": 1,
  "expected_source_active": true,
  "source_sha256": "<reviewed current issue source fingerprint>",
  "action": "claim",
  "note": "Actual facility contact is checking the original arrival receipt",
  "follow_up_at": "2026-10-17T18:00:00Z"
}
```

Actions are `claim`, `release`, `force_release`, `note`, `snooze`, and `resume`. `force_release` requires an administrator before mutation or replay. `snooze` uses `defer_until`; only claim/note may specify `follow_up_at`, and only the current owner may change an active deadline. Both timestamps require an explicit RFC3339 offset. Notes require 1–200 bytes. Unknown fields and unused timestamp fields fail. Actor/admin status, owner, source snapshots, cycle, recording time, version, and evidence digest are server-owned.

Use version zero only for an issue that has no persisted case yet. The server checks the exact current source, its reviewed active/cleared state, and the current case version under the tenant lock. Even an identical condition recurring before the clock worker runs cannot accept an old inactive-source review. Competing claims cannot both commit. A changed source or version returns 409; failed actions consume no idempotency receipt. Another subject cannot reuse a claim's key to retrieve that human assertion. A successful retry by the same subject returns its original result after later history, with no new mutation.

## Metrics and integrity

Metrics distinguish active/critical/unassigned/deferred work and overdue owner follow-ups. Historical response metrics include detected cycles, acknowledged cycles, valid timed samples, unavailable response timings, and median seconds to the first claim per cycle. Timing begins at durably recorded detection, not an invented historical first-seen time. Releases/reclaims in one cycle do not inflate first-response counts. Clock-skewed observations retain their actual timestamps and claims but contribute no fabricated zero-second response. These measure this implementation's observations; production improvement requires a real operating pilot and independent outcome measurements.

Case digests cover the retained source, state, and entire history; source hashes bind the reviewed condition. An integrity failure blocks case mutation and suppresses unverified ownership/timing metrics. Malformed or unsupported case/policy structures are quarantined and serialized back with their original JSON values, so a real tracking or other source transaction cannot silently drop them or become unavailable merely because this work projection is unreadable. Invalid policy does not produce a guessed tracking-freshness classification. This exception is limited to the work projection; unknown business roots and malformed shipment/finance aggregates keep the storage contract's existing error behavior.

Quarantine is preservation, not automatic repair. Recover from authoritative original records, backups, receipts, and incident evidence through a qualified procedure. Do not drop the work index to clear the warning or overwrite current tracking/finance with an older whole-workspace snapshot. Reconcile missed source changes and fence stale reviews before restoring human handling. Digests detect accidental/unauthorized application-contract changes; they do not authenticate a malicious database owner who rewrites records and hashes.

## Storage and qualification

The newer [full-workspace history comparison](workspace-history-performance.md) extends borrowed selection to the staff workspace while retaining first-active appointment order, chronology holds and complete returned history. Its source-bound HTTP results include the dense memory increase and a separate cadence probe.

Gate and elapsed-appointment selection now uses borrowed lookups within each source calculation. Exact shipment/tender selection, correction order and clock/snapshot boundaries remain unchanged. The [retained stop-history comparison](stop-history-performance.md) records the measured CPU reduction and mixed HTTP latency/resource outcomes; whole-tenant storage and other projection scans remain costs.

Migration `019_operations_work.sql` advances format 16 to **17**, preserving existing business fields, timestamps, documents, and prior migration history. Older writers/workers are fenced before they can discard the work projection or bypass its recording behavior. Legacy missing work/policy fields default to an empty index and default policy without inventing past detections. See [deployment and rollback](storage.md).

Ten focused Rust tests cover detection, ownership/deadlines/deferral, recurrence and metric denominators, source/version/subject-bound replay, administrative access and partner privacy, pagination scopes, critical holds/retry follow-through, corruption/quarantine preservation, and honest clock-skew handling. The disposable PostgreSQL scenario uses independent instances for competing claims and source-clear/deferral races, verifies receipt/audit rollback and exact restoration, retains history after reconnect, checks tenant isolation, and records actual tracking through both digest-corrupt and structurally malformed work/policy data while preserving the original damaged values. The production browser exercises staff/mobile handling, stale open reviews, source-verified closure/recurrence, administrator release, policy, retained history, and viewer/partner denial. The actual EDI/Sync qualification owns recovery after a signed final D1, rejects critical deferral, and closes the case only after actual intermediate proof permits complete-route Sync tracking.

[Projection performance](operations-performance.md) records the optimized before/after workloads and limits. Reads verify the complete retained index once per immutable workspace snapshot, then render only requested issue rows; summary reads avoid issue JSON. Verification is never cached across requests, and history is retained.
