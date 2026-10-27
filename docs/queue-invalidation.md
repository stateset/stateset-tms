# Queue scheduling invalidation

Migration 031 separates conservative block invalidation from exact scheduling projection. Business storage format remains 21. The migration replaces a function and triggers transactionally; it does not rewrite original JSON, blocks, receipts, business versions or workspace timestamps.

Canonical writes can change many blocks before writing the final header. Previously, each block invalidated the three queue hints by updating the workspace row, and that update reconstructed both queues again. This made a large queue's insertion repeat increasingly expensive reconstruction work.

The hint-only trigger now accepts the existing all-`-infinity` invalidation without reconstruction. This requests immediate worker consideration, and the worker still reads and checks the authoritative queue under the tenant lock. Other hint edits are recomputed, so setting a deadline to null or a later time cannot hide work. The separate header trigger always recomputes exact inbox, EDI and Sync/native-commerce deadlines, including `UPDATE data=data` after same-cardinality block edits. Mixed data/hint updates run conservative invalidation before the final header projection.

The native writer always persists the final header after its block changes. Raw block edits without a final header leave conservative due hints; they do not establish a new exact scheduler projection. Missing or malformed canonical data still requests a full native check rather than hiding work.

## Verify behavior

Use an owned disposable database:

```sh
TEST_DATABASE_URL=... cargo test --locked --test queue_invalidation -- --ignored --nocapture
TEST_DATABASE_URL=... cargo run --locked --example queue_write_profile
```

The regression traces actual scheduler function calls, compares predecessor/current trigger behavior, proves zero queue reconstructions for block-only invalidation, attempts to hide work with a hint edit, verifies a no-op header restores exact hints, and compares original queue bytes. Migration preservation compares physical original records before and after DDL.

The diagnostic profile creates a new schema, seeds fresh tenants with 123, 1,000 and 5,000 records in each queue direction, verifies retained queue data and scheduling hints, and reports PostgreSQL function timings. It uses the unchanged 30-second statement budget. Its role needs permission to create the isolated schema and enable `track_functions` for its own connections. Successful runs drop their schema; failed runs retain it for inspection. The application database should never be used as the diagnostic target.

The profile measures canonical ingestion, including its triggers. It does not measure sustained HTTP arrivals, full financial histories, deployment resource limits, cross-tenant fairness, cold console rebuilds or production latency objectives. Those acceptance requirements remain separate.
