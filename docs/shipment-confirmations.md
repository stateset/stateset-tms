# Review-bound shipment confirmations

Staff shipment confirmations now retain the shipment identity and version from when the action opened. A later workspace refresh cannot silently substitute a newer version into the request. This applies to the shared tender, accept/decline, dispatch, pickup, location, delivery/POD, exception and cancellation confirmation forms.

The defect was demonstrated against the preceding production UI and the real Freight API. An operator opened a tender on version 1, then an independent HTTP request revised the shipment's reference, buy agreement and receiving dock to version 2. After the normal background refresh, the old form sent `expected_version: 2`. The API accepted it, changed the shipment to tendered and created an outbound EDI job. The backend's existing optimistic concurrency check worked as designed; the browser had supplied a version the operator had never reviewed in that form.

## Operator behavior

A confirmation keeps the original shipment identity, version, reference and route/equipment context. When a refreshed workspace shows that shipment has changed or is unavailable, an inline alert explains that the action must be closed and the latest shipment reviewed. Confirmation is disabled; carrier selections and typed notes remain in the open form. Closing and reopening an action after reviewing the updated shipment starts a new decision. Refreshes that change only another shipment leave a valid confirmation usable.

Submission also checks this boundary, and the request always sends the originally reviewed version. If another writer wins before the browser sees its update, the existing API rejects the stale command with HTTP 409. The browser then refreshes the source without closing the form, keeps the unsaved input and error, and blocks confirmation once the changed shipment arrives. A failed refresh retains the conflict and the original version; it does not silently upgrade or retry the command against newer facts.

This is a shipment-version boundary, not a frozen copy of every master-data/document/partner record. Backend authorization, current qualification, source holds, document integrity, financial locks and idempotency remain authoritative. Normal physical event recording, EDI/Sync delivery and database formats are unchanged.

The subsequent [interrupted-save recovery](save-recovery.md) preserves the exact original command for receipt replay when its response is lost. A background version change does not authorize a new command or prevent checking that original receipt. A confirmed save with an unsuccessful workspace refresh retries only the read.

## Evidence

| Scenario | Required evidence |
| --- | --- |
| A tender form remains open across a concurrent rate/dock revision and background refresh | No command sent; revised raw shipment retained; carrier selection preserved; explicit close/review/reopen then creates one tender using the revised version and agreement |
| An unseen concurrent exception wins before form submission | Original version sent, real HTTP 409, current source refreshed, independently recorded report unchanged, unsaved note retained and stale confirmation disabled |
| Only another shipment changes during refresh | Reviewed shipment's form remains usable, note survives, exactly one command uses its reviewed version, independent shipment remains unchanged |

To reproduce the browser qualification after building the API and installing web dependencies with Node.js 22:

```bash
npm --prefix web run build
FREIGHT_E2E_PRODUCTION=true npm --prefix web run test:e2e -- tests/command-review.spec.js
```