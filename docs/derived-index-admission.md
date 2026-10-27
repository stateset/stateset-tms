# Derived-index admission and financial workload

## Behavior and configuration

The staff workspace response serializes retained history and financial collections directly from typed records, avoiding a second complete JSON value tree. Derived views keep their calculations, explicit field whitelist, privacy omissions and HTML redactions. Encoding completes before HTTP success. Complete/deferred history and approval combinations are compared with the retained independent value projection; errors and private-field omissions remain checked.

Catalog pages have one active slot per Store, independent of whole-workspace snapshot/CPU admission. Their finite queue defaults are 64 pending, at most 16 per tenant, and a 5,000 ms deadline. They add neither database connections nor a reserved connection. History retains snapshot read admission. The catalog behavior is enabled by default; the whole-workspace processing budget remains separately opt-in.

| Setting                              | Default                         | Accepted range  |
| ------------------------------------ | ------------------------------- | --------------- |
| `FREIGHT_CATALOG_MAX_PENDING`        | 64                              | 1–4096          |
| `FREIGHT_CATALOG_TENANT_MAX_PENDING` | smaller of 16 and total pending | 1–total pending |
| `FREIGHT_CATALOG_QUEUE_TIMEOUT_MS`   | 5000                            | 1–60000         |

The optional [Compose overlay](../compose.catalog-admission.yaml) forwards explicitly set catalog settings and omits unset values, including the dependent per-tenant default. Use it with `compose.yaml` and any required database TLS/restricted-role overlays. The six retained synthetic Compose checks cover unset, reduced and explicitly set queue configurations. The application rejects invalid limits at startup. Setting `FREIGHT_WORKSPACE_CONCURRENCY=1` opts into the separately documented [shared processing budget](workspace-admission.md); leaving it unset does not disable catalog/index admission.

Cold catalog and history index builders can lock the tenant workspace row. They now obtain the existing bounded local tenant write turn **before** acquiring a database connection. Transaction ownership retains both that turn and page admission through actual commit or rollback, including cancellation. Indexed pages remain independent of the whole-workspace CPU budget. Pool, SQL lock, read and processing limits are unchanged. Even warm indexed transactions join this local turn. Separate Store instances/processes still coordinate through PostgreSQL; cross-instance serialization conflicts retain bounded retries.

Cold tender-deadline preparation uses the same local write turn before pool acquisition, without taking a snapshot/CPU/page lease. Two additional actual PostgreSQL guards pause the real deadline builder after its tenant lock and separately abort its caller. Both reproduce 503 instead of queueing on the preceding production code, while original receipts/readiness stay available. Both pass after the change, including ownership through actual rollback and no original rewrite. This proves the local admission mechanism; it does not identify the earlier controller trace’s holder. SQL, limits and storage remain unchanged.

Two actual PostgreSQL guards pause real catalog/history inserts after obtaining the tenant row lock. Before the fix, a new mutation bypasses that local owner and fails the fast SQL lock budget. After the fix it queues before pool acquisition; protected original receipts and readiness remain available before builder release. The original cross-instance history test still requires two blocked SQL builders, exact before/after snapshots, no physical source rewrite and exactly three retries. Its setup now uses independent Stores because two clones intentionally share local admission. The preceding browser traces alone do not establish the original lock holder in those historical failures.

## Preceding admission qualification and retained attempts

The preceding source `fa808f8e` has complete standard workspace results from a fresh empty memory-backed target: 334 Rust/component and 100 actual PostgreSQL cases. Strict all-target Clippy and formatting pass. The first host build exhausted disk and the first native export hit a null-argument error; those failures remain retained. Export was corrected without rerunning or narrowing tests, and all 108 unstripped native files were read back before owned cleanup. Node, X12, full browser, host release API/fixture, actual Dockerfile image, finite financial and both recovery modes then passed independently.

| Application scope           | Passing scenarios |
| --------------------------- | ----------------: |
| Rust/component              |               334 |
| Actual PostgreSQL 16        |               100 |
| Node                        |                65 |
| Actual X12 codec contracts  |                 8 |
| Complete production browser |               105 |
| Total                       |               612 |

The single complete browser run has no retries or skips, uses the qualified native executable, restricted verification-only database login, work concurrency one and two Tokio workers. All 2,172 prepared frontend artifacts remain bound to their earlier production build. Strict all-target Clippy and formatting pass. The actual unchanged production Dockerfile builds API image `sha256:8995faa1bd759507ca336bf5fb4ad96e3d706b97d40810d150996285fb804085`. Storage remains format 21/codec 1/migration 028; dependency locks and frontend inputs are unchanged.

The [direct serializer](direct-workspace-json.md) failed its catalog gate. The [catalog stage](catalog-read-admission.md) retained two 104-pass/one-failure browser runs and incomplete 243/246 and 245/246 workload attempts; its later timing diagnostic does not erase them. The preceding candidate's first PostgreSQL attempt exposed the now-serialized same-Store test setup. The next passed all 98 PostgreSQL cases but failed strict Clippy on an ignored helper's bare unwrap; the final source fixes that diagnostic and freshly passes 334/98, Clippy and formatting. An earlier shared-Cargo-cache qualification was invalidated; corrected builds use source-exclusive targets, copying only third-party artifacts when seeding. Every attempt, source snapshot, database backup and correction remains retained rather than counted as extra scenarios.

## Preceding finite invoice-rich profile and observer correction

The synthetic fixtures retain 1,000 shipments in each large tenant and 100 in the catalog probe. Each large tenant has 750 carrier invoices, customer bills and settlements, 1,500 cash movements, 3,000 journal entries, 6,000 journal lines and over 20,000 retained events/history entries. These are uniform USD examples with named synthetic review links, not live bank/carrier or independently reconciled operator samples.

The actual API and PostgreSQL images run with 512/768 MiB limits, two CPUs each, no swap, one read/processing job, two Tokio workers, two database connections, default allocator and error logging. The fixed plan offers 60 large reads, 60 catalog queries, 60 health requests, 60 readiness requests and six writes, with ten seconds between writes. Exact original-state verification gates each later write; unknown outcomes never receive blind retries.

| Offered traffic                                   | Result                                                            |
| ------------------------------------------------- | ----------------------------------------------------------------- |
| 60 large reads                                    | 45 complete HTTP 200; 15 explicit busy responses                  |
| 60 catalog queries                                | 60 complete HTTP 200                                              |
| 60 health / 60 readiness                          | 60 HTTP 200 per category                                          |
| Six original writes                               | Six committed once, no withheld arrivals                          |
| Six exact original replays, after offered traffic | Six original responses; no physical rewrite in runtime assertions |
| Observed API / PostgreSQL peaks                   | 458.72 / 613.83 MiB; no OOM or hard-limit fault                   |

All 246 offered requests are sent without transport or harness errors. Six legacy-SQL comparisons before/after traffic and replays retain exact original bytes for all three tenants. Full responses, versions, events, financial originals, receipt outcomes, seals and untouched tenants remain checked; warmed business responses match the old source except `operations.generated_at`. The independent audit checks logical originals and response bodies; per-replay physical tuple assertions belong to the retained runtime harness. Checked backups precede removal of owned resources.

## Preceding actual-image recovery and remaining requirements

The 71 recovery checks include all 58 preceding assertions and are separate from the 612 application scenarios. Actual Freight/EDI/Sync/web images traverse two genuinely different PostgreSQL clusters and actual EDI-volume loss. Restore recovers all 150 application tables and exact original finance, approvals, receipts, queues, files/control numbers, signed sessions and sealed saves. All three backends verify database CA/hostname, use SCRAM and reject wrong CA, hostname and password; plaintext is refused. Work concurrency one, two Tokio workers and default catalog limits are observed on both database generations.

Freight's nonroot schema jobs and restricted verification-only serving grants pass on both clusters; EDI/Sync retain their owner-login profiles. The TLS-only run retains controller state. The independent encrypted-controller run removes the original controller files, PKI and TLS volume, then starts a separate process from the recovered source/recipe and restores original credentials, signed session/sealed-save state and role/grant declarations. All 71 original guards pass, including exactly two inbound attempts and original body/authentication/history requirements. The host and pinned image cache survive; escrow remains separately held on the same laboratory host. Public archives exclude generated private credentials, keys, handoff data, database dumps and duplicate image binaries. Original coordinated backups remain private and included in the encrypted checkpoint. This is not off-host/lost-key custody or production RPO/RTO qualification. All owned resources and ports are checked after cleanup.

Production traffic admission, sustained capacity and upgrades, live identity/carrier/public-callback commissioning, off-host/key/configuration recovery, calibrated visibility/planning and independently reconciled finance datasets, and independent operator/accessibility outcomes remain open. Whole-workspace reconstruction and complete response bytes remain costs; larger histories, slow clients, replicas and additional buffers can exceed this finite profile. The platform goal stays active; these gates do not justify an overall A+ grade.
