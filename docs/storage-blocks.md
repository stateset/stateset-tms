# Native record-block codec and storage experiment

The subsequent [decoder optimization](block-decoder.md) preserves this version-1 representation while reducing duplicate decoding/allocation. The PostgreSQL, fourteen-test and resource numbers below remain historical observations of the original executable; the newer component has its own nineteen-test, compatibility and resource qualification.

The standalone Rust crate in `storage-codec` supplies a versioned `pack`/`unpack` codec. Array blocks use fixed ordinal ranges. Literal object keys use SHA-256 buckets with deterministic subdivision. Groups split further when their canonical JSON exceeds the configured target. The PostgreSQL probe compares spans of 16 and 64 with a 64 KiB canonical target. A single oversized original record or scalar is retained; this target is not a universal record, wire, process-memory or tenant bound.

The frame retains an exact root manifest, descriptor types/counts, array ordinals and literal keys. Decoding refuses missing roots/records, duplicate identities or blocks, unsupported layouts, wrong block addresses and invalid grouped sizes before returning a workspace. Canonicalization preserves decimal scale without binary floating point, retains opaque private-looking keys, and rejects duplicate JSON member names. Lexical numeric exponent expansion has a one-million-digit budget; native PostgreSQL snapshots already contain expanded numeric text.

Full-hash collisions retain their literal members with a deterministic fallback. An insertion can change blocks within its selected bucket; it does not require regrouping every object member. Array appends and local changes preserve unrelated fixed-range blocks. Physical block rewrites can contain unchanged logical records; those original payloads must still compare exactly.

## Qualification scope

Native Rust tests cover exact large numbers, Unicode, null/boolean/zero distinctions, opaque values, duplicate business IDs, array order, configuration/version refusal, missing roots and records, block-address validation, stable neighboring groups, collision fallback and oversized originals. The PostgreSQL experiment adds actual stored-JSONB round trips, tenant isolation, stale-write rollback, complete reads and read-only original receipt selection.

All four storage layouts use the same actual archived before/after outcomes from the qualified Freight API. Both packed layouts and the reference aggregate/record layouts omit production business, format, history, catalog and queue triggers. Data preparation and native business application are outside SQL timings. Complete-read observations include psql, the Python observer, serialization, process startup and tracing; they are not native API latency measurements. Separate native CLI resource observations are needed to distinguish codec CPU/memory from that observer overhead.

The PostgreSQL limit remains two CPUs and 768 MiB. Continuous sampling and final kernel diagnostics cover the owned storage workload. All layouts coexist in one cluster, so global memory high water cannot be assigned to one layout. Initial and post-write physical sizes include tables, TOAST and indexes; dead versions and normal background maintenance affect post-write sizes. The synthetic histories are highly compressible.

## Measurements and decision

Initial physical sizes, including table, TOAST and index bytes:

| Shipments, history depth 4 | Aggregate MiB | Separate records MiB | 16-record blocks MiB | 64-record blocks MiB |
| --- | ---: | ---: | ---: | ---: |
| 100 | 0.211 | 3.594 | 0.734 | 0.367 |
| 1,000 | 1.484 | 35.391 | 4.094 | 2.805 |

At 1,000 shipments, 64-record packing cuts the observed separate-record footprint by about 12.6 times and costs about 1.89 times the compressed aggregate. Post-write sizes are retained separately. The 16-record footprint falls after writes while the aggregate grows: these include background maintenance and dead versions, so their difference is not an intrinsic compression claim.

Both packed layouts rewrite only three or four blocks and two descriptors per save, plus the tenant revision. Changed canonical block bodies contain 21,080–74,651 bytes for span 16 and 50,199–103,772 bytes for span 64 in the 1,000-shipment fixture. Unrelated existing root/block tuples retain their actual `xmin` and `ctid`: at least 2,583 for span 16 and 785 for span 64. These are physical tuples, not counts of unchanged logical records. SQL, descriptors, protocol and revision overhead are outside the block-body byte figures.

The 1,000-shipment median SQL primitive execution times are 285.870 ms for the aggregate, 1.161 ms for separate records, 1.467 ms for span 16, and 2.057 ms for span 64. These omit application preparation, complete reads, production maintenance triggers and the revision guard's uninstrumented statement. They are not transaction or API latency. Tracing-heavy complete-read medians are 4.25, 6.08, 10.10 and 8.60 seconds, respectively, with wide ranges. The packed probe commits its SQL snapshot before invoking the CLI decoder; a production adapter must retain transaction ownership through actual decoding. Both unfavorable full-read observations and their observer overhead remain visible.

Without PostgreSQL sampling, native 1,000-shipment pack user CPU ranges from 1.09 to 1.18 seconds, with about 141.5–141.8 MiB peak process RSS. Unpack user CPU ranges from 2.17 to 2.40 seconds, with about 232.9–234.1 MiB peak RSS. System CPU and wall times remain in the report. The 100-shipment pack/unpack RSS observations are about 15 and 26 MiB. These short finite CLI observations include process I/O, omit typed Workspace materialization and do not establish an API memory limit or sustained capacity.

Continuous and final PostgreSQL diagnostics show no OOM or memory-limit hits; the kernel historical peak is 629.9 MiB under the 768 MiB limit. The controller and codec run outside that database limit. All four layouts coexist, so this peak is not attributable to one representation.

The initial next gate was reducing and remeasuring native decoder CPU and allocation while retaining complete integrity validation and identical original values. This original decoder materialized and repacked the workspace and serialized both frames to compare them. The [subsequent decoder work](block-decoder.md) removes that duplication and retains this exact executable and all unfavorable observations. Canonical application integration and end-to-end measurements remain required before adopting the representation.