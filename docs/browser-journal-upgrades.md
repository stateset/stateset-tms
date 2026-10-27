# Browser recovery across workspace upgrades

An older open workspace could reset an encrypted recovery entry during local cleanup and then store another original as plaintext. The current workspace upgrades the native IndexedDB database to **version 2** before using recovery storage. Both previously shipped journal modules explicitly request database version 1, so the browser refuses their subsequent opens after the upgrade commits. Their cleanup operations are subject to the same barrier.

This native database version is separate from encrypted **record format 2** and from Freight's SQL format **18 / migration 023**. The encryption protocol, API, SQL schema and EDI/Sync contracts are unchanged.

## Preserving originals

The structural upgrade retains the existing out-of-line `owners` store and all of its records. It does not scan or rewrite other operators' entries. The authenticated owner subsequently follows the existing [encrypted journal conversion](sealed-save-journal.md): seal their valid plaintext original, then replace the complete matching record while preserving its original request key and browser generation. Existing ciphertext and unknown rows remain intact. Restoring recovery sends no business operation automatically.

A version-1 database with a missing `owners` store, an inline key path or automatic key generation is refused. Its upgrade transaction aborts and its previous records remain. A database already upgraded beyond version 2 is also refused, with a request to reload the workspace. Neither case silently recreates recovery storage.

The browser's versioned open, blocked connection and upgrade transaction rules provide this boundary; an open at a lower version fails with `VersionError`. New connections close when notified of another version change. See the [Indexed Database API recommendation](https://www.w3.org/TR/IndexedDB-2/#opening-a-database-connection). The concrete behavior below is qualified in Chromium.

## An older tab holds the database open

Short existing transactions can finish before the upgrade proceeds. If an old connection blocks the open for five seconds, the new workspace pauses shared saves and displays:

> Another tab is using older save recovery. Close or reload that tab, then retry recovery storage. Your original records are retained.

Close or reload the older tab, then choose **Retry recovery storage**. A timed-out opener aborts its upgrade if the browser admits it later; it cannot silently upgrade the database after the workspace has reported failure. The retry restores the original for explicit checking or resolution.

Deploy compatible gateway and browser assets together. Keep the server keys required by unresolved encrypted entries. Source reversal does not downgrade an upgraded browser database: restoring the old version-1 client makes that client's journal unavailable. Preserve compatible recovery access while resolving retained originals.

## Qualification and limits

The barrier applies after the structural upgrade commits, within the same browser origin and storage profile. It does not stop an already admitted business request, revoke a credential, erase unopened legacy plaintext or recover deleted browser data. A writer deliberately using the new database version or deleting the database is outside this old-client compatibility protection. Malicious authenticated clients, production key custody, live identity commissioning, other browser engines and measured operator outcomes remain separate qualification work. Unchanged backend and actual EDI/Sync evidence is reused with source fingerprints, not counted as freshly rerun tests.
