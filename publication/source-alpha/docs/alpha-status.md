# Source alpha scope

This is a reviewable source snapshot, not a production release. The `SOURCE_ALPHA.json` manifest binds every exported file other than itself, records input/output hashes for changes and explicitly leaves publication clearance false. It does not contain private Git objects, historical logs, screenshots, installed dependencies, compiled binaries or containers.

API, TUI, web business code, migrations, dependency locks and synthetic fixtures retain their original bytes. Documentation paragraphs referring to omitted development artifacts are removed, MCP guides are regenerated, and browser journal fixtures use small byte-binding receipts instead of private historical reports. Those receipts verify fixture bytes, not a historical execution result. Publication templates and the hosted workflow differ from the private development checkout; the manifest records those differences.

Technical guides can describe earlier implementations and measurements. Historical claims in those guides are background only: their original qualification material is deliberately absent. Use the contribution checks and completed CI runs for this source. Historical profiling/recovery harnesses may need separately supplied reference inputs or sibling repositories.

## Included document interface

The source includes authenticated, bounded PNG/JPEG/PDF page previews, original/source/page integrity checks, compressed Kitty transport with capability acknowledgement, direct Ghostty image lifecycle handling, explicit single-tmux Unicode placements and character/text fallback. Native Linux PDF workers require enforced filesystem/syscall isolation. Default and restricted/database-TLS Compose profiles use a separate credential-free, network-free renderer and a private Unix socket. The renderer adds its own resource budget; this snapshot establishes no combined-capacity guarantee.

The three document PDF fixtures are authored synthetic solid-color pages, text and a protected-document refusal example. They contain no real shipment evidence. The export preserves their exact bytes alongside the application, test and qualification sources. The exporter refuses a stale policy that omits tracked first-party Rust/SQL or Python source/test files; compiler and test execution still qualify the exported tree separately.

## Known qualification limits

Production capacity, cold console-projection rebuild latency, broad search/count cost, scaled queue-write latency, full PostgreSQL 16 coverage, production browser/container qualification, live identity and partner commissioning, independent dispatcher acceptance, and recovery/capacity objectives remain open. Default Rust tests ignore database cases. A green source check is not production acceptance.

The included workflow covers source/community checks on ephemeral hosted runners. Repository billing/access, branch protections, fork approvals and organization runner restrictions must be configured by maintainers. A queued or unstarted workflow is not a passing check.

## Publication and distributions

Rights and confidentiality review of the exact snapshot remains required before publication. The first-party license is MIT OR Apache-2.0; dependencies retain their own terms. A source-only scope does not grant clearance for bundled native dependencies, binaries or containers. See [third-party notices](../THIRD_PARTY_NOTICES.md) and [distribution review](security/publication-review.md#dependency-notice-bundle).

The committed RSA keys are deliberately public [synthetic signing fixtures](../tests/fixtures/identity/README.md). They must never be trusted by a live identity provider.
