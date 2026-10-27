# Independent operator acceptance

Use this packet with dispatchers and finance reviewers who did not implement the workflows. Record observed outcomes against one frozen source/build. Automated fixtures and a maintainer's demonstration do not count as independent acceptance.

Use an owned disposable workspace with synthetic customers, carriers, documents and amounts. Do not send live tenders, callbacks, invoices or payments. Assign separate operations, finance, viewer and partner identities. Use the web app for financial writes; the terminal's Billing, Payables and Queues views are read-only.

Before the trial, record the source commit, API/TUI executable hashes, web build identity, database version, terminal/browser versions and configured roles. Declare task time budgets, acceptable assistance and recovery objectives before observing results. Keep participant identities and recordings in the private review workspace; use anonymous participant IDs in any published aggregate.

| Task | Operator work | Required outcome |
| --- | --- | --- |
| Find and inspect | In Ghostty, switch all five workspaces, search a shipment, move across pages, inspect a linked record and return | Correct record and selection; clear empty/loading/error states; no accidental write |
| Plan and tender | Create a synthetic load in the web app, review the procurement, tender and accept using the appropriate identities | Agreed source/rate revisions remain identifiable; wrong-role and stale replies are refused |
| Review a conflict | Have a second operator change the source while the first operator has a confirmation open | Original draft retained, conflict explained, fresh review required before another command |
| Coordinate work | In the terminal, claim an issue, record findings, change follow-up, defer, resume and release | Correct owner/history/deadlines; source evidence, shipment and financial truth unchanged |
| Execute and deliver | Dispatch, record tracking, supply original POD bytes and complete the configured review | Chronology and evidence match the intended shipment; corrections retain earlier facts |
| Reconcile finances | Finance reviews a carrier invoice, prepares a settlement, issues customer billing and records a synthetic correction | Exact cents, audit decisions and immutable originals; correction is distinguishable from original posting |
| Recover an uncertain save | Interrupt the owned test connection during confirmation, reconnect and inspect/check the retained original | One logical application, original request identity retained, known completed/stopped outcome; no new command silently replaces it |
| Recover access | Sign out or switch organizations while a read or save is delayed | Old workspace data is cleared; old responses cannot enter the new identity; recovery requires the original authorized scope |
| Use assistive input | Complete representative navigation, search and review using keyboard and a named screen reader; repeat a narrow viewport | Discoverable controls, readable labels, usable focus, background isolation and focus restoration; no hidden confirmation |
| Verify a restore | In a separately restored test environment, locate a prior shipment, original document and command receipt | Exact business facts and bytes remain available; pending work and replay outcomes are understandable |

For each attempt, record participant ID/role, task, start/end time, completion, mistakes, assistance, interruptions and recovery time. Attach a redacted trace or an observer's concrete account of any failure. Retain unsuccessful first attempts separately from repeats after a fix.

Acceptance requires every critical business/security/recovery invariant above, the predeclared task and recovery budgets, and resolution or an explicit owner decision for each observed usability problem. Do not mark the packet accepted merely because a test suite passes or a participant completes only the easiest tasks. The release owner records the exact accepted build and the participating independent reviewers.

Use [access configuration](access.md), [terminal controls and recovery](terminal.md), [financial workflows](finance.md) and [save recovery](save-recovery.md) for setup. The ten platform acceptance requirements are tracked in the private development repository's implementation workboard; publishing this packet does not attest that a trial occurred.
