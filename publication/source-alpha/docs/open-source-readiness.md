# Source alpha readiness

The source snapshot contains dual MIT OR Apache-2.0 first-party licenses, contributor/security guidance, pinned dependency locks and notices, synthetic quickstarts, and an ephemeral hosted community workflow. It excludes the private repository's history, qualification artifacts and screenshots. See [scope and limitations](alpha-status.md).

Before publishing, maintainers must review rights and confidentiality for these exact bytes, obtain completed clean-checkout checks, configure branch protection and fork approvals, remove persistent runner access or enforce trusted-workflow restrictions, and explicitly approve publication. This preparation changes no repository visibility and grants no publication clearance.

The [launch procedure](source-alpha-launch.md) stages an independently verified Git snapshot and describes the proposed check, review and token protections for the selected source repository.

Binary and container releases need a separate review of actual delivered dependencies and notices. See [publication review](security/publication-review.md). Production deployment also needs the operational qualification listed in [alpha scope](alpha-status.md).
