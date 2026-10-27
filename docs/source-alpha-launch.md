# Source alpha launch procedure

Use a separate repository for the reviewed source snapshot. The private development repository retains excluded historical material and must remain separate. Preparing a local repository does not publish it or grant rights/confidentiality clearance.

## Bind the reviewed artifact

Record the exact archive SHA-256, its `SOURCE_ALPHA.json` hash and the owner's review decision. Check the synthetic signing-key context, third-party terms and the intended public commit identity. An existing alpha verification report covers only its named source/archive; do not attach its passing results to a modified snapshot.

The staging tool verifies the complete archive pin, every file/hash/mode, exact membership and normalized archive metadata. It refuses paths outside the source root, Git metadata, symlinks/hardlinks, duplicate members or JSON fields, historical scanner fingerprints, private workflows, expanded data beyond its limit and hidden trailing payloads. It creates only a new local repository:

```sh
python3 scripts/stage-source-alpha.py \
  --archive /ABSOLUTE/PATH/TO/REVIEWED_SOURCE.tar.gz \
  --expected-sha256 REVIEWED_64_CHARACTER_SHA256 \
  --output /NEW/PRIVATE/STAGING_DIRECTORY \
  --report /NEW/PRIVATE/REVIEW.json \
  --author-name PUBLIC_COMMIT_NAME \
  --author-email PUBLIC_COMMIT_EMAIL
```

Use an explicit public identity. GitHub documents its [noreply address formats](https://docs.github.com/en/account-and-profile/reference/email-addresses-reference#your-noreply-email-address). Staging ignores ambient Git directory/configuration overrides, disables hooks and commit signing, and verifies one root commit, byte-identical tracked files, clean status and no remote. The report is created exclusively with mode 0600; the source directory is 0700. It never copies old Git objects, rewrites shared history, tags or pushes.

Run Gitleaks against the staging repository's full reachable history and run `python3 scripts/check-open-source.py --source-alpha`. Check `git rev-list --all --count` is 1, `git remote -v` is empty, and `git fsck --full --strict` succeeds. Build/test outputs are not publication inputs. The archive verifier is the authoritative file binding; review Git author/committer metadata separately.

## Configure the selected target

After the maintainer chooses the repository and approves a private remote bootstrap, verify it is empty and push only the staged `main` branch. Do not import or mirror refs from the private development repository. Set the repository's [Actions defaults](https://docs.github.com/en/rest/actions/permissions#set-default-workflow-permissions-for-a-repository) using [workflow-permissions.json](../publication/workflow-permissions.json): read-only default tokens and no automated approving reviews. Existing explicit workflow permissions still apply; defaults do not replace workflow review.

The proposed [branch-protection payload](../publication/branch-protection.json) uses GitHub's [branch protection API](https://docs.github.com/en/rest/branches/branch-protection#update-branch-protection). It requires the unique `Community source checks` context from GitHub Actions (app ID 15368), current-base checks, one approving reviewer, approval after the latest push and resolved conversations. It enforces these settings for administrators, requires linear history and disallows force pushes and deletions. It does not invent a CODEOWNERS team. Assign an actual second maintainer before enabling the review requirement.

Apply this payload to the selected source repository's `main` after verifying the exact check name and publisher there. Do not replace an existing repository's protections or team restrictions with a generic payload. Observe a completed successful run before publication. Current local results cannot repair account billing, create an authorized runner or establish the target's remote result.

The public workflow uses ephemeral hosted runners. Before accepting fork execution, remove persistent runner access or enforce organization-level restrictions to trusted workflows; an incoming PR can edit a workflow. Verify the actual repository/organization policy and fork approval settings.

## Publish only after the remaining decisions

The maintainer confirms first-party contribution rights, confidentiality of the exact source and public commit metadata, the security/support contact and alpha scope. Review the staged tree, completed CI and applied protections, then explicitly authorize publication of that selected repository. This procedure never changes visibility automatically. Publishing binaries or containers needs a separate review of the exact dependency distribution; this source-only launch does not clear them.
