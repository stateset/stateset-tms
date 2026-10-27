# Snapshot read overload protection

Snapshot admission previously bounded active work but retained an unlimited number of waiters for an unlimited time. A burst could leave operators waiting behind expensive snapshots, including after a browser's useful waiting time had passed. The current scheduler bounds waiting work before SQL, decoding or projection starts, while retaining tenant order and the existing ownership of admitted jobs.

| API setting | Default | Accepted range |
| --- | --- | --- |
| `FREIGHT_READ_CONCURRENCY` | 2 active snapshots | 1–64 |
| `FREIGHT_READ_MAX_PENDING` | 64 waiting snapshots | 1–4096 |
| `FREIGHT_READ_TENANT_MAX_PENDING` | 16 waiting snapshots per tenant | 1–global pending limit |
| `FREIGHT_READ_QUEUE_TIMEOUT_MS` | 5000 ms | 1–60000 |

These are per-process limits shared by `Store::read` and projected workspace/control-tower reads, including internal callers and source preflights that use those methods. Active jobs do not consume pending capacity. If the tenant limit is absent, its default is capped at the global limit. Explicit incompatible values, empty strings, nonintegers, zero and out-of-range settings prevent startup with the setting's name. Compose and the example environment declare these settings; size all replicas and the separate database pool together.

## Admission and cancellation

Only a tenant's head waiter joins the global semaphore queue. Other requests wait in their tenant's FIFO queue. An idle process admits immediately without retaining tenant registry state; a newly arriving request cannot bypass existing queued work. The global limit also bounds the number of live tenant queue entries. Rejected identities are not registered.

A single deadline covers both the local tenant queue and the global semaphore queue. Pending reservations are returned on cancellation, timeout, successful admission or an admission error. A tenant entry disappears when its final waiter leaves. A timed-out or rejected request starts no SQL/decode/projection work. Tokio's timer is observed when the runtime schedules the task; this is an admission budget, not a precise wall-clock service guarantee.

Once admitted, a job continues owning its active slot through the actual snapshot/decode/projection work even if its caller disconnects. Queue timeouts cannot free another job's slot. This preserves the existing cancellation and snapshot boundaries instead of allowing abandoned work to overcommit the process.

## API, sign-in and operator behavior

An exhausted queue or elapsed admission deadline returns:

```http
HTTP/1.1 503 Service Unavailable
Retry-After: 1
Cache-Control: no-store
Content-Type: application/json

{"error":{"code":"read_capacity_exceeded","message":"Workspace is busy. Please retry shortly."}}
```

The authenticated Next.js gateway preserves the status, body and retry hint. Staff and partner credential verification distinguish temporary service failure from an invalid credential. They issue no cookie before successful verification and do not erase an existing session on a busy response. A generic upstream server error receives a temporary-unavailability message without exposing internal details. Clerk identity and organization checks retain their separate authentication rules.

The existing control tower retains its last successful list and open follow-up text on a failed automatic refresh. It displays the busy message without signing the operator out or silently submitting the form. Gateways and sign-in verification make one upstream call; they do not automatically repeat writes. A later explicit retry must retain the original idempotency key where required. Successful business mutation and receipt/source/version checks remain under the tenant transaction.

The partner portal previously removed its shipment list and open evidence form on every refresh error. Temporary errors now retain the last successful list, typed tracking evidence and selected original POD file while displaying the error. A newer refresh supersedes older responses, preventing a delayed successful response from restoring private data after a newer denial. HTTP 401/403 still clears the list, modal, pending action and messages. Sign-out invalidates in-flight refreshes and suspends automatic reads until explicit sign-in succeeds. Clerk session/organization changes retain their keyed component reset. Retained forms keep their original offer/version; refreshing never silently rebinds an unfinished carrier assertion to another shipment or newer agreement.

The new HTTP test exposed an unnecessary API preflight: every load command fetched a snapshot even though only tender and dispatch can require a canonical source observation. Physical tracking commands now enter their existing guarded transaction directly, preserving actual observations during read saturation. Tender and dispatch still use the shared source predicate, canonical observations and transactional source grants. Other commitment workflows keep their existing source checks. A read-capacity response during commitment preflight consumes no mutation receipt or shipment version.

## Verification and remaining work

Four added scheduler tests cover strict configuration, tenant/global limits under 1,000 rejected identities, both queue deadlines, and deadline/permit-release races. Existing tenant-order, cancellation and idle-slot tests remain. Two added store scenarios exercise real loopback HTTP: memory and disposable PostgreSQL. They hold a real blocking projection, fill both pending slots, reject local/global overflow before running its projection, observe finite queued responses and responsive health, cancel the original caller without releasing its active slot, commit a physical observation, reject a commitment preflight, release the held job and read the current version. The PostgreSQL scenario additionally holds an exclusive workspace-table lock during overload; busy responses complete while SQL cannot read that table. Reconnect retains the actual write and exactly its receipt; rejected/expired reads create no tenant.

Two Node tests exercise the actual gateway and both credential-verification handlers with upstream responses. The identical tests fail against the preceding handlers: the gateway loses `Retry-After` and sign-in loses the busy code. The current tests require original status/body/retry headers, no automatic retry, no session issuance on failure, credential rejection remaining distinct, and session issuance only on successful verification. The production control-tower browser adds a busy automatic refresh while a follow-up form is open and requires retained rows, text and session.

The subsequent [staff session lifecycle](staff-session-lifecycle.md) extends staff response/session fencing. Its separate current browser evidence leaves this overload-release source/build record intact.
