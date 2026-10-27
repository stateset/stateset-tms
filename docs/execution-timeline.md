# Shared shipment execution timeline

Shipment details, current carrier offers, and customer shipments now show the same route-ordered actual observations. Expand **Execution timeline** to inspect each stop's gate arrival, actual pickup/delivery service, and gate departure. Missing times remain unrecorded. The timeline does not estimate travel, derive an ETA, fill missing gates, or infer service from arrival.

## One chronology across evidence sources

Known actual times follow origin arrival → origin pickup → origin departure → each intermediate arrival/service/departure → final arrival → final delivery → final departure. Origin/final service times come only from applied native carrier pickup/delivery reports; intermediate service times come from their immutable confirmations. Gate times use the original timezone-qualified observations and current explicit corrections.

Previously the gate sequence and service sequence were checked independently. Both sequences could be individually valid while contradicting one another across stops. The shared validator now checks the combined route before committing a new gate, service confirmation, correction, or applied native pickup/delivery. Its error identifies both conflicting stop numbers, observation kinds, and actual UTC times. Rejection changes no load version, evidence, audit, queue, or idempotency receipt.

Older native observations retained as `stale_ignored` remain in the carrier's original history but do not redefine an applied pickup/delivery milestone or constrain a later service correction. Delayed native evidence that actually advances shipment status still participates in chronology checks. Raw EDI observations retain their existing lifecycle behavior. EDI processing/receipt timestamps are not silently converted into timezone-qualified stop times.

## Repair existing conflicts without losing history

Reading an older inconsistent shipment exposes the original observations, conflict pairs, and a route attention item. It does not rewrite history or invent a corrected time. Correct original gate/service evidence through the existing operations controls, with actual supporting findings and the current load version. Each correction retains its predecessor.

When independent historical conflicts exist, one correction can repair one conflict while others remain visible. It cannot introduce a new conflict or leave its own corrected observation contradictory. Finance stays held until the known timeline is consistent, every required intermediate service is confirmed, and consolidated POD is finance-reviewed. Existing posted-evidence locks remain effective.

Applied native pickup/delivery times can now be repaired through [reviewed movement corrections](movement-corrections.md). The original authenticated report remains unchanged; a source-bound correction supplies the effective actual time. Raw EDI documents, location reports, physical status retractions, and retired assignments remain outside this workflow. Do not change correct gate/service times merely to force consistency.

The current tender scopes the view. Retired observations do not constrain or expose a replacement assignment. Staff see original evidence references and actors. Carrier/customer projections share operational kinds/times and facility order while omitting staff references, notes, and identities. A carrier's separate own-submission history remains available under its existing scope.

## Finance and Sync

Chronology consistency participates in delivery evidence readiness: carrier invoice approval, settlement preparation, and customer billing cannot proceed on conflicting execution times. Existing postings remain; this does not reverse or invent a payment.

Ordered Sync tracking adds `trackingData.freight_execution_consistent`. A final-delivery payload with a false flag stays `local_only`. The worker also checks current chronology for older final-delivery payloads without the flag, and rechecks its hold under the tenant lock. Original payloads and any previous transmission attempt remain recorded; Retry cannot force `local_only` payloads upstream. A later consistent correction enqueues a fresh snapshot, allowing final tracking on the original order UUID. In-transit physical tracking remains independent of final-delivery approval.

The marker is Freight metadata in Sync's raw tracking JSON. Canonical typed tracking continues exposing supported public fields such as tracking number and estimated delivery. No sibling repository is changed.

## Persistence and verification

Existing operations permissions govern gate/service and reviewed native time corrections; current carrier scope governs native movement reporting. The timeline itself is derived; the [native correction endpoint](movement-corrections.md#atomic-source-bound-correction) appends separately retained evidence. `GET /v1/workspace` adds each load's `execution_timeline: {consistent, evidence_valid, record_count, conflicts, stops}`. Partner responses add a public projection. Missing observations do not become stored records.

Migration 017 introduced the chronology fence in format 15; format 16 additionally retains native movement corrections and fences workers that ignore effective corrected times. Current format 17 retains operations ownership/follow-up history and source-derived recovery. These upgrades preserve business fields, timestamps, original documents, and migration history. Inconsistent history remains available for explicit repair rather than being normalized during migration.