# Storage compatibility and deployment

New PostgreSQL mutations acquire a local tenant turn before acquiring a database connection. The default limits are 64 active-plus-queued new-write requests per store and 16 per tenant, configured by `FREIGHT_WRITE_MAX_IN_FLIGHT` and `FREIGHT_WRITE_TENANT_MAX_IN_FLIGHT`. Global values must be integers from 1–4096; the tenant value is 1–global and defaults to the smaller of 16 or global. Invalid configuration prevents startup, including memory mode. This admission boundary applies to the common PostgreSQL mutation path; memory mode keeps its existing owned global write guard.

A write-admission deadline starts before queuing. Queue wait and subsequent pool acquisition share the existing `FREIGHT_DATABASE_ACQUIRE_TIMEOUT_MS` budget without resetting it. Uncontended shared commands retain their one-transaction receipt/mutation path. Contended shared commands first perform a protected selected-original lookup, independent of snapshot admission; a hit returns its exact verified original without joining the write queue, even when full. That lookup retains its existing SQL budgets. A miss waits only for the original remaining admission budget, then still rechecks the latest complete source and competing original under the actual PostgreSQL lock. Custom file/policy closures keep their guarded mutation path.

The owned transaction carries the actual write turn through decode, application, persistence and rollback. Dropping a caller or blocking result starts owned rollback while retaining admission until that rollback finishes. Commit/rollback tasks also retain the turn through their actual acknowledgement. Cancellation after commit begins can still leave an unknown outcome; resolve the original key/body. Queue rejection/expiry returns 503 `write_capacity_exceeded`, `Retry-After: 1` and `Cache-Control: no-store`, with no automatic operation retry. This response cannot rule out another instance having applied that original.

PostgreSQL mutation application now runs in the same owned blocking job as reconciliation, financial document metadata selection and canonical preparation. The job owns the actual transaction, source/format locks, baseline and candidate; its worker never inserts source/blob data or commits independently. Unchanged or declined outcomes roll back in the async caller. Cancelling that caller leaves its real work and locks owned until the worker finishes; its abandoned result rolls back. Cancellation after persistence or commit begins can still leave an unknown write outcome and requires original receipt recovery. The async caller still validates original document bytes and performs atomic blob/source/receipt persistence.

Each tenant has one row in `freight_workspaces`. Its `data` contains the codec version, complete root manifest and scalar descriptors. Collection originals live exclusively in `freight_workspace_blocks`; the header is not a recoverable business workspace on its own. Native decoding validates the complete frame, original membership and block partitions before returning business data. No second writable aggregate is retained.

`freight_block_counts` contains derived membership metadata for selected reads. Catalog, history, tender deadlines and queue wakeups are also derived. Missing declared originals must cause refusal, not a successful smaller collection. Absent optional inbox, appointment and gate collections retain their historical empty defaults; explicit nulls, malformed collections and missing declared blocks do not. `freight_document_content` retains original file bytes in the same database.

## Compatibility and transactions

Schema application requires the checksummed migration ledger to be an exact prefix of the release; verification-only startup requires the complete exact ledger. Changed checksums, gaps and future migrations are refused. Already applied transformations are skipped, including their table locks. Migrations 001–027 remain byte-identical. Historical migrations 024/025 retain format 18, native migration 026 produces format 19, migration 027 produces format 20, and migration 028 establishes format 21. An older executable refuses startup against the new ledger.

Migration 026 acquires the format fence before tenant/table locks. One transaction owns DDL, native lossless backfill, guards, counters and ledger advancement. It preserves every original business root, including unsupported data, original timestamps, portable history revisions and file bytes; only the business-format marker changes. A tenant that refuses backfill rolls back earlier tenants, new tables and the ledger together. An unsupported business root remains unreadable by the API after migration rather than being discarded.

Migration 027 holds the exclusive format row before the workspace table lock. Native validation checks every preceding format-19 frame, including unknown opaque business extensions, complete membership, ordinals, keys, partitioning and numeric budgets. Only after all frames validate does the same transaction update the outer/inner compatibility markers, registry, derived history format and ledger, and replace the format guards. Original block payloads and physical tuples, files, timestamps, portable history revisions and queue deadlines remain exact. A corrupt frame refuses the entire upgrade. Existing old transactions finish before this boundary; already-prepared old header writes then fail with SQLSTATE `23514`.

Migration 028 repeats complete native validation under the exclusive format-row/workspace locks before adding `freight_workspaces.synthetic_demo boolean NOT NULL DEFAULT false`. A constraint permits a true flag only for tenant `demo`. Every retained row stays false, including old sample databases: sample-like records cannot prove origin. The same transaction advances only compatibility metadata, guards and ledger to 21/028; original blocks, files, history revisions, timestamps and queue hints remain. A newly created server demo seed commits its records and true flag together. Existing verified seeds reopen without resetting edits; unverified tenants refuse demo startup. Use `FREIGHT_DEMO=false` with named reviewers or a fresh isolated demo database. Never force a provenance flag based on record contents.

The registry, outer header and inner marker must agree on exact integral format 21. Storage-backed calls and readiness refuse incompatible formats with `503`; `/health` is a compatibility alias for readiness; `/live` reports process liveness. Workers cannot enumerate or claim new persisted work through an incompatible Store. Preceding format-19/20 canonical headers and aggregate writers carrying a forged current marker are refused. Related file inserts roll back with a failed write. Privileged database owners can bypass guards; this mechanism does not authorize arbitrary SQL writers.

Reads fetch the header and originals at one repeatable-read snapshot. Mutations lock the tenant and decode the complete source. Blocking decode/preparation workers own the actual transaction and pool connection through queued/running CPU work and cancellation. They never commit independently. Changed/deleted blocks, final header, membership metadata, derived invalidation, receipts and original file changes commit in the caller's transaction.

Preparation overlays actual typed changes onto the original raw source. Unchanged precise numbers, opaque keys, ignored nested evidence, timestamps and absent defaults survive. Unknown business roots and malformed shipment/finance records are refused. Operations-case/policy data has a narrow quarantine path: unsupported evidence is retained as raw JSON, human case actions fail closed, and physical shipment facts can still be recorded without deleting that evidence.

## Original retries and selected reads

Existing receipts cannot be changed or removed. Exact command/upload/source-observation retries return their original result without updating the header, blocks, counters, timestamps, audit or candidate files. A callback with a new delivery identity remains a real change even when its document was already received. Request fingerprints, stopped saves, ownership and optimistic versions remain authoritative.

A selected original receipt verifies its own identity and collection metadata; it does not certify unrelated tenant history. A new save still uses the locked, complete source check. Selected history reads retain source revisions, captured prefixes, original identities and scope checks. Full native reads validate every block. `freight_workspace_originals(tenant)` is a read-only SQL diagnostic reconstruction, not an alternate writable source.

## Separate schema application from API startup

`stateset-freight --migrate` requires `DATABASE_URL` for the migration role, applies pending embedded migrations and verifies the migration ledger/storage format in the existing guarded transaction. It exits after closing its pool, without starting HTTP, workers or identity integrations and without creating a demo tenant. API credentials and integration JSON are unnecessary for this command. It explicitly selects schema application even when `FREIGHT_MIGRATIONS=verify` is present. A termination signal cancels the command and exits unsuccessfully; inspect the database and rerun the same job before deploying servers.

`FREIGHT_MIGRATIONS=apply` remains the default for existing installations. `verify` requires PostgreSQL and all 31 exact migrations in the current schema, checks format 21, and issues no schema DDL or migration-history updates. Empty, unknown and non-Unicode policy values refuse startup. Both modes use the same transaction-level migration advisory lock, 10-second lock timeout and 60-second statement timeout. Verification waits behind a schema job and then inspects the committed ledger. Missing, incomplete, modified or future metadata refuses startup without attempting repair.

Use the same explicitly configured, protected `search_path` for the migration and runtime connections. Verification rejects metadata found only in a later search-path schema when the current schema has no installation. This check does not authorize other schemas in the path: only trusted owners should be able to create objects in schemas used by the application. PostgreSQL explains the [schema search-path trust boundary](https://www.postgresql.org/docs/16/ddl-schemas.html).

Provision a separate login with no ownership, privileged memberships, superuser, database/role creation, replication or RLS-bypass rights. Keep its authentication material in the intended secret provider. For a dedicated database named `freight`, an existing fresh login named `freight_app`, and an owner-installed application in `public`, run the following as the schema owner/administrator after a successful schema job. The database and schema names are examples; use the actual protected installation schema consistently.

```sql
REVOKE ALL ON DATABASE freight FROM PUBLIC;
GRANT CONNECT ON DATABASE freight TO freight_app;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO freight_app;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO freight_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO freight_app;

DO $$
DECLARE item record;
BEGIN
  FOR item IN
    SELECT tablename FROM pg_tables
    WHERE schemaname = 'public'
      AND tablename NOT IN ('freight_schema_migrations', 'freight_storage_format')
  LOOP
    EXECUTE format('GRANT INSERT, UPDATE, DELETE ON public.%I TO freight_app', item.tablename);
  END LOOP;
END $$;

GRANT UPDATE(singleton) ON public.freight_storage_format TO freight_app;
```

This example assumes a fresh role without previous grants or memberships. Reconcile grants for every new release and restore; the schema job does not create logins or manage privileges. Restrict database creation and temporary-table rights as well as schema creation. Keep table/function/schema ownership with the migration identity. Application SQL functions retain their installed invoker permissions; this recipe does not grant owner privileges or change function definitions.

The format check uses `SELECT ... FOR SHARE`. PostgreSQL requires update permission on at least one column for that lock, so the role receives only `UPDATE(singleton)` on the format table. The existing constraint fixes this key to true; `version` remains unwritable and migration history remains read-only. See PostgreSQL's [locking permission requirement](https://www.postgresql.org/docs/16/sql-select.html) and [column grants](https://www.postgresql.org/docs/16/sql-grant.html).

Run the serving process with the runtime connection and `FREIGHT_MIGRATIONS=verify`, then perform the readiness and authenticated workflow checks below. The supplied Compose file passes the policy through but still uses its development database owner; setting `verify` alone does not provision a separate role. In-memory demo mode uses `apply` and has no schema job.

The [restricted-role image recovery driver](../scripts/qualify-restricted-recovery.py) extends the complete existing restore drill. It runs the actual nonroot API image as a schema-only job, serves under a different login, checks denied DDL/history/version writes, and recreates the retained role/grant recipe on the fresh cluster before comparing complete recovery manifests. This local profile uses trust authentication and retained controller configuration; password/TLS and lost-role/configuration recovery require separate qualification.

## Compose with separate credentials

[compose.restricted.yaml](../compose.restricted.yaml) provides an explicit workflow for a dedicated Freight database. It starts PostgreSQL with SCRAM host authentication, runs the actual API image's `--migrate` job, runs a separate PostgreSQL client to configure `freight_app`, then starts the API with that login and `FREIGHT_MIGRATIONS=verify`. The API receives only the runtime password. The administrator password stays with PostgreSQL and the two short-lived administrative jobs. The API also waits for a separate healthy PDF renderer using the same selected API image. The renderer has no database credentials, database network or TLS mounts; only the API receives its read-only socket mount. The web service waits for the API's default image healthcheck. Compose supports [successful one-shot dependencies](https://docs.docker.com/compose/how-tos/startup-order/); this startup ordering does not drain an existing deployment.

Use the existing `.env.example` for application settings and add a nonempty `FREIGHT_RUNTIME_DATABASE_PASSWORD` that differs from `POSTGRES_PASSWORD`. Both are literal password values; keep the connection URLs password-free. The runtime password can contain Unicode, spaces, dollar signs, quotes, backslashes and URL delimiters, but cannot contain CR or LF because it is supplied to psql's line-based password prompts. The provided administrator identity is the database/bootstrap owner; commission its permissions and credential storage separately for the intended deployment.

```sh
docker compose -f compose.restricted.yaml up --build --detach
```

This file uses a separate `freight-restricted-postgres` volume and private `freight-restricted-renderer-socket` tmpfs volume, and publishes only API/web loopback ports. The renderer adds its own 384 MiB/one CPU/16 PID budget; its socket volume contains no document originals. It does not adopt the development Compose volume. PostgreSQL authentication initialization variables apply to fresh volumes; they do not replace an existing `pg_hba.conf`. Check the actual host rules after restoring an existing database. Existing databases need a verified backup/restore and role commissioning procedure before changing their ownership or startup workflow. The template uses the protected `public` schema and the current release's format 21/31-migration installation. Do not use its dedicated-database grants on a shared application database.

The [role provisioner](../scripts/configure-runtime-role.sh) runs [its grant recipe](../scripts/configure-runtime-role.sql) and password change in a single guarded transaction. A new `freight_app` is marked as managed; an existing role must have that marker, safe attributes, no memberships, no object ownership and no role/database-specific settings. Unmanaged collisions and elevated identities refuse before modification. The recipe removes previous direct table, sequence and column grants, then grants business DML, metadata reads and the format-lock column. Metadata write access remaining through `PUBLIC` causes the whole job to refuse. Passwords are passed to psql's client-encrypted [`\password` command](https://www.postgresql.org/docs/16/app-psql.html), keeping their cleartext out of SQL statements and command arguments.

For an upgrade or runtime-password rotation, first drain traffic/callbacks/partner calls as described below, then stop both serving processes. Build/select the intended images, update the runtime password in the controlled configuration, and run the jobs explicitly before recreating servers:

```sh
docker compose -f compose.restricted.yaml stop web api renderer
docker compose -f compose.restricted.yaml run --rm --no-deps migrate
docker compose -f compose.restricted.yaml run --rm --no-deps grants
docker compose -f compose.restricted.yaml up --detach --no-deps --force-recreate --wait renderer
docker compose -f compose.restricted.yaml up --detach --no-deps --force-recreate api web
```

After each command succeeds, perform the readiness, authenticated access and original-receipt checks below before resuming traffic. A job failure leaves serving stopped until resolved. Setting `POSTGRES_PASSWORD` on an existing volume does not rotate its administrator's actual password; that is a separate administrator operation. The role job explicitly rotates the runtime password, including when reapplying an unchanged password. Environment variables remain visible to privileged container administrators; external secret custody, TLS, ingress, intended capacity and lost-key/configuration/role recovery require separate commissioning.

The reusable local qualification accepts a new report path and source-bound image verification, creates only a new labelled synthetic Compose project, and removes its own database volume/network/containers afterward. It requires the recorded images already cached; no deployment database URL or real credentials are accepted:

```sh
python3 scripts/qualify-authenticated-roles.py \
  --verification "$OWNED_IMAGE_VERIFICATION" \
  --tui "$QUALIFIED_TUI_BINARY" \
  --output /tmp/freight-authenticated-roles-new.json
```

If Docker's default address pools are exhausted, the role and database-TLS qualification commands accept `--docker-subnet` with an unused RFC1918 IPv4 `/24` through `/28`. Choose a range outside existing host/VPN routes. The driver rejects overlap with existing Docker allocations, applies the subnet only to its owned Compose project, records the override hash and verifies network removal during cleanup. Existing networks are retained.

## Certificate-verified database connections

[compose.database-tls.yaml](../compose.database-tls.yaml) layers database TLS onto `compose.restricted.yaml`. It sets `PGSSLMODE=verify-full` and an explicit read-only CA bundle for the schema job, role job and API. Its [HBA rules](../scripts/postgres-tls.pg_hba.conf) reject unencrypted TCP and require SCRAM on encrypted connections; the local bootstrap socket retains its separate administrative profile. The PostgreSQL configuration enables TLS with a minimum of TLS 1.2. Public web/API HTTPS termination is a separate deployment concern.

Supply an operator-managed server certificate chain, matching unencrypted private key and trusted CA bundle using file paths. The server certificate needs a DNS subject alternative name for the Compose database hostname `postgres` and server-authentication purpose. The first certificate in the chain must be the leaf; append any intermediate certificates. Database clients receive the CA bundle, without server private-key mounts. The renderer receives neither database TLS files nor database settings; the TLS overlay preserves the API's read-only renderer socket mount. PostgreSQL describes [server certificate/chain layout and permissions](https://www.postgresql.org/docs/16/ssl-tcp.html); [`verify-full`](https://www.postgresql.org/docs/16/libpq-ssl.html) checks certificate trust and hostname.

```sh
export FREIGHT_DATABASE_TLS_CERT=/secure/path/server-chain.crt
export FREIGHT_DATABASE_TLS_KEY=/secure/path/server.key
export FREIGHT_DATABASE_TLS_CA=/secure/path/ca-bundle.crt
docker compose -f compose.restricted.yaml -f compose.database-tls.yaml up --build --detach
```

Keep the host private-key file restricted to its administrators. The CA bundle is public certificate material and must be readable by the nonroot client containers. A short-lived root helper validates the supplied chain, validity, DNS SAN, hostname, server purpose and key match before staging a complete generation in the dedicated TLS volume. It validates the copied bytes again, assigns the PostgreSQL image's user and a mode-0600 private key, then atomically switches the `current` symlink. PostgreSQL mounts that volume read-only. The helper does not generate a certificate authority or silently accept an untrusted/self-signed certificate.

Publish certificate changes with PostgreSQL stopped. Validation or staging failure preserves the previous publication; interruption immediately after the atomic switch can leave a complete new generation published even when the command exits unsuccessfully. Inspect the publication and command result before proceeding. Earlier generations remain in the protected volume for rollback, so include that volume in the intended private-key retention/custody policy.

Use the same two-file configuration for every maintenance command. For example, after draining external traffic/callbacks and selecting new certificate files:

```sh
freight_tls_compose() {
  docker compose -f compose.restricted.yaml -f compose.database-tls.yaml "$@"
}
freight_tls_compose stop web api renderer postgres
freight_tls_compose run --rm --no-deps database-tls-files
freight_tls_compose up --detach --no-deps --wait postgres
freight_tls_compose run --rm --no-deps migrate
freight_tls_compose run --rm --no-deps grants
freight_tls_compose up --detach --no-deps --force-recreate --wait renderer
freight_tls_compose up --detach --no-deps --force-recreate api web
```

Check authenticated readiness and original receipts before reopening traffic. This workflow restarts PostgreSQL; it does not claim hot certificate reload or zero downtime. Trust/certificate rotation and runtime-password rotation are distinct operations. External issuance/revocation, private-key custody, intended-deployment networking, public HTTPS, capacity and disaster recovery require their own commissioning.

The offline [certificate/publication regressions](../scripts/test_postgres_tls.py) use real OpenSSL and filesystem operations, including intermediate chains, expiry, wrong authorities/hostnames/purpose, missing DNS SAN, source-key races, private permissions, rejected rotations and a signal immediately after the atomic switch:

```sh
python3 -m unittest discover -s scripts -p test_postgres_tls.py -v
```

With the exact source-bound API/web/PostgreSQL images cached, the reusable image qualifier generates its own private synthetic PKI and fresh labelled Compose project, runs the complete credential and TLS controls, then removes only that project's volumes/network/containers. It accepts no deployment certificates, passwords or database URL:

```sh
python3 scripts/qualify-database-tls.py \
  --verification "$OWNED_IMAGE_VERIFICATION" \
  --output /tmp/freight-database-tls-new.json
```

## Coordinated restore with database TLS

## Deploy and roll back

1. Restore a whole-database/schema backup into isolation and verify it before a production change. Include durable demo provenance, headers, **all canonical blocks**, membership metadata, original files, ledger, registry, functions, triggers and derived relations. Header-only or file-only backups cannot restore the platform. Include the separately owned EDI/Sync databases, required EDI files and separately retained service configuration/keys in a coordinated recovery plan.
2. Pause traffic/callback delivery, stop workers and drain active partner calls. Inspect in-flight leases and upstream acceptance before retrying a handoff. A database format fence cannot cancel an HTTP request already sent by an older process.
3. Use a maintenance window for exclusive workspace locking and complete validation (plus backfill when upgrading from an aggregate release). Run the new image's `stateset-freight --migrate` with the migration role against the intended database/schema, reconcile runtime grants, then start the new API with the runtime login and `FREIGHT_MIGRATIONS=verify`. If the job or startup times out or refuses data, resolve the cause and retry; do not disable guards or lower markers to force startup. Backfill time and deployment resource budgets still require native measurements for the intended workload. Existing owner-based deployments may retain the default `apply` policy.
4. Check `/ready`, authenticated workspace/catalog/history reads, exact original document bytes, financial journals, original receipt retries and pending queues before resuming work. Tenant serialization and supporting-file integrity checks still apply.
5. Roll back application code only to a release supporting the current database format and ledger. Returning to format 20 or an earlier format requires a separately qualified restore/conversion procedure with all writers stopped. Reconcile later business activity and external carrier/bank actions; restoration can discard activity after the recovery point. Never relabel canonical blocks as an aggregate by changing a marker.

## Local checks

`cargo test --workspace --locked` runs active Rust and codec tests. With an owned disposable PostgreSQL URL, `TEST_DATABASE_URL=... cargo test --workspace --locked -- --ignored --test-threads=1` also exercises durable scenarios. `bash scripts/check.sh` includes workspace formatting/lints/tests, Node checks, a web build and production-browser scenarios.

[Canonical storage tests](../tests/canonical_blocks.rs) check exact original migration, native deltas, original files, rollback and absent optional collections. [Format tests](../tests/storage_format.rs) retain earlier-format upgrades, future-version refusal, old-writer fencing and immutable ledger checks. [Receipt tests](../tests/idempotency_writes.rs) observe actual header/block write traces and physical original tuples. History, queue, expiry, document, financial and counterparty scenarios retain their business and corruption assertions against the canonical representation.

## Queue scheduling metadata

[Migration 031](queue-invalidation.md) separates conservative block invalidation from exact header projection. It preserves format 21 and original business records. Apply it through the normal schema job before starting the new binary in `verify` mode. An older binary refuses the newer migration ledger on restart; existing native writers still finish with a header update that recomputes exact hints.


## Document previews in managed-role and TLS profiles

The restricted profile selects the isolated renderer explicitly; it does not depend on native Landlock availability inside the API container. The TLS overlay merges its public database CA mount with the API's read-only socket mount. The renderer receives only its private tmpfs socket volume, with no database password, CA/key files or application settings. Both services use the selected API image; upgrade procedures above recreate and wait for the renderer before recreating the API/web services.

The role/TLS qualifiers now verify the resolved Compose boundary and actual renderer configuration, upload a synthetic PDF through the restricted database login, check every expected pixel of both pages, exercise missing/other-tenant credentials, preserve original bytes/review metadata and verify fail-closed preview plus continued original/readiness access while the renderer is stopped. Restart recovers previews without recreating the API. Password rotation/recreation retains the same original and page identity. The TLS qualifier checks document retention again after its certificate fault controls.

Supply a current source-bound image verification manifest with exact cached API/web/PostgreSQL IDs. The historical default manifest belongs to its recorded older source and intentionally refuses a different API. `--tui` optionally adds five actual API/TUI PDF PTY modes after password rotation, and again after all TLS fault controls in the TLS profile. If the image verification also binds a client executable/source inventory, the selected client must match it. A missing `--tui` is explicitly recorded as not requested, not a client pass. PTY checks verify transport, identity and lifecycle; they do not establish rendered-window or independent dispatcher/accessibility acceptance.

```sh
python3 scripts/qualify-database-tls.py \
  --verification "$OWNED_IMAGE_VERIFICATION" \
  --tui "$QUALIFIED_TUI_BINARY" \
  --output /tmp/freight-database-tls-documents-new.json
```

Both commands own fresh durable PostgreSQL Compose volumes and synthetic credentials/PKI, then remove only their project resources. These checks do not replace the full release matrix, production capacity, live-provider commissioning, native vulnerability review or independently retained/off-host recovery acceptance.
