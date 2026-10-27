# Stop addresses, appointments, and gate evidence

Origin, intermediate pickups/deliveries, and final delivery share these appointment and gate controls. Ordered routes support up to twenty stops. See [multi-stop planning and actual service](multi-stop.md) for full-route documents, native carrier acceptance, service evidence, and financial/Sync holds.

Load plans now contain street addresses, postal/country codes, contacts, phone numbers, and instructions for pickup and delivery. The browser requires street/postal details when creating or revising a plan. API drafts and legacy records may omit them, but each new tender requires both stops to have one or two nonempty street lines and a postal code. Missing data is never filled with a guessed real address. Synthetic demo stops use explicitly synthetic addresses.

The `stateset-edi` 204 receives the actual stop addresses through its supported `Party` fields and emits N3/N4 segments. Wire fields enforce ASCII and length/delimiter limits: facility 60, city 30, each street line 55, at most two lines, postal code 15, and uppercase two-letter state/country codes. This is the current gateway-compatible address format, not worldwide postal validation or geocoding. Contact/phone/instructions remain operating information in Freight; the gateway request model has no appointment-window or stop-instruction field. Share windows and instructions directly with the carrier.

## Facility-confirmed appointment windows

In load details, select **Book pickup appointment** or **Book delivery appointment**. Enter facility-local start/end times, the IANA timezone, the actual facility contact, external confirmation reference, and note. Booking records human-confirmed coordination; it does not reserve capacity in a facility system or send a carrier message. Coordinate changes with the facility and carrier before recording them.

The start's local date must match the planned stop date, the end must be at/after the start within 48 hours, and all active appointment starts must follow full route order in UTC. Overnight windows are supported. Time resolution uses the supplied timezone's rules, rather than the browser/server timezone. A spring-forward time that does not exist is rejected. A fall-back time that occurs twice requires an explicit earlier/later choice; the default asks for clarification. Records retain the supplied local times, timezone/choice, resolved UTC instants, creator, and proof.

Carrier insurance and an assigned driver's license must cover the final local day of every active window. Tender/dispatch use that extended date as well as the planned delivery date and today. Booking/rescheduling an assigned shipment cannot extend beyond the recorded qualifications; record actual renewed qualifications before extending the appointment.

Rescheduling supersedes the old appointment and links the replacement; cancellation records its actor/reason. Both preserve prior windows and confirmation evidence. After a current-assignment gate arrival, its appointment baseline cannot be rescheduled or cancelled to hide lateness. Completed stop appointments also remain. Cancel a booked appointment before revising its stop details/date in the load plan. Cancelling a load or releasing a carrier does not claim the facility cancelled its booking; record actual facility cancellation separately. An active booking is retained across re-tendering until explicitly replaced/cancelled.

The [shared execution timeline](execution-timeline.md) checks gates against actual service at every other stop and native origin/final movement. A correction preserves the original and must not introduce a contradiction. Independent historical conflicts can be repaired one at a time while finance stays held.

## Actual arrival and departure

**Record pickup/delivery arrival** records an observed gate time, timezone, evidence reference, and note against the current carrier attempt and the appointment in force. Actual times cannot be more than five minutes ahead of the server clock. Arrival requires an accepted carrier assignment; delivery arrival requires pickup evidence in the shipment lifecycle. Departure requires a recorded gate arrival and the corresponding pickup/delivery lifecycle milestone.

Gate times never advance shipment status, assign fleet equipment, invent a POD, or establish finance approval. A truck can arrive while loading/unloading remains pending. Record pickup/delivery through the existing commands or authenticated carrier evidence. The 214 codec's local clock/status observations are retained in their original audit evidence; they are not automatically reinterpreted as timezone-qualified gate times.

Known gate instants must follow pickup arrival/departure, each intermediate arrival/service/departure, then final delivery arrival/departure. Missing observations remain missing; recording delivery arrival does not fabricate an origin gate record. Duplicate records require **Correct pickup/delivery arrival/departure**. A correction appends a new record linked to its predecessor, requires supporting evidence and the current load version, and revalidates the whole known sequence. Original records remain readable. Current stop summaries and corrections are scoped to the active tender attempt; released-carrier gate evidence stays historical.

## Attention, timing, and financial meaning

Current stop cards show the confirmed window, early/on-time/late arrival, minutes after the window, and dwell. Known arrival/departure pairs yield elapsed whole minutes. While the stop remains open, an arrival without departure shows elapsed time since arrival. If the physical stop has completed but departure is missing, completed dwell remains unknown instead of accumulating a fictional dwell total. Missing arrivals after a booked window enter the overview/exception filter until arrival or physical completion resolves that attention condition. Metrics are computed when the workspace is read; they do not overwrite manually recorded shipment exceptions.

These measurements are operational evidence. They do not calculate detention eligibility, add a charge, alter buy/sell rates, create invoices, or post a journal. Contract free time, charge approval, and external billing agreements need their own verified financial workflow before automation.

## API, persistence, and sync-server

All stop mutations require operations permission, `Idempotency-Key`, and `expected_load_version`. Appointment/gate changes, load version, audit, receipt, and any sync handoff commit together under the tenant row lock. Concurrent booking against one version commits exactly one result. Existing PostgreSQL snapshots deserialize missing arrays/details safely.

- `POST /v1/loads/{id}/appointments` books/reschedules.
- `POST /v1/appointments/{id}/cancel` records facility cancellation.
- `POST /v1/loads/{id}/stop-events` records arrival/departure.
- `POST /v1/stop-events/{id}/correct` appends a correction.

Workspace responses include `appointments`, `stop_events`, and each load's `stop_execution` summary. See [request shapes](api.md).

For imported orders, appointment/gate mutations enqueue the existing ordered sync-server tracking endpoint. `trackingData.freight_stops` contains each stop's current window UTC instants/timezone and current-attempt `arrival_at`/`departure_at`. Evidence references, contact details, staff identities, and internal notes are excluded. Corrections update the original source order, and carrier release snapshots clear the old attempt's times. Subsequent appointment/gate/movement updates use the current assignment; retired carrier times do not leak into a replacement assignment. Sync-server currently stores arbitrary nonempty tracking JSON, so this uses its existing contract without changing that repository.

Verified evidence includes real-codec N3/N4 address compatibility, timezone/DST cases, booking history and baseline protection, atomic failure rollback, chronological corrections, missing-departure handling, retired-carrier isolation, actual HTTP sync writeback, PostgreSQL concurrent writers/reconnect, role denial, and a production browser workflow on desktop/mobile. Live dock-system booking, carrier communication, route optimization, and detention billing remain further work toward the full TMS.
