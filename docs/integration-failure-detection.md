# First integration failure becomes owned work

The control tower now opens recovery work after the first retained failed EDI or Sync attempt. Previously, a new issue appeared only when the integration job reached `held`, after its retry allowance was exhausted. The same issue now follows the original job through backoff and retry, so an operator can take responsibility while automatic recovery continues.

## Detection contract

- Outbound jobs in `pending_handoff`, `sending`, or `retry` are actionable when they retain an error or already have an active recovery case. This applies to EDI and Sync destinations.
- Inbound documents use the same rule for `pending`, `processing`, and `retry`.
- Held jobs remain actionable even without a retained error. Fresh pending jobs without an error or existing case do not produce guessed failures.
- Clearing an error to request another attempt preserves the case ID, owner, follow-up deadline, first detection time, and current cycle. Retry alone does not resolve the issue.
- Leaving recovery clears this condition while retaining its history. A later failure reopens the same subject as a new, unassigned cycle.

Detection records when Freight actually observes the condition; it does not backdate upgraded workspaces. The normal source transaction and periodic reconciliation use the same predicate. Existing workspaces with an errored in-flight job can therefore acquire an issue without waiting for another failed attempt.

Queue resolution has a narrow meaning. Gateway acceptance does not confirm carrier custody; a document routed to finance review still needs that review. Shipment execution, customer decisions, retry limits, original notifications, handoff payloads, and financial authorization keep their existing rules. No schema or dependency change is required.

## Evidence

The current source passes **355 Rust tests**, including all **71 PostgreSQL 16 tests**, with zero failures or skips, strict Clippy, and formatting. Both control-tower production-browser scenarios and all 15 actual-process health/monitoring checks pass. The 348 protected prior evidence, migration, and dependency files remain unchanged.

The state matrix covers EDI outbound, Sync outbound, and EDI inbound recovery and terminal states, with and without errors. It verifies original business data preservation, ownership through retries, source-driven resolution, and recurrence. PostgreSQL fixtures exercise public Store reconciliation and named ownership across independent connections; they seed source states rather than pretending to cause an upstream failure. The HTTP tests independently cause real loopback failures and recovery.

The new production-browser scenario submits an authenticated callback to the API, receives a real missing-document response from the owned EDI fixture, and verifies attempt-one detection from retained case history. It claims the issue through the control tower and verifies ownership, deadline, original notification, and history after manual retry. The existing broader control-tower browser scenario is also rerun. The unchanged production Next.js build is reused only after checking its retained source and artifact hashes. An initial browser launch failed because the default executable was absent; the passing run records its explicit local Chromium executable and version.

The first drill completed 43 functional checks but failed its final source-integrity guard because the live EDI checkout changed during execution. That failed report is retained and is not counted as a passing qualification. The confirmation uses fixed source snapshots and a separate build directory, with additional build inputs recorded in the source manifest. Compatibility evidence applies to those exact recorded revisions; subsequent sibling edits require qualification again.

## Remaining A+ gates

This closes a specific operational visibility gap. A production A+ assessment still requires representative sustained load and replica testing, live identity and partner commissioning, public callback acceptance, off-host recovery, and independently measured operator and finance outcomes. This change has not been deployed, and the two browser scenarios do not substitute for the full browser suite or a real operator pilot.
