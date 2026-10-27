# Freight storage block codec

A lossless Rust JSON codec used by the [in-progress canonical storage integration](../../docs/canonical-blocks.md). It is now a Freight workspace member and uses the root dependency lock. Codec frame version 1 is separate from business format 19. Earlier isolated measurements retain their exact standalone sources and executable.

Arrays use stable ordinal ranges; objects use SHA-256 buckets of literal keys. Deterministic subdivisions enforce the configured count and canonical JSON byte targets. A single oversized original is retained. The complete root manifest, collection shape, membership, ordinals, addresses and partitions must validate before `unpack` returns a workspace.

Numbers retain decimal precision and scale without `f64`. Duplicate JSON member names are refused. Duplicate business IDs in arrays remain distinct. Strings and opaque objects retain their original values; whitespace and object-member order are canonicalized.

Build and check from the workspace root:

```sh
cargo test -p stateset-freight-storage-codec --locked --offline
cargo clippy -p stateset-freight-storage-codec --locked --offline --all-targets -- -D warnings
cargo build -p stateset-freight-storage-codec --locked --offline --release
target/release/stateset-freight-storage-codec pack 64 65536 < workspace.json > frame.json
target/release/stateset-freight-storage-codec unpack < frame.json > restored.json
```

`span` must be a power of two from 1 through 1,024. `target_bytes` ranges from 1,024 through 1,048,576. These limits apply to grouped canonical JSON; they do not bound an individual original, transport, tenant or process. Lexical numeric expansion is limited to one million digits. Canonical JSON nesting is limited to 128 levels, consistently in packing and unpacking. PostgreSQL JSONB snapshots already contain expanded decimal text.

The preceding [qualified decoder optimization](../../docs/block-decoder.md) validates borrowed entries with the encoder's shared partition constructor, then assembles complete output without a second workspace/repack or full-frame comparison. Its finite CLI CPU/RSS observations retain their exact earlier executable and do not establish embedded API memory or capacity. The CLI releases its owned input text before decoding. `unpack_in_place` additionally lets an owner retain a fully validated normalized frame; errors can leave partial normalization and must never authorize persistence. Current application integration requires fresh coherent regression, migration, performance and Freight/EDI/Sync recovery qualification.