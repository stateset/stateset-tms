# Secret scanning

Use Gitleaks 8.30.1 with the included policy. The community workflow verifies the official archive's pinned SHA-256. Run the synthetic credential canaries and scan the snapshot:

```sh
python3 scripts/check-secret-policy.py
gitleaks dir --redact=100 --no-banner .
```

After creating a public repository, scan all fetched Git refs with `gitleaks git --log-opts='--all --full-history' --redact=100 --no-banner`. These scans do not inspect server-only refs, unreachable objects, external attachments, LFS bodies or account data. Do not commit raw scanner reports or credential values.

The policy retains built-in detectors and recognizes only enumerated SHA-256 fields in bounded evidence/catalog locations. Synthetic canaries require credential detection in root, evidence, source and catalog fixtures. No complete directory is exempt. Any scanner finding needs review; do not add broad exclusions to make a check pass.

Automated scans do not establish rights, confidentiality, fixture provenance or safety of binary visual contents. Those require maintainer review before publication.
