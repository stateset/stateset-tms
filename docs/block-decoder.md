# Block decoder validation and allocation

The [initial block comparison](storage-blocks.md) exposed expensive reads in a storage representation that substantially reduced separate-record disk use and write amplification. Its decoder copied every record into a second map, materialized the complete workspace, normalized and packed that workspace again, and serialized two complete frames for comparison. The CLI also retained its complete input string through decoding.

The new decoder canonicalizes original payloads once and builds identity/order indexes that borrow the original entries. Encoder and decoder share the same partition constructor: the encoder supplies owned entries, while the decoder supplies references. Validation compares every expected address, member and order against the stored blocks. Missing/extra blocks, invalid subdivisions, duplicates, mismatched descriptors and unsupported frames still refuse the complete operation. Workspace output is assembled directly from the validated original entries and returned only after the complete frame passes. The CLI releases its input string after owned frame parsing.

This removes a second complete decode/repack and two complete frame serializations; it does not replace integrity checks with a checksum, trust an unchecked partial read, or discard oversized originals. Codec frame version 1 and its JSON representation remain unchanged. The generic `Block<T = Entry>` representation supports borrowed validation internally while retaining the owned default public frame type.

## Qualification boundary

The comparison uses the retained preceding release executable, exact original before/after archives from all ten native save outcomes, byte-identical packed frames and decoded output, strict JSON type/value/decimal-scale checks, corruption controls, and counterbalanced native CLI resource observations. Tests specifically exercise unnecessary subdivisions, within-block item order, duplicate identities across distinct blocks, canonical payload text, corrupt descriptors and extreme ordinals.

## Native resource observations

The comparison makes 48 dedicated process observations: both executables, both modes, both spans, two fixture sizes and three repetitions. Variant order reverses on alternate repetitions; unpack order reverses pack order. The complete trial order, CPU/system/wall values, native `/usr/bin/time` logs and output bindings remain retained. Measurements run without the database sampling thread and include CLI input/output and process startup.

Median observations for the 1,000-shipment fixture:

| Span | Mode | Preceding user CPU seconds | Current user CPU seconds | Preceding peak RSS MiB | Current peak RSS MiB |
| --- | --- | ---: | ---: | ---: | ---: |
| 16 | Pack | 1.07 | 1.14 | 141.73 | 141.80 |
| 16 | Unpack | 2.13 | 0.93 | 233.02 | 54.97 |
| 64 | Pack | 0.99 | 1.02 | 141.69 | 141.71 |
| 64 | Unpack | 2.16 | 0.98 | 234.26 | 53.94 |

Unpack peak RSS falls by about 76–77%, and user CPU by about 55–56%, in these matched finite observations. Total user-plus-system CPU medians fall from 2.34 to 0.99 seconds at span 16 and 2.36 to 1.06 seconds at span 64. Current unpack user CPU ranges from 0.68 to 1.10 seconds and 0.56 to 1.05 seconds, respectively. The retained preceding ranges also vary substantially; this is not a universal timing guarantee.

At 100 shipments, median unpack RSS falls from about 26.03 to 8.55 MiB at span 16 and 25.85 to 7.68 MiB at span 64. Packing remains a separate cost: the 1,000-shipment pack medians increase slightly, and process memory stays near 142 MiB. The decoder improvement does not establish a uniform read/write improvement. Complete typed business work and packing belong in the eventual end-to-end application measurement.

The source-controlled application and integrations have not changed. This component's resource observations are not native API latency or an embedded process memory bound. The CLI input release contributes to the observed process memory change; existing allocator state and typed Workspace materialization in an API are separate costs. The earlier PostgreSQL measurements remain bound to their original executable. Byte-identical frame compatibility preserves that physical representation, but a new full-read latency measurement still requires actual PostgreSQL and application execution.

All 244 protected production inputs and 276 preceding evidence files retain their original hashes. The existing 444-test application and separate 45-check actual-service recovery qualifications retain their original sources and scope; they are not recounted as fresh tests of this component.

## Application work that remains

A canonical adapter still needs an explicit business-format transition and checksummed migration, old-writer refusal, complete source/financial/receipt/file atomicity, transaction ownership through queued CPU work and cancellation, replacement derived catalog/history/deadline maintenance, and exact original receipt lookups. The codec's standalone dependency lock must be reconciled with the application lock. Qualify complete application tests, actual Freight/EDI/Sync recovery and end-to-end read/write behavior against that integrated source before adopting the new representation.