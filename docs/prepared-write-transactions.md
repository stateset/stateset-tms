# Transaction-owned PostgreSQL write preparation

PostgreSQL mutation preparation now runs on a blocking worker that owns the actual transaction, source/format locks and pool connection. It reconciles operational issues, selects financial evidence, serializes the complete workspace directly to JSON, builds the SQLx JSONB arguments, and drops the full workspace before returning. This removes the temporary complete JSON `Value` tree and moves those CPU costs off the asynchronous runtime threads. Business format remains **18**, with **25 unchanged migrations**.

This extends [single-transaction shared saves](conditional-save-transactions.md). Exact originals still bypass the tenant row lock; misses still lock and decode the complete current source and recheck request identity, stopped saves, versions and permissions. Application stays in the caller's future. Memory-mode preparation and the separate document-authorization path retain their preceding implementation.

## Atomicity and cancellation

The worker never inserts bytes, updates source or commits independently. If a caller disappears while preparation is queued or running, that job retains its transaction until actual CPU work ends. Its abandoned result drops and rolls back. The transaction's pool connection bounds admitted work; snapshot read capacity is not required. A cancellation after commit begins still requires the existing exact-key recovery protocol because acknowledgement can be lost.

Financial evidence selection copies only document ID, size and SHA-256. The caller still locks and validates actual supporting bytes before inserting a new blob and updating the workspace in the same transaction. Preparation occurs before that byte verification; it cannot authorize a write. A refused preparation or failed verification rolls back source, receipts and file changes together.

`write_json::encode` streams the entire typed stored model through serde's `RawValue`. SQLx builds the original JSONB wire parameter on the worker. The SQL remains `UPDATE freight_workspaces SET data=$2,updated_at=now() WHERE tenant_id=$1`; there is no text cast or change to JSONB storage. Unicode, large integers, original receipts and quarantined unsupported operations evidence are compared with the preceding complete encoding and round-tripped through the streaming decoder.

## Qualification

Browser scenarios use production Next.js assets with the harness's in-memory API. PostgreSQL persistence and the changed preparation path are exercised separately by the PostgreSQL tests and actual-service recovery drill.

A real PostgreSQL regression occupies the sole blocking worker, applies a staged compound load/file mutation, then cancels its caller while preparation is queued. A format-change probe must remain blocked until the worker really finishes. The test then verifies unchanged physical source, no original blob, one successful exact-key retry, an unchanged exact replay, and exact downloaded bytes without an orphan candidate. It exercises the transaction primitive and actual domain mutation; it is not a claim about a new public compound endpoint. The same added test fails against the isolated preceding production implementation. The first full check accidentally reused that baseline lib-test executable from the shared target directory, producing the old test inventory and the expected baseline PostgreSQL failure. Its logs and executable remain retained as unqualified; the final check invalidates that generated executable, rebuilds and asserts the complete current inventory. Future counterfactual runs must use a separate target directory. Another PostgreSQL test verifies the actual SQLx argument is JSONB and equals the preceding stored representation.

## Resource observations and tradeoffs

| Run | Shipments, history depth 4 | New median ms | Replay median ms | New-save sampled API peak MiB | Whole-run PostgreSQL cgroup peak MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| before | 100 | 136.2 | 7.2 | 63.1 | 555.3 |
| before | 1,000 | 1570.8 | 51.3 | 367.3 | 555.3 |
| after | 100 | 140.3 | 13.2 | 49.0 | 723.2 |
| after | 1,000 | 1546.8 | 53.4 | 276.1 | 723.2 |
| after-confirmation | 100 | 112.2 | 7.0 | 38.3 | 547.9 |
| after-confirmation | 1,000 | 1535.7 | 47.5 | 278.8 | 547.9 |
| before-confirmation | 100 | 178.5 | 11.8 | 55.6 | 679.5 |
| before-confirmation | 1,000 | 1840.4 | 49.0 | 464.7 | 679.5 |

The PostgreSQL figure is each entire run's high water, repeated for its two workload rows; it is not a per-request PostgreSQL allocation. API memory is sampled during requests. Preserve the higher PostgreSQL observation and slower replays alongside favorable API observations. The runs do not isolate a universal causal memory bound, eliminate tail latency, or qualify the deployment's resource budget. Every archived original response, exact replay and source invariant is rehashed and parsed; only the two live tracking timestamps are validated and removed for cross-run response comparison. Stored originals and own exact replays are never normalized.