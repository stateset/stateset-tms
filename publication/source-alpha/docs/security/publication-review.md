# Reviewing publication artifacts

The source manifest records a candidate snapshot, not permission to publish it. Review contribution rights, confidential material, identity fixtures, contacts and the exact files before publishing. Keep detailed review notes private. A source snapshot carries no private development history; publishing other branches, tags or assets requires their own review.

## Dependency notice bundle

For the exact platform and dependency installation intended for distribution:

```sh
cargo fetch --locked
npm --prefix web ci
npm --prefix mcp ci
freight_notices_parent="$(mktemp -d)"
python3 scripts/collect-third-party-notices.py --output "$freight_notices_parent/bundle" --fail-on-review
```

The collector preserves package notices, nested license directories, supplementary README/version material and separately identified pinned upstream texts. It validates locked npm metadata and file hashes, inventories native assets, refuses unsafe paths/symlinks and never overwrites an existing bundle. `--fail-on-review` retains the bundle and exits 2 when review remains; ordinary collection exits 0 without granting clearance. Neither mode attests distribution compliance.

This is a conservative dependency-source inventory, not an exact binary SBOM. Absent optional packages, build/runtime/OS dependencies and embedded native components require platform-specific consideration. Pinned materials resolve some text gaps; native package terms, caniuse-lite attribution and client-only's missing notice still require review. See [upstream provenance](upstream-notices.md) and [third-party notices](../../THIRD_PARTY_NOTICES.md).

Review the actual delivered artifact and preserve applicable notices and corresponding-source/relinking material before publishing binaries or containers. Source-only alpha checks do not qualify those distributions.
