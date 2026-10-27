# Reviewed native movement times

Operations can correct an applied native carrier pickup or delivery **actual time** from shipment details. The original authenticated carrier report remains unchanged. A correction appends evidence; it does not retract custody, rewind shipment status, change location or the latest tracking timestamp, revise a carrier agreement, accept POD, or move money.

## Review the original report

Open **Native movement time review**, inspect the original actual time, carrier reference, and authenticated reporting actor, then choose **Correct pickup time** or **Correct delivery time**. Enter the corrected UTC instant, actual carrier contact, supporting reference, and findings. Confirm that the exact original report and supporting evidence were reviewed. Previous corrections remain visible with their original actor and recording time.

The named operations actor attests to this review. The correction does not claim that the original carrier user authenticated or approved the new time. External driver/facility correspondence remains operator-attested evidence. A raw database owner could rewrite records and hashes; these digests detect retained-evidence corruption under the application's write contract, not a malicious database administrator.

Only an applied native `pickup` or `delivery` report belonging to the current load, tender, and carrier can be corrected, while the load is in transit or delivered. Location reports, `stale_ignored` reports, retired assignments, raw EDI documents, physical status changes, and movement retractions are outside this workflow. EDI processing timestamps remain distinct from actual stop times. Correct source evidence rather than changing a correct gate/service time to force agreement.

## Atomic source-bound correction

`POST /v1/carrier-submissions/{id}/time-corrections` requires `operations.write`, an idempotency key, and exactly these fields:

```json
{
  "expected_load_version": 12,
  "source_sha256": "<the exact source digest from movement_sources>",
  "expected_previous_id": null,
  "observed_at": "2026-10-15T17:30:00Z",
  "contact": "Actual carrier dispatch supervisor",
  "reference": "Actual driver and facility correction reference",
  "note": "Reviewed findings supporting this corrected actual time"
}
```

Use the latest correction ID as `expected_previous_id` for a later correction. The endpoint rejects unknown fields, future or unchanged times, missing findings, changed source bytes, a stale load version, and a changed correction predecessor. Actor and recording time are server-owned. A successful retry returns its original receipt even after later corrections; a failed request consumes no key.

The source digest binds the entire canonically serialized stored carrier submission, including its original time, actor, offer/tender/carrier identity, reference, note, location, and outcome; it is not a digest of the original HTTP request bytes. Each correction retains that digest and a digest of its own evidence, plus an explicit predecessor. Verification checks every retained link and the original source, including superseded corrections. A corrupt source, correction, or chain exposes an evidence-review hold rather than silently accepting a different time. Restore/reconcile original records through controlled incident recovery; this endpoint cannot approve corrupt history.

The existing tenant transaction commits the correction, incremented load version, named audit, idempotency receipt, and fresh Sync snapshot together. Competing reviews against one version cannot both commit. The immutable rate document and its original SHA-256 remain unchanged. The financial evidence lock includes an approved carrier invoice, any prepared settlement, or any nonvoid customer bill, including a draft. Correct actual times before those commitments; this workflow does not rewrite posted finance.

## Timeline, privacy, and integration

The [shared chronology](execution-timeline.md) uses the effective corrected time. It still validates all known route-ordered arrival/service/departure facts. A correction cannot introduce a contradiction or leave its own source contradictory. Independent older conflicts can be repaired incrementally; finance and final delivery writeback remain held until the complete required evidence is consistent. Intermediate service boundaries use corrected native pickup/delivery times rather than the original mistaken timestamp.

Staff receive the exact original source and retained correction history. Carrier/customer timelines show the effective time, a correction marker, and the original time without staff contact, reference, findings, or actor. The original carrier's own submission history retains its original time and additionally identifies its effective corrected time. Retired offer scope remains historical.

Sync receives `trackingData.freight_actual_milestones`, an array containing only `kind`, effective `observed_at`, and `corrected`. New native reports are included in the snapshot created by their own transaction. Correction snapshots retain the same source order UUID, physical status/location, route, and tracking watermark. No private correction evidence enters this metadata. These custom fields survive in Sync's raw tracking JSON; its typed canonical order projection does not expose arbitrary Freight metadata. Existing queued payloads and completed writebacks remain immutable, ordered, and subject to the existing source/route/chronology holds. An invalid current correction chain also holds final delivery updates.

No EDI source document is rewritten or sent as a replacement movement by this operation. Existing signed callbacks, native reports, and rate-confirmation transport keep their original contracts.

## Persistence and qualification

Migration `018_movement_corrections.sql` advances storage format 15 to **16**, fencing older writers/workers that could discard corrections or ignore effective chronology. It verifies prior markers and changes only format markers, preserving original business fields, timestamps, documents, and migration history. Missing older `movement_corrections` defaults to an empty collection when decoded. See [deployment and rollback](storage.md).