# Reviewing billing identity changes

Billing identity controls the issuing legal name, address, billing contact and payment instructions captured by newly issued customer invoices. An open editor now retains the identity and version the operator actually reviewed, together with their verified tenant/operator scope.

## Concurrent editing

A background update to the same billing identity displays a warning and preserves all four unsaved fields. A new stale edit sends no write. Close the form and reopen **Edit billing identity** to review the current legal and payment details before making another change. Reopening loads the current values and version; it does not silently merge or authorize the older edits.

If another operator's change has not reached this browser, submission still carries the original reviewed version. The server rejects it with HTTP 409 under its existing transaction lock. The editor requests a fresh workspace without replacing the open form's values or reviewed version. A failed refresh keeps the conflict and original entries available. Unrelated workspace updates leave the editor usable.

An unchanged retry of an already attempted save retains its original body, reviewed version and idempotency key. If its response was lost after commit, the server can return that original receipt even after another operator has changed the identity again. That replay cannot overwrite the newer identity. Edited retry values do not bypass a known newer version.

The profile POST also carries the verified tenant/operator that opened the form. The gateway checks those values against independently authenticated access and uses that same captured upstream credential to forward the operation. A switched tenant or operator credential is refused before mutation. This uses the existing gateway contract; the client-supplied identity never grants access.

## Qualification

- First-time identity creation racing with another creator.
- A refreshed stale editor, preserved inputs and explicit reopening before a fresh save.
- An unseen server conflict followed by a refresh.
- An unrelated workspace change.
- A lost acknowledgement, a blocked edited retry and an exact original receipt after a newer update.
- Repeated conflicts while workspace reads fail.
- A switched tenant credential.
- Another operator's credential in the same tenant.

## Scope and operating limits

Only the billing identity editor gains this reviewed-version and identity boundary. Other finance workflows retain their existing contracts. Issued invoice snapshots, postings, allocations and actual cash history are unchanged. This editor does not verify bank instructions with a bank or send a payment.

The editor's existing pending request key remains local to its mounted component. An unchanged retry within that editor is qualified; reload, navigation, tab closure and cross-device restoration of this embedded workflow are not newly implemented. The shared workspace's [encrypted journal](sealed-save-journal.md) and server resolution cover their listed save paths; `finance/profile` is not one of those paths. Retain an uncertain form and check its unchanged original before starting another change. Production identity commissioning, measured operator outcomes and other browser engines remain separate evidence gates.
