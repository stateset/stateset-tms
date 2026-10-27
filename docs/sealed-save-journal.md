# Encrypted browser save recovery

The subsequent [browser database upgrade](browser-journal-upgrades.md) prevents the two preceding journal clients from reopening recovery storage after its native version-2 upgrade. The encryption and qualification below describe the earlier encrypted-journal release.

New shared-workspace originals are encrypted before the browser stores them. IndexedDB record format **2** contains an encrypted envelope plus the random request key, local outcome/check count and browser generation. The original path, input, success message and recording time remain inside the envelope. Encryption keys stay on the web server. An independently authenticated tenant/operator must match before the server returns an original; organization and verified Clerk session also form part of that scope.

Reload, reopened-tab recovery, explicit original replay, server resolution, competing-tab claims and credential removal retain their [existing behavior](browser-save-journal.md). Restoration sends no business operation automatically. Checking an original can retrieve or complete that original operation; server resolution retains a completed result or durably stops an unapplied request.

## Envelope and authority

`POST /api/freight/save-journal` offers `seal` and `open` actions. It verifies the captured credential/JWT through the actual Freight access endpoint and requires staff workspace access. Tenant and subject come from that verified response. Organization comes from the verified gateway scope; the Clerk session comes from the exact signed token after the API validates its live session. The caller’s owner value must equal this server-derived scope. Neither a different operator in the same tenant nor another tenant can relabel a captured envelope to open it. Origin checks, no-store responses and bounded input apply to both actions.

The envelope uses Node’s [authenticated AES-GCM API](https://nodejs.org/docs/latest-v22.x/api/crypto.html): AES-256-GCM, a fresh random 12-byte nonce, a full 16-byte authentication tag, and associated data containing the protocol purpose, key ID and complete verified owner. Decryption returns no bytes until authentication succeeds. Key IDs are authenticated as well. The immutable original is encoded once; its client fingerprint is reconstructed from its exact path/body after opening. Client-side packet checks reject incomplete acknowledgements before any business request can start.

Encryption and opening happen outside IndexedDB transactions. Existing local transactions still serialize claims, check counts, decisions and generation/key comparisons. Legacy conversion uses a compare-and-swap of the complete previous record. A pending conversion registers its verified generation before waiting on HTTP, so sign-out can clear the old plaintext and a changed generation cannot reopen an old view. Local cleanup never needs the decryption service or key and cannot prevent credential removal.

These envelopes protect retained request contents. Visible owner identifiers, request keys, local outcome/check metadata and ciphertext length are not secret. Local metadata is not a server receipt or a durable stop. A profile writer can delete or damage a journal; an attacker controlling an authenticated browser can request its authorized plaintext. This does not protect against server-key compromise, active same-origin compromise or stolen valid credentials.

## Keys and deployment

Configure the optional **server-only** `FREIGHT_SAVE_JOURNAL_KEYS_JSON`:

```json
{
  "active": "your-current-key-id",
  "keys": { "your-current-key-id": "base64url-encoding-of-32-random-bytes" }
}
```

Replace that placeholder with a cryptographically random 32-byte key. One to ten named keys are supported. Never put this variable in a public browser variable or source-control a real key. The active entry seals new originals; retained entries open older ones. Every web replica must share the required keys. Compose forwards the setting; an empty optional setting means it is unset.

When the keyring is unset, a domain-separated key derives from the existing `FREIGHT_SESSION_SECRET`, using key ID `session-v1`. This preserves credential-mode deployments with a valid existing secret. Clerk deployments must provide the explicit keyring or a valid session secret. Missing or malformed production key configuration pauses new shared saves and opening, with the original retained. Changing a secret without retaining its old derived key can make existing originals unreadable.

For rotation, first distribute the old and new keys to every replica while retaining the old active ID, then switch the active ID. Keep the old key while any browser can hold unresolved originals. Do not reuse a key ID for different key bytes. There is no automatic journal expiry that makes deleting old keys safe. Back up keys separately under the deployment’s secret-custody policy; restoring business databases alone does not restore web encryption keys or browser data. No production custody or key-loss recovery procedure is qualified here.

The secret-free **synthetic demo only** uses a random process-lifetime key. Its key is lost on web-process restart or development module reload; configure a stable secret/keyring if demo recovery must survive that. Real workspaces and Clerk mode never use this fallback. No hardcoded demo encryption key is used.

Deploy compatible gateway and workspace assets together. This adds no Freight SQL migration: the backend remains format 18/migration 023. The preceding format-1 browser code cannot read format-2 contents. This earlier encryption release alone allowed old pages to reset records during cleanup and write plaintext afterward. The subsequent [native database version barrier](browser-journal-upgrades.md) closes that path after its upgrade commits; blocked older connections require a close/reload and explicit storage retry. Source reversal is an isolated reproduction aid, not a safe rollback procedure for unresolved browser journals.

## Existing plaintext and failures

A valid format-1 original converts only when its own verified operator opens it. Conversion keeps its original path/body/key, generation and outcome/check metadata, and replaces the plaintext only after sealing succeeds and the complete old record still matches. If encryption is unavailable, the old entry remains intact and new shared saves pause. A malformed old entry is retained for support rather than discarded. An unknown key or an invalid authentication tag also retains the encrypted bytes and blocks replay.

Other operators’ unopened format-1 records can still contain plaintext. Resolve, convert or deliberately clear those old records before treating an existing shared profile as protected. This change cannot retract earlier exports, profile copies, backups or already exposed plaintext. Clearing browser/site data, private-profile disposal or profile loss can still remove recovery information; successful local sign-out cleanup removes it without stopping an original already begun.

Embedded workflows, unsaved drafts, offline execution and cross-device recovery remain separate. The browser data is separate from the API’s durable receipts and stop decisions.

## Qualification

The Python verifier uses the separate Python [cryptography AES-GCM library](https://cryptography.io/en/44.0.3/hazmat/primitives/aead/#cryptography.hazmat.primitives.ciphers.aead.AESGCM) to open an actual captured envelope and reject another owner’s associated data. It also independently reads actual API customer records and scans public JavaScript for known fixture key/secret markers. Those fixtures contain no real customer data or credentials.

The Freight backend, codec wrapper and canonical EDI/Sync sources retain their preceding source-bound qualification when their bytes match. The service restore drill does not cover the new web keyring or physical browser-profile restoration. Live Clerk enrollment, real operator outcomes, deployment performance and the full platform evidence gates remain open.
