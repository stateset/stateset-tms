# Third-party notices

The project's `MIT OR Apache-2.0` choice applies to first-party code. It does not replace dependency licenses or grant rights to third-party trademarks.

## Vendored StateSet primitives

`vendor/stateset-primitives/` contains StateSet iCommerce primitives 1.39.0, from <https://github.com/stateset/stateset-icommerce>. Copyright (c) 2026 StateSet. The component is licensed under MIT OR Apache-2.0; its original license files remain in that directory. The Freight application uses these types through `crates/commerce-bridge/`.

## Vendored StateSet pricing

`vendor/stateset-pricing/` contains StateSet iCommerce pricing 1.39.0, from <https://github.com/stateset/stateset-icommerce> at revision `b85979972f177217de5722bc6dda29f3c542afc0`. Copyright (c) 2026 StateSet. The component is licensed under MIT OR Apache-2.0; its original license files and `UPSTREAM.json` provenance remain in that directory. The Freight application uses its currency minor-unit table and rounding through `crates/commerce-bridge/`.

## Vendored SQLx core

`vendor/sqlx-core/` contains published SQLx core 0.8.6 under MIT OR Apache-2.0. Its original authorship and license files are retained. The registry archive checksum and complete original file hashes are recorded in `UPSTREAM.json`. Only its normalized Cargo descriptor changes: `webpki-roots` uses the fixed 1.0.10 bundle directly in place of the 0.26 compatibility wrapper. All Rust sources and the original Cargo descriptor are unchanged. This preserves Freight's Rust 1.90 support; SQLx 0.9.0 requires Rust 1.94. Remove the patch when a compatible upstream release replaces it or a separately qualified SQLx upgrade is adopted.

## Rust and JavaScript dependencies

Exact resolved packages are recorded in `Cargo.lock`, `web/package-lock.json` and `mcp/package-lock.json`. Their package metadata and distributed license files are authoritative. Rust dependencies include MIT, Apache-2.0, BSD, ISC, Unicode, Zlib, Unlicense and CDLA-Permissive terms, sometimes in combined expressions. JavaScript packages also include 0BSD and CC-BY-4.0 terms.

The web lockfile includes `sharp` and platform-specific `@img/sharp-*` / `@img/sharp-libvips-*` packages. Several declare **LGPL-3.0-or-later**, alone or combined with Apache-2.0 and MIT. These are third-party image-processing libraries. A binary/container redistribution must preserve applicable notices and satisfy those components' redistribution requirements. Review the actual target-platform packages and upstream source/license material before distributing a binary or container; a source dependency inventory is not a completed distribution-compliance review.

Do not remove dependency notices when creating application bundles. New dependencies need a license review, including native libraries and packages for each distribution platform. See [public-release readiness](docs/open-source-readiness.md) for current audit scope and remaining work.

To collect actual package notice files and a lock-bound inventory, follow the [distribution review procedure](docs/security/publication-review.md#dependency-notice-bundle). Missing installed license files and native component terms are reported explicitly; successful collection does not attest compliance.

[Version-bound upstream materials](third-party/README.md) supplement package-provided notices. They include original Next.js and glob-to-regexp terms, GNU/libvips reference terms, publisher native component/build references and prepared caniuse-lite attribution. Their original terms remain attached. Build-script licensing does not replace compiled-library licensing, and no copyright holder is invented for client-only. See [provenance and remaining review](docs/security/upstream-notices.md).

## PDF preview runtime packages

The Linux API image includes Debian `poppler-utils` and its native libraries, `util-linux` and DejaVu fonts for PDF previews. These are separate process/runtime dependencies and are not inventoried by `Cargo.lock` or the Rust license gate. Preserve each installed package's copyright and license material under `/usr/share/doc/`. Record the exact image package versions and collect their notices when reviewing a binary/container distribution. The first-party dual license does not replace their terms; the source-only publication review does not clear a container distribution.
