# Preparing a source-only alpha candidate

`scripts/prepare-source-alpha.py` exports an immutable Git commit through an explicit [file allowlist and public document templates](../publication/source-alpha.json). It reads Git blobs rather than working-tree files: local `.env`, dependencies, build outputs and uncommitted edits cannot silently enter the candidate. Commit reviewed preparation changes before using it. Node.js 22 and Python 3.8+ are required for guide generation and packaging.

From a Git checkout:

```sh
freight_alpha_parent="$(mktemp -d)"
python3 scripts/prepare-source-alpha.py --output "$freight_alpha_parent/source" --archive "$freight_alpha_parent/source.tar.gz"
```

Both paths must be new and outside the input checkout. The directory and archive are private (0700/0600). Nothing is published, no Git history is copied or rewritten, and no remote is contacted. The archive has normalized ordering, modes, owners and timestamps, so identical input commits produce identical archives. The output manifest binds every exported file other than itself and records original/revised hashes and transformations. It leaves publication and distribution clearance false.

## Scope and transformations

The allowlist retains API/TUI/web/MCP source, all migrations and dependency locks, licenses and upstream materials, synthetic fixtures, optional contract-harness source and development scripts. It excludes historical evidence, screenshots, the old development README, qualification summaries, private release notes and the full persistent-runner workflow. New tracked files require an explicit allowlist review.

Public templates provide the README, contribution instructions, alpha limits and publication/security guidance. Whole Markdown prose blocks linking omitted development artifacts are removed; missing links to unknown files fail the export. Other historical implementation and measurement descriptions may remain as background, explicitly unqualified for this source snapshot. Review the resulting guides before publication. The existing MCP generator regenerates operation descriptions and guide content from the exported API documentation.

The browser journal module bytes remain unchanged. Their manifest is rebound to small per-fixture receipts containing only the existing fixture/source hash binding. These replace private historical execution reports; they are not equivalent historical evidence. Browser assertions continue checking each fixture's bytes and receipt. No business code, migration, dependency resolution or test assertion is edited by the export.

The included community workflow uses only an ephemeral GitHub-hosted runner and explicitly selects the source-alpha metadata policy. The full self-hosted workflow and historical scanner fingerprints are omitted. Automated scans and the allowlist are not confidentiality or rights clearance.

## Review and validation

Run `python3 scripts/check-open-source.py --source-alpha`, the publication/exporter unit checks, `npm --prefix mcp run check:catalog`, secret scanning and the appropriate Rust/web/MCP/terminal checks from the exported directory after installing its locked dependencies. Verify manifest hashes again after checks; local build outputs are outside the manifest and must not enter a publication archive. See the exported contribution guide for commands. Inspect the two synthetic RSA fixtures and their test-only documentation.

Keep the original private development repository separate. Before publishing this candidate, obtain rights/confidentiality clearance, completed remote checks, branch protection and actual fork/runner policy review. Source-only review does not clear native dependency redistribution in binaries/containers. See [readiness](open-source-readiness.md). An explicit publication decision is still required.

The [launch procedure](source-alpha-launch.md) verifies the reviewed archive and stages a fresh Git repository with one root commit and no remote. It also provides the exact proposed branch-protection and default-token payloads for the selected source repository.
