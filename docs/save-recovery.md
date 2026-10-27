# Interrupted shared-workspace saves

The subsequent [original-save resolver](save-resolution.md) adds a durable server decision: retain a completed save or stop an unapplied original before a delayed copy can run. It also recognizes a stopped original receipt during a check. The subsequent [browser journal](browser-save-journal.md) restores original input/key through reloads and reopened tabs. The memory-only lifetime described below is this earlier release’s historical scope.

The shared workspace save handler now distinguishes an interrupted response from a failed save, and a confirmed save from a successful follow-up refresh. It covers the root workspace's shipment creation/revision/commands, master-data forms, manually received/corrected carrier invoices, inline invoice actions and integration queue retry actions. Embedded quote, finance, document, partner and control-tower components retain their independent request lifecycles.

## Why the distinction matters

The preceding production UI allowed a real duplicate in the owned reproduction. The API committed a customer creation, but the browser response was interrupted after that commit. The operator edited the still-open form and clicked save again. The UI generated a different request key and the API correctly treated it as a second creation. Two customer records resulted. A second fixture interrupted the request before it reached the server; the edited retry then silently changed the original intended body.

A separate tender fixture exposed a recovery gap: after a tender committed but its response was lost, background refresh showed the new shipment version. The legitimate stale-confirmation guard then blocked the original form, although its original idempotency key could safely retrieve the committed receipt. Rebuilding a command from the newer version would be another decision, not a check of the original save.

## Operator behavior

| Observed result | What the workspace shows | Recovery operation |
| --- | --- | --- |
| A successful response is decoded, then the workspace refresh fails or is superseded | **Save completed**, with the original success message and a notice that displayed data needs refreshing | **Refresh workspace** reads current data; it does not submit the save again |
| The response is missing/unreadable, or a server/proxy failure leaves the result uncertain | **Save outcome not confirmed**; original form input is held | **Check original save** submits the exact original path/body/key and obtains the existing receipt or completes that same request |
| An initial request receives an ordinary validation/conflict rejection | Existing validation/conflict error; input remains editable | Correct the form or review current source, as required by that workflow |

A background workspace refresh cannot by itself prove which request committed. It therefore cannot clear an uncertain save. A rejected check also cannot erase uncertainty about the earlier request; the original attempt remains available for another check. The API's `replayed` flag can change while the original saved `data` remains identical.

While a shared save needs recovery, new saves through this handler are blocked. The original form is disabled, but its header close/Escape action can close the presentation; the recovery notice remains in the workspace with its original request. A synchronous in-flight guard prevents overlapping submissions or checks. After a decoded successful acknowledgement, the form closes and only a fresh accepted workspace response clears the completed-save notice. Unrelated embedded components retain their existing server-side version, authorization and idempotency protections.

Sign-out/access closure clears the held request, body, key and recovery notice immediately. Existing session/refresh generations discard late responses, so an old check cannot restore a previous session's data or notice. Recovery metadata is kept in this page instance's memory; it is not a durable browser outbox and does not survive reload/navigation/tab closure or sign-out. An original save may already have committed when the page is left. Backend receipts remain part of the existing durable workspace storage. The subsequent resolver implements safe server-side stopping; browser recovery restoration remains further work.

## Qualification

Seven actual-API browser scenarios cover:

- Interruption after customer creation commits, followed by a background refresh and exact original replay, retaining one customer.
- Interruption before the request reaches the API, followed by the same original request completing once.
- A decoded successful save followed by repeated HTTP 503 workspace refreshes, then read-only recovery with one original POST.
- A committed tender with an interrupted response, background version change, exact reviewed-version replay and one stable EDI job.
- A real HTTP 422 validation rejection, editable correction and a new key for the corrected body.
- Sign-out while a successful original receipt is held in flight, immediate private-state clearing, a discarded late response and a clean later session.
- An unreadable response after a real commit, a denied subsequent check, retained original input/key/body, and eventual original receipt recovery without another customer.

To reproduce with Node.js 22, web dependencies and the API executable available:

```bash
npm --prefix web run build
FREIGHT_E2E_PRODUCTION=true npm --prefix web run test:e2e -- tests/save-recovery.spec.js
```