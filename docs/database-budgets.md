# Runtime PostgreSQL budgets

Snapshot admission bounds queued reads. It cannot stop an admitted statement waiting indefinitely for a PostgreSQL lock. Runtime connections now have application-owned pool-acquisition, statement and lock budgets. Each newly opened connection receives its settings before use, including replacements after connection closure. Settings apply only to Freight's pool; EDI, Sync and database-wide defaults are unchanged.

| Setting                                 | Default | Accepted milliseconds |
| --------------------------------------- | ------- | --------------------- |
| `FREIGHT_DATABASE_ACQUIRE_TIMEOUT_MS`   | 5000    | 1–300000              |
| `FREIGHT_DATABASE_STATEMENT_TIMEOUT_MS` | 30000   | 1–300000              |
| `FREIGHT_DATABASE_LOCK_TIMEOUT_MS`      | 5000    | 1–300000              |

Invalid, empty, signed, whitespace-padded, fractional, zero and overflowing values prevent startup. The parser runs in memory mode too, although no database operations occur there. Pool size remains independently configured by `FREIGHT_DATABASE_MAX_CONNECTIONS`. Startup migrations retain their existing transaction-local 10-second lock and 60-second statement limits; committing startup restores the runtime session settings.

PostgreSQL enforces `statement_timeout` on individual statements, and `lock_timeout` on individual lock acquisitions. These are not whole-transaction or end-to-end request deadlines. A lock budget at or above the statement budget is superseded by the statement budget. See PostgreSQL's [timeout definitions](https://www.postgresql.org/docs/16/runtime-config-client.html#GUC-STATEMENT-TIMEOUT). SQLx's pool-acquisition budget also covers connection establishment, checks and initialization, as implemented in the locked `sqlx-core` dependency.

Pool acquisition expiry and PostgreSQL SQLSTATE `55P03`/`57014` produce:

```http
HTTP/1.1 503 Service Unavailable
Retry-After: 1
Cache-Control: no-store
Content-Type: application/json

{"error":{"code":"database_temporarily_unavailable","message":"Database is temporarily busy. Retry the original request shortly."}}
```

Database I/O loss, a closed pool and explicit PostgreSQL connection/shutdown conditions now return the same 503 code and headers, with the message `Database is temporarily unavailable. Check the original request before retrying.` These conditions emit `freight_database_unavailable` with `reason=connection_unavailable`; they are distinct from known budget exhaustion. Protocol violations, decode and integrity errors retain their ordinary storage-error behavior. The explicit connection/shutdown list follows PostgreSQL's [error definitions](https://www.postgresql.org/docs/16/errcodes-appendix.html); `08P01` protocol violation is excluded. No SQL details, credentials or original payloads are included in the new warning or HTTP error.

Staff and partner credential verification preserve the unavailable code/retry hint, issue no session on failure, and retain a temporary-unavailability message distinct from credential rejection. Gateways make no automatic retry. An unavailable response cannot determine whether a disconnected mutation already committed; browser saves retain an unknown outcome for HTTP 5xx and use the original key/body and receipt-aware recovery.

An admitted snapshot retains its permit after its caller disconnects. A server-side SQL timeout can end that abandoned database work; only then does the job return its slot. SQLx rolls back failed transactions before pool reuse. This preserves ownership of actual work rather than freeing a permit while its query continues. Pool exhaustion also expires finitely, including readiness checks, and later requests recover when capacity returns.

Mutation retries must retain their original idempotency key and exact request. A known busy response is not a universal certificate that nothing committed: statement/commit acknowledgement boundaries and external handoffs require receipt-aware recovery. The API does not automatically resubmit writes, fabricate a successful handoff or compensate a possible commit. The interruption regressions deliberately stall the workspace UPDATE before commit so their rollback assertions have an unambiguous recovery point.

The owned PostgreSQL tests exercise real loopback HTTP, actual table/advisory locks and a server-side sleeping statement. They exhaust the actual pool, check settings on multiple original and replacement connections, observe an admitted SQL read before cancelling its caller, retain the table lock while waiting for capacity to return, and verify readiness/retry recovery. A second scenario stalls uploads after original file INSERT, once on a lock and once in a statement. A nontransactional test sequence witnesses each INSERT independently of rollback. Complete JSON, `updated_at`, `xmin`, `ctid`, file counts and absent receipts must remain unchanged after interruption. The exact original request then commits once, replays without another INSERT, retains original download bytes, and survives reconnect. These test functions, triggers and sequences exist only in disposable schemas.

The native carrier browser retains its selected POD and entered evidence across the database busy response, then completes submission and finance review. Its tracking form continues covering read-queue overload separately. Handler unit tests cover both staff and partner verification, retry hints, no premature cookie and no automatic retry.

For the subsequent connector response release, first apply the [integration-response reversal](integration-response-budgets.md) in an isolated copy, then follow this page’s existing reconstruction. The recorded reports and patch hashes remain historical.

For the subsequent tenant-worker release, first apply the [tenant-worker reversal](tenant-integration-workers.md) in an isolated copy, then follow this page’s existing reconstruction. Recorded report and original patch bytes remain unchanged.

For the subsequent process-drain release, first apply the [process-drain reversal](process-drain.md) in an isolated copy, then follow this page’s existing reconstruction. Historical reports and original patch bytes remain unchanged.

For the subsequent bounded-dispatch release, first apply the [bounded-dispatch reversal](bounded-dispatch-turns.md) in an isolated copy, then follow this page’s existing reconstruction. Historical reports and original patch bytes remain unchanged.
