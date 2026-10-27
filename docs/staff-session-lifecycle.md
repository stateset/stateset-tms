# Staff workspace session lifecycle

The shared [shipment confirmation forms](shipment-confirmations.md) now also bind their open action to the originally reviewed shipment identity/version. This protects the decision across ordinary refreshes within a valid session; the session boundaries below continue to handle access loss and sign-out.

[Interrupted shared saves](save-recovery.md) retain an original request only within this page instance. Access closure clears its payload/key and recovery notice, and the existing epoch fence discards a delayed successful receipt after sign-out.

Staff refreshes previously applied in response-arrival order. A delayed successful workspace response could repopulate the screen after a newer authentication denial. Sign-out also waited for the session endpoint before clearing private state, and an in-flight Sync order listing could open its dialog after a later sign-in.

The workspace now has two boundaries. A refresh sequence admits only the latest workspace response. A session epoch invalidates requests begun before access denial, explicit sign-out or component cleanup. The existing Clerk provider remounts its children when the session or organization changes; this boundary remains in place.

Workspace refresh access denial and explicit sign-out immediately clear the workspace, selected shipment, dialogs, imported orders, quote selection, POD choice, pending mutation key, transient messages and navigation/search state. Automatic workspace reads remain suspended until successful explicit credential verification or an explicit Clerk access retry. A late successful response cannot reopen a suspended workspace. Transient refresh errors still retain the last successful workspace and unfinished forms, as qualified by [snapshot overload protection](read-overload.md).

The same session epoch guards the staff page's request function used by its mutations and Sync order listings. A result from an earlier session is discarded before these callers can populate a new session. Page mutation completion and source-list error/finally handlers cannot clear a later session's busy state, open its dialog or display a stale success message. Embedded workflows retain their own request lifecycles and unmount when workspace access clears; this change does not claim to cancel every descendant request. Invalidating browser state does not cancel or undo a server operation that has already committed; normal transaction, version and idempotency contracts remain authoritative.

Explicit sign-out closes the local workspace before contacting the server or identity provider. While that request is pending, sign-in stays disabled and periodic reads stay suspended. If cookie removal fails, the screen explains that server sign-out could not be confirmed and offers **Retry sign out**. New credential sign-in remains disabled until sign-out succeeds, preventing a delayed cookie-deletion response from clearing a newer session. Clerk sign-out retains the configured provider redirect.

The browser tests use the actual production Next.js gateway and Rust API for credential verification, session cookies and successful logout. They control selected browser requests to impose response ordering and an unsuccessful logout. One scenario denies a newer automatic refresh while a form is open, releases an older success, verifies private state stays cleared and polling stops, then signs in explicitly. The other holds a Sync order listing and cookie deletion, verifies immediate local closure and suspended polling, injects a failed deletion, retries against the actual gateway, checks the revoked cookie returns 401, opens a new session and releases the old private order listing. It must not open a dialog or trigger a stale refresh.

To exercise the production build against the owned credential gateway and synthetic Sync fixture:

```bash
npm --prefix web run build
FREIGHT_E2E_PRODUCTION=true npm --prefix web run test:e2e -- session-lifecycle.spec.js --trace on
FREIGHT_E2E_PRODUCTION=true npm --prefix web run test:e2e
```

The subsequent [runtime database-budget release](database-budgets.md) changes backend timeout handling and the two verification handlers. It has fresh full-suite and actual-service evidence; this session release retains its original source/build qualification record.
