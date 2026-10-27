# Carrier tender and release

Every offer is an independent tender attempt with a carrier snapshot, plan snapshot, authenticated creator, status, and carrier-facing shipment reference. Re-planning keeps the freight load ID/reference, source order UUID, BOL, and historical evidence. A replacement tender receives a new `SF-…` shipment reference even when the carrier is unchanged. Partners must echo the attempt's shipment reference in their 990, 214, and 210 documents.

## Withdraw an unsent offer

Use **Withdraw unsent tender** in load details to return an offer to planning. Local cancellation is also available for a planned load or a proven-unsent offer. No [partner portal publication](partner-portal.md) or non-retracted external [carrier rate-confirmation handoff](carrier-rate-confirmations.md) may exist for that tender, including prior document revisions. Every associated EDI job must still be `pending_handoff`, explicitly `not_started`, have zero current/cumulative attempts, and have no upstream response. Freight commits the withdrawal and supersedes only that attempt's jobs together.

An HTTP failure, timeout, expired lease, or reset retry cycle cannot establish that a carrier never received an offer. The dispatcher persists `transmission: attempted` before IO and retains cumulative attempts. Older rows without transmission evidence read as `unknown`; they cannot use local withdrawal. A portal publication, manually accepted assignment, or recorded external carrier rate-document handoff is a commitment even when its original HTTP handoff has not started. Issuing a rate document alone does not establish communication. A later report of an old/changed document being handed off creates a separate coordination hold; resolve that retained evidence with actual carrier findings before new commitments.

## Release a carrier before pickup

1. Open the load's tender history and select **Request carrier release**. Choose return to planning or cancel, and record a reason. Freight preserves the carrier assignment and pauses dispatch, new EDI attempts, and operator retries for that attempt.
2. Either publish the request through **Authenticated carrier release**, or contact the carrier through your actual operating channel. Establish whether it accepted the offer, has custody, and agrees to release the assignment. Publication makes the exact request available in the scoped carrier portal; it does not send an email or EDI cancellation.
3. The carrier can respond natively as described below. For genuine external correspondence, record the carrier contact, external confirmation reference, and substantive note. **Confirm carrier released** clears assignments and returns the load to planning or cancels it according to the request. **Confirm carrier retained** closes the request and resumes the existing assignment. External attestations record the authenticated operations subject and require the latest load version.
4. For re-planning, revise the plan and tender again. The old attempt, release proof, wire identity, and handoff history remain visible. Pickup/location and assignment changes continue to enqueue ordered sync-server tracking against the original source order.

Release can be requested while tendered, accepted, or dispatched, before pickup. A live HTTP handoff lease prevents release confirmation; allow it to finish or expire. Expired leases can be fenced without resending, even if the connection has since been disabled. Their previous transmission outcome remains uncertain. The old worker cannot update a replacement tender when its response arrives.

Existing carrier invoices or non-void customer invoices block release/re-tendering until their financial implications are reconciled; no invoice is silently reassigned or discarded. Current-carrier pickup evidence while a release is pending records the real movement and changes the request to `blocked_by_movement`. It creates an incident requiring investigation. Cancelling a load never substitutes for resolving physical custody.

## Staff confirmation context

The shared [shipment confirmation forms](shipment-confirmations.md) keep the shipment identity and version reviewed when opened. A background refresh cannot authorize an open tender or other action against a newly revised shipment. Changed shipments disable confirmation while retaining selections/notes; close the action and review the latest shipment before reopening it. An unseen competing update still receives the existing API version-conflict rejection.

## Authenticated carrier release

For a pending current pre-pickup request, operations publishes an explicit carrier message and a response/application deadline in UTC, no more than seven days ahead. The frozen document binds the exact tender shipment reference, carrier, lane, equipment, disposition, message, and deadline. Private operations reasons, customer identity, prices, and financial review details are excluded. Publishing or revising requires the reviewed load version and preceding publication ID; it retains the assignment and original documents. Open staff forms keep the identity/version that was reviewed even during background refresh.

The authenticated carrier sees **Carrier release requests** in `/portal`, with an exact document download. **Confirm carrier release** requires a contact, reference, substantive findings, the published document hash, current load version, and explicit confirmation that the carrier has not picked up and has no custody of this assignment. **Retain assignment** closes the request without inventing tender acceptance or physical movement. Release requires an unexpired deadline; retention remains available after expiry because it preserves ownership. Other carriers, customers, staff/service roles, superseded requests, replacement assignments, and pickup-blocked requests cannot submit that native response.

With no hold, the native response applies the existing release/retention rules atomically. A live handoff lease, financial evidence, or unresolved custody review retains a positive acknowledgement as `awaiting_operations`; the carrier assignment and pending request remain. This is not a successful shipment release. After resolving the hold, operations can **Apply recorded carrier release**. Application checks the current exact request/tender, latest publication, deadline, load version, custody, financial records, and leases again. It preserves the original carrier contact, actor, time, document hash, and evidence hash; the later operations actor/time are separate application evidence. Expired authorizations require a new publication and current carrier confirmation. A superseding publication cannot reuse the old response.

Actual pickup wins regardless of the order of competing transactions. A release that commits first leaves subsequent pickup as a retained former-carrier custody incident; pickup that commits first blocks the release. Expired HTTP leases are fenced by the existing release command, and a returning worker cannot overwrite the new assignment. Native release and retention remain usable during commerce cancellation or outage; unsafe Sync writeback stays held/local. No response creates a cancellation fee, invoice approval, journal, bank transfer, or EDI cancellation message.

## Investigate late carrier evidence

Correlation checks the exact attempt reference and its original partner/SCAC. A retired attempt's 990 is ignored for current ownership and retained in the audit. It cannot accept or decline a newer offer, including another offer to the same carrier. An ambiguous freight-reference fallback cannot identify a new attempt.

Historical 214 movement and 210 invoices create open carrier incidents. The original event remains intact; Freight does not move the replacement load or post the former carrier's invoice against its new buy agreement. Estimates alone do not create a physical-custody conflict. An incident pauses new tendering/dispatch and EDI handoffs, and prevents delivery evidence from qualifying finance approval/issuance.

In **Integrations → Inbound documents**, these signed documents show **Reconciliation required**, with a link to the load. Investigate with the carrier, then record the contact, external reference, and findings through **Resolve carrier conflict**. Resolution retains current ownership and all historical evidence. The inbox item becomes `reconciled`; its original processing result and attempt journal remain. It is not replayed as a new shipment event or invoice. If custody or liabilities need another action, record that verified action explicitly using the existing operational/finance workflows; this confirmation does not invent it.

## Legacy records

When an older assigned load first needs tender correlation or an operation, Freight snapshots its existing carrier and retains its original load-reference shipment identity. This is marked `legacy`, with creator `legacy_snapshot` and the time of adoption; it does not invent the original tender time. Missing transmission history remains uncertain. A confirmed release followed by a new tender establishes the new independent identity without rewriting old payloads.

## Gateway and commissioning boundary

The inspected `stateset-edi` `Tender204` builder emits `B2A*00` originals and exposes no cancellation purpose in that request model. Freight uses the gateway's supported original-offer endpoint and records external release evidence. Electronic cancellation/change messages, carrier-specific acknowledgment rules, transport certification, and production carrier acceptance remain commissioning work. A successful gateway response confirms generation/queuing, not delivery or carrier agreement.