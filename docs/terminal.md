# Freight terminal console

The native Rust `freight-tui` console runs inside Ghostty with five keyboard-driven workspaces: a shipment catalog, actionable operations issues, customer billing, carrier payables, and inbound/outbound integration queues. Inspect a selected record alongside the table in wide windows, or press Enter for scrollable details and use left/right to browse adjacent records. Operations staff can review and submit location updates, report exceptions and resolve exceptions. In Operations they can claim/release work, record findings, change follow-ups, and defer/resume owned work. Dispatch, approvals, bank actions and queue retries remain in the existing web/API workflows; financial views remain read-only.

## Start in Ghostty

From this checkout, run:

```bash
./scripts/freight-tui --demo
```

The launcher builds the current Rust source when needed. Demo mode is entirely local and always marked **DEMO · SYNTHETIC DATA**. Its 65 sample shipments exercise both pages, search and status filters; its billing and queue records are synthetic. The demo operator can practice the three shipment actions. Simulated changes and original-request receipts remain in memory for the process lifetime. It never connects to a server or uses a configured credential.

For a standalone binary:

```bash
cargo build --locked --release -p stateset-freight-tui
./target/release/freight-tui --demo
```

The console uses Crossterm's terminal sequences, including alternate-screen entry, bracketed paste, resize events, and RGB colors. It does not invoke ncurses or require a terminfo lookup, patched fonts, Ghostty configuration changes, or replacing `TERM=xterm-ghostty`. Standard Unicode characters work with Ghostty's regular fonts. `--light` selects a light palette; `--no-color` or `NO_COLOR=1` disables colors. If other tools on a remote host report missing Ghostty terminfo, follow [Ghostty's terminfo guide](https://ghostty.org/docs/help/terminfo); the console itself does not need that database.

## Connect to the Freight API

Start with `./scripts/freight-tui --connect` for guided setup. Enter the API origin, paste a staff credential into the hidden field, and optionally paste the **same retained recovery key** into the hidden recovery field. Review the API destination and press `y` to connect. Esc cancels without sending a request; a pasted or repeated `y` cannot confirm. Existing credentials are never reused after changing the API origin unless a new credential is entered. No credential or key file is written by setup. Starting interactively without a configured token also opens setup. `--snapshot` always requires an environment credential and never prompts.

Set the API origin and a staff credential in the environment before launching. A viewer credential suffices for browsing; shipment actions require `operations.write` from an operations or administrator identity. For example, the explicitly enabled local API demo (`FREIGHT_DEMO=true`) accepts this synthetic credential:

```bash
export FREIGHT_TUI_API_URL=http://127.0.0.1:8090
export FREIGHT_TUI_TOKEN=freight-local-demo
./scripts/freight-tui
```

For your deployed workspace, use its HTTPS API origin and your authorized staff service credential or current Clerk JWT. Set `FREIGHT_TUI_ORGANIZATION_ID` when using a Clerk organization. The API derives tenant and permissions from the credential. Credentials are accepted through the environment or hidden setup fields, never CLI flags or saved configuration. Restart with `--connect` to change a credential. Remote HTTP origins, embedded URL credentials, URL paths/queries/fragments and redirects are refused.

Each fetch verifies `/v1/access`. Loads use `/v1/loads?limit=50` with server-side search/status and scoped cursors. Operations use `/v1/operations?limit=50` with server-side search and the default actionable view. When `/v1/access` advertises `features.console_catalog: true`, Billing, Payables and Queues use `/v1/console?limit=50&view=…`. Search and balance/attention/direction filters apply across all matching records before paging. The status line shows the matching count and current page; financial cards labeled “All” always cover the entire workspace. These reads omit unrelated commercial/customer and queue payload/authentication data. With migration 030, warm PostgreSQL pages read the console index without decoding the full workspace. The first read after a source write, cache loss or UTC date rollover rebuilds it under normal workspace admission. Counts, identity digests and substring search still scan matching projected candidates; production capacity remains unqualified. Older APIs and the small synthetic demo retain `/v1/workspace?history=deferred` snapshot/local-filter behavior. Missing amounts display as unknown, not zero; signed amounts use integer cents.

Each workspace remembers its applied search, shipment status filter, server page, selected record and table position for the current session. Switching back performs a fresh read before restoring the selected record by identity, including if the results have reordered. If the record disappeared, selection moves to the first fresh result. Selecting the already active workspace keeps the current snapshot. `r` returns only the active workspace to its first page; its search and status filter remain. An expired server cursor still requires `r` to restart. Navigation memory contains no cached read snapshots, stays in process memory, and is cleared on access denial or a tenant/operator change. An identity change discards the response requested with the previous context and reads again without its filters.

From an operations issue, bill, payable or outbound queue record, press `o` to open its linked shipment. The shortcut also works inside details. It replaces the Loads search/status/page with the record's validated shipment ID and performs a fresh bounded catalog read; only one exact raw ID match can be displayed or reviewed for an action. Missing, malformed, duplicate or substring matches cannot select another shipment. An inbox document ID is never treated as a shipment link. Records without an actual shipment ID explain that no supported link is available.

Press `b` from the linked shipment or its details to return to the source workspace with its search, cursor and selected record retained, followed by a fresh read. The source is visible before the linked ID, including at compact widths. Catalog search is still limited to 50 rows per response; if the exact shipment is not on that page and another cursor is available, `n` explicitly checks the next page. The shortcut never scans additional pages automatically. `r` retries the first page. Backspace clears the link and filters; applying a changed search also resumes normal Loads browsing. Access denial or a tenant/operator change clears related navigation. Opening/returning is read-only and preserves any unresolved original save; shipment actions still require their normal fresh-data, role and explicit review checks.

Refresh is manual by default. Snapshots become visibly **STALE** after 60 seconds, and a stale snapshot cannot open a new shipment action. `--stale-after 10..3600` adjusts that threshold. Press `R` to toggle automatic reads every 30 seconds, or use `--refresh 5..3600` (0 disables it). Automatic reads preserve the current page, filters and selected record. They pause during searches, action drafts, details/help/status inspection, pending saves, access denial and errors. A failed scheduled read requires explicit `r` recovery; no command is ever scheduled or retried by this timer. Requests use a bounded worker, connection/request timeouts, and a 32 MiB response limit. Navigation and exit remain responsive during requests. A transport failure preserves a marked older snapshot. Access denial clears displayed records, identity and finance totals and requires an explicit retry. Responses from a previously selected view cannot replace the current view. API error bodies are not displayed, and text is stripped of terminal controls before rendering. The console never writes API credentials or read snapshots to disk. Live commands retain only their encrypted recovery envelope until the original is resolved and a fresh same-owner read succeeds.

In Billing and Payables, `f` cycles all balances, open balances, credits, zero balances and unknown balances. Billing includes overdue invoices between open balances and refunds due; Payables skips aging because its snapshot has no overdue-days field. Classification uses the raw signed integer-cent balance: positive is open, negative is a customer refund due or carrier credit, and zero means a zero balance regardless of document status. Missing, non-integer or out-of-range amounts remain unknown, never zero. Overdue invoices require both a positive balance and a positive integer overdue-days value from the API; the terminal does not infer aging from due-date text or its local clock.

Balance filters combine with text search without fetching. Each financial workspace retains its own filter and selected record through workspace switches, shipment links and refreshed reads. Backspace clears its balance filter and search together. Filter names appear before long queries, and compact footers show the controls. Finance cards remain the API's workspace totals; they do not become subtotals of the filtered rows. These controls perform no payment, refund, settlement or recovery write.

In Queues, `f` toggles attention items (`retry`, `held` and `failed`), and `d` cycles all directions, outbound and inbound. These filters combine with text search. With console catalog support, changing them starts a fresh first-page server read; older APIs and the small synthetic demo filter their fetched snapshot locally. Completed, externally reviewed, sending/processing and unrecognized states remain available under all states; an old error or unknown transmission alone does not classify a completed record as needing attention. The filter summary appears before the search text at compact widths. Backspace clears direction, attention and search together. Workspace switching, linked-shipment return and read refreshes retain the filters and selected record. Inbound and outbound identities are kept distinct even if their raw IDs match.

Queue details show direction, attempts in the current cycle, total attempts, recorded transmission, next attempt, lease deadline and original error when those values are available. Missing counts remain unknown. Attention items point to the web workspace's Integrations view for recovery. Queue filters and details cannot retry or resolve a handoff; payloads, upstream response bodies, callback authentication and lease tokens are not added to the inspector. Existing unresolved terminal saves retain their separate recovery controls and immutable original request.

## Keep your place between pages

All five workspaces remember the selected record and table position for each visited server page when their API supports pagination. Loads and Operations always use server pages; Billing, Payables and Queues require the console catalog feature marker. Press `p` to return to the previous page, or `n` to revisit a forward page offered by its fresh response. Ctrl+P also provides **Next server page** and **Previous server page**, with unavailable choices disabled. Each move clears the displayed rows and requests fresh data; memory holds only cursor, record identity and table position, never read snapshots or reviewed commands.

Returning restores selection by identity if rows reorder. If the record disappeared, the first fresh result is selected. Resizing adjusts the viewport around that selection. A changed next cursor discards positions from its old forward branch; remembered pages never supply a cursor the current API response did not offer. No page is prefetched or scanned automatically.

Page positions survive workspace switching, linked-shipment return, scheduled reads and Ctrl+R. A failed return keeps its saved row anchor for an explicit Ctrl+R retry. `r` restarts page one and clears page-position history; applying a changed search, status or work scope, clearing filters, and changing tenant/operator also clear it. Unchanged or canceled search keeps the history. An API upgrade or rollback that changes console catalog support clears cached financial/queue cursors while retaining their filters and private original-save recovery. Page browsing preserves uncertain original saves and retained stop decisions, and performs no write. This memory lasts only for the current process.

## Retry without losing your place

Press Ctrl+R from the main table to reread the current page. It keeps the server cursor, search, shipment status, operations scope, local balance/queue filters, linked-shipment context and selected record. A reordered successful result restores selection by identity; a removed record falls back to the first result. It also preserves the saved selection anchor when a workspace return failed before its first row arrived. A failed read keeps the previous snapshot marked stale and pauses automatic retry until an explicit read succeeds. Reads recheck API access and send no shipment or recovery command.

Use `r` to restart the first page when a server cursor expires or the queue changes. Ctrl+P provides separate **Refresh current page** and **Refresh from first page** choices. Current-page requests already queued or running are coalesced, and held/repeated Ctrl+R key events cannot queue extra reads. Ctrl+R does not leave a search field, details/help panel or action review; close that panel first. Command palette text remains text.

The connection panel (`i`) shows the read target, page and whether a read is idle, requested or running. Within that panel, Ctrl+R rereads the current page, `r` starts from page one, and `R` toggles automatic reads; all keep the panel open. Current-page refresh preserves its scroll position. Automatic reads remain paused while the panel is open. Access denial or changed tenant/operator still clears private navigation, and reads preserve any uncertain original save and its retained stop decision. They never check, resend, stop or discard it.

## Operations work views

In Operations, `f` cycles Actionable, All active, Assigned to me, Unowned, Deferred, Source inactive and All issues. Ctrl+P also selects a work view directly, for example `work mine` or `work deferred`. These are the API's queue views over the tenant's work, rather than filters limited to the displayed 50 rows. Assigned to me uses the verified API principal's subject; the terminal sends no owner or subject override. Active, assigned and unowned views can include deferred work; Actionable excludes deferred work. Source inactive corresponds to the API's `resolved` view and means the source condition is no longer active, even if retained human work still has a claimed status.

Changing work view keeps the text search, clears the old cursor and rows, and requests the first bounded page. Old-view replies cannot replace it. `n`/`p` retain this scope; `r` restarts its first page. Switching workspaces or following a related shipment and returning retains scope, query, page and selection with a fresh read. Backspace restores Actionable and clears search. Access denial and tenant/operator changes clear active and cached scopes. Navigation preserves any uncertain original save and retained stop decision.

The table labels the source deadline explicitly. Enter shows the source deadline separately from the work case's scheduled follow-up, overdue flag and deferral deadline, plus source-active/changed, evidence validity, work status and work version. Unknown values remain unknown. Deferred and inactive source records retain their original priority without being counted as current attention items. Browsing these views does not claim, defer, resume or resolve human work, and does not change shipment truth.

For a scoped plain snapshot, run `./scripts/freight-tui --view operations --operations-scope mine --snapshot`. Accepted scope codes are `actionable` (default), `active`, `mine`, `unowned`, `deferred`, `resolved` (source inactive), and `all`; the flag requires `--view operations`. Plain output includes the selected scope. Synthetic demo data exercises all seven scopes, including deferred work assigned to its terminal operator and a retained claimed case whose source is inactive; its default three actionable rows keep their existing order.

## Act on Operations work

Press `a` on an Operations record, or choose **Review selected actions** in Ctrl+P. The menu offers claim for yourself, release your ownership, add findings, change your follow-up, defer your follow-up, and resume deferred work. Only applicable choices are enabled. Claiming another person's work requires their release followed by your own fresh claim; the console has no direct owner override. Critical source holds cannot be deferred. An unstable or untrusted source cannot open a work review.

Claims and follow-up changes require a future time within seven days. Deferral must be within 24 hours and no later than the existing follow-up. Enter `30m`, `2h`, `1d`, or an RFC3339 time with an explicit `Z` or offset. Enter pins the absolute UTC time, then asks for actual findings or coordination (1–200 UTF-8 bytes). The final review shows issue/version, authenticated actor, current owner, source activity/priority, source hash, exact UTC time and note. Only an unmodified, non-repeated `y` can confirm while the complete review fits. A time that expires before confirmation is refused. Esc discards the unsent draft.

These actions retain the exact body, source hash, reviewed version, tenant/subject and request key through the same encrypted write-ahead recovery as shipment actions. A lost response does not schedule another command. `c` checks the exact original and `x` reviews resolving it, retaining its result if completed or preventing later application. A stop decision survives restart. Source state/version/ownership remain checked by the API; work actions cannot manufacture shipment progress, clear a source hold or change financial truth. The API must advertise `features.operations_save_recovery: true` from `/v1/access`. Older APIs keep work read-only. Access is rechecked before each work submission or recovery request; missing feature support sends no work command.

A confirmed work action reads fresh data in the current work view. A claimed item leaves Unowned; released work leaves Assigned to me; deferred work leaves Actionable and remains available in Deferred or Assigned to me. Change scope with `f` or Ctrl+P to find it again. Records are not kept visible using an older snapshot. The current session's other workspace filters remain intact.

## Find commands

Press Ctrl+P from the main table to search navigation commands. Type words in any order (for example, `go invoice`, `status transit` or `balance overdue`); matching ignores case and includes workspace aliases such as shipments, queues and settlements. Up/down select a result, Home/End reach either end, and Page Up/Down move eight entries. Enter opens the selected command; Esc or Ctrl+P cancels. Active choices are marked, and unavailable choices stay visible but cannot execute. Choosing the active workspace or filter preserves its snapshot, selected row and viewport.

The palette can switch workspaces, open search/details/help/connection, refresh, toggle scheduled reads, clear filters, follow a related shipment or return, and directly select this workspace's status, balance or queue filters. Its commands use the existing navigation rules: server filters start a fresh first-page read; balance/queue filters reuse their snapshot only with older APIs or the small synthetic demo. Availability is checked again when Enter is pressed. Shipment actions and original-save check/stop commands keep their existing reviewed shortcuts outside the palette.

The command search is separate from the workspace search, stays in memory, accepts up to 200 UTF-8 bytes, strips terminal controls, and removes whole graphemes with Backspace. Ctrl+U clears it. Long queries keep their caret visible, pasted text cannot execute a command, and a repeated Enter cannot choose one. The palette pauses scheduled reads, cannot replace an active editor/inspector/review, and closes on access denial or a tenant/operator change. Ctrl+C retains the normal unresolved-save warning. Opening, searching and canceling preserve the workspace and any uncertain original save.

## Reviewed shipment actions

On the Loads view, select a shipment and press `a`. Choose `1` for an actual reported location (in-transit loads), `2` for an actual exception note (open loads), or `3` to resolve an existing exception. Location and exception input are limited to 200 UTF-8 bytes, matching the API contract. Enter opens a review; only an explicit `y` submits it. Esc discards an unsent draft. Key repeats and bracketed paste cannot confirm a save.

The review retains the exact shipment identity, version and verified tenant/subject that opened it. Later reads cannot silently rebase that decision. The full review must fit onscreen; confirmation is disabled below 72 columns × 22 rows or when wrapped content overflows the dialog. Immediately before sending, the client checks current staff identity and capabilities again. The API applies its existing tenant, role, shipment-state and expected-version checks to `POST /v1/loads/{id}/commands`.

The command receives a random idempotency key and one immutable serialized body. A valid matching acknowledgement confirms the original save; only a read follows. A failed refresh cannot cause another command submission. An ordinary initial structured API rejection requires a new review after refreshing. Missing/unreadable acknowledgements and server failures leave the original outcome uncertain; neither background reads nor failed checks clear that uncertainty. New terminal writes remain blocked until the original is confirmed or stopped.

Press `c` to check the exact original command with the same path, bytes and key. This may complete the original if it had not reached the server. Alternatively, press `x`, enter a resolution note, review, and confirm with `y`: `/v1/save-recovery/resolve` returns the original result if committed, or records a durable stop preventing later application. Stopping does not undo a completed command. If a stop response is lost, `c` retries that same resolution and note; it never changes back into a business-command retry. `x` then reviews the retained stop decision rather than editing it.

### Encrypted recovery across restarts

Live writes require `FREIGHT_TUI_RECOVERY_KEY` or the recovery key entered in setup: a random 32-byte key encoded as 64 hexadecimal characters. Generate it once with `openssl rand -hex 32`, retain it in your approved secret storage, and provide that same key through the environment or hidden setup field on every launch. Do not generate a different key each time. Do not place the key in the recovery directory, commit it, or share a shell command containing its value. Browsing and offline demo mode work without this key; live saves fail closed if the key or safe storage is unavailable.

`FREIGHT_TUI_RECOVERY_DIR` optionally selects an absolute directory. The default is `$XDG_STATE_HOME/stateset-freight`, or `$HOME/.local/state/stateset-freight`. Newly created directories are private (0700); recovery and lock files are 0600. Existing directories must have those permissions. Symlinks, shared or unsafe writable directories, non-regular files and multiply linked files are refused. Use a local filesystem supporting advisory locks, atomic rename and file/directory synchronization; network filesystem semantics are not qualified. Linux and macOS are the intended hosts; validation here was on Linux.

Before a command can leave the worker, the original owner, reviewed shipment, serialized body and idempotency key are sealed with AES-256-GCM, a fresh random 96-bit nonce, and authenticated version/API-origin/tenant/subject context. The encrypted file is synced, atomically replaced and its directory synced. Only then may the API request begin. An owner-scoped process lock prevents two terminals from replacing the same journal. A competing terminal can still browse. Closing a journal explicitly unlocks its file even if a duplicated descriptor remains open; an active replacement owner retains its separate lock. A stop decision and its exact note are synced before sending the resolver request; after a restart it cannot silently become a business-command retry.

Reopen with the same API origin, verified operator, recovery directory and key. A successful API read verifies identity before exposing the recovered command. Restoration **never sends a command automatically**. An unresolved original offers `c` to check the same bytes/key or `x` to review stopping it. A recorded stop decision offers only that same resolution. A confirmed, stopped or initially rejected outcome is recorded as final before presentation; after a successful same-owner read, the journal is durably removed. If recording the final outcome fails, new writes remain blocked and the encrypted original remains available for resolution. A failed read cannot trigger another POST or discard the original.

Access denial clears private command context from the display and releases the process lock; its encrypted file remains for the verified original operator. Different origins and tenant/subject identities use distinct files and authenticated encryption contexts. Wrong keys, malformed records and authentication failures preserve the file and block writes. Keep both the directory and its original key when resolving a storage problem. Deleting files, changing the directory, rolling back state or losing the key can lose local recovery; encryption does not protect against a malicious process already running as your operating-system user. Key rotation and cross-device migration are not implemented; resolve all outstanding journals before changing keys. The cryptographic primitive uses [ring’s AEAD implementation](https://docs.rs/ring/0.17.14/ring/aead/index.html); persistence uses [Rust file locking and synchronization](https://doc.rust-lang.org/std/fs/struct.File.html) with descriptor-relative filesystem operations.

`q` or Ctrl+C warns when an original remains unresolved; Esc stays and uppercase `Q` exits. Live commands have write-ahead recovery, including after a crash or signal; the synthetic demo remains memory-only. Terminal cleanup cannot run after SIGKILL.

Compact tables use two-line rows instead of dropping the final columns: loads retain origin/destination and carrier, and financial rows retain both amount/total and balance. Amount columns reserve room for the full signed 64-bit money range, including in 52-column windows. The table title shows the selected position within the current filtered page. Compact footer hints fit at 52 columns and follow the workspace, search editor or unresolved original save. Long descriptive fields can still be clipped; Enter opens the full record. Press `i` for a scrollable connection/recovery panel showing identity, freshness, read scheduling, complete storage errors and the latest save message. It never displays credentials or keys. Help also scrolls with up/down.

Expanded details, help and connection panels show the visible line range and total wrapped lines. Up/down scroll one line; Page Up/Down move one viewport with one line of overlap; Home/End (or `g`/`G`) reach the first/last content. Scrolling stops at the content boundary and is recalculated when the window resizes. In details, left/right (or `h`/`l`) inspect the previous/next record within the current filtered page, reset the new record to its top, and keep the inspected record selected after closing. These keys do not fetch another server page or prepare a write. At the first or last record they keep the current record and scroll position.

In Loads, open Ctrl+P and search **selected customer** or **selected carrier** to find shipments with that record's exact party ID across the server catalog. These filters combine with search and status, restart at page one, and persist through reads and workspace changes. A missing, oversized or terminal-unsafe ID disables the corresponding choice; display text is never substituted for the raw ID. This selects a party from an existing record, rather than browsing a customer/carrier directory.

The header shows `[customer]`, `[carrier]` or `[customer + carrier]`; `i` shows the complete active IDs. The palette's **clear customer filter** and **clear carrier filter** commands remove either restriction independently, including after empty results. Backspace clears both with the other Loads filters. Opening a related shipment clears old party restrictions so they cannot hide the linked record. Identity changes and access denial clear remembered party scopes. These commands only read data and preserve unresolved saves.

## Keyboard

| Key | Action |
| --- | --- |
| Ctrl+P in the table | Search navigation commands; Enter chooses, Esc cancels |
| `1`–`5`, Tab, Shift+Tab | Switch workspace, retaining its search, page and selection |
| Arrows, `j` / `k` | Select a row |
| Home / End, `g` / `G` | First / last row |
| Page Up / Page Down | Move ten rows |
| `/` | Search; Enter applies, Esc cancels |
| Left / Right, Home / End in search | Move the edit caret by grapheme or to either end |
| Backspace / Delete in search | Remove the previous / next complete grapheme |
| Ctrl+U in search | Clear the draft without applying it |
| Backspace in the table | Clear this workspace's search, shipment status, operations scope, balance and queue filters |
| `s` | Cycle shipment status; filters combine with search |
| `f` in Operations | Cycle API work views, retaining search and restarting the first page |
| `f` in Billing / Payables | Cycle balance filters; Billing also includes overdue invoices |
| `f` in Queues | Toggle attention items (`retry`, `held`, `failed`) |
| `d` in Queues | Cycle all / outbound / inbound; combines with attention and search |
| `n` / `p` | Next / previous server page, restoring its selected row after a fresh read |
| Ctrl+R | Reread the current page, retaining cursor and selection; also works in the connection panel |
| `r` | Refresh from the first page; also works in the connection panel |
| `R` | Toggle automatic reads every 30 seconds; also works in the connection panel |
| `i` | Inspect connection, freshness and recovery status |
| `o` on a linked record or its details | Open the exact related shipment with a fresh read |
| `b` on a linked shipment or its details | Return to the source workspace and selected record |
| `a` | Choose and review a shipment action |
| `c` / `x` | Check / stop an uncertain original save |
| `y` | Confirm a fully visible action or stop review |
| Enter | Expand selected record; up/down scroll |
| Left / Right, `h` / `l` in details | Inspect previous / next record on the filtered page |
| Home / End, `g` / `G` in a panel | First / last content |
| Page Up / Page Down in a panel | Scroll one viewport with one line of overlap |
| Esc | Close details/help, or quit |
| `?` | Keyboard help |
| `q`, Ctrl+C | Quit, or warn when a save remains unresolved |

Search is bounded to 200 UTF-8 bytes. The search editor supports insertion at the caret, grapheme-aware movement/deletion (including combined accents and joined emoji), and Ctrl+U to clear the draft. Its horizontal window keeps the caret visible in compact terminals; `‹`/`›` mark hidden query text and the footer shows bytes used. Typed and pasted text use the same terminal-control filtering. Paste stops before a complete grapheme that would exceed the byte limit. Editing sends no request; Enter applies a changed search, while Esc or applying the unchanged query keeps the current page and selection.

Backspace in the main table clears its applied search, shipment status and customer/carrier filters together. Paged views return to the first page with a fresh read; local financial/queue filters reuse their snapshot. Empty results explain this shortcut, active status appears before query text, and other workspaces retain their filters. Clearing filters never checks, resends or discards an uncertain original save. Within a search or action draft, Backspace keeps its normal editing meaning.

Bracketed paste is accepted only in search and action text fields; a pasted shortcut cannot quit or confirm. Windows smaller than 52 columns × 16 rows show a resize hint and remain responsive. Terminal modes are restored on normal exit, Ctrl+C, SIGTERM/SIGHUP, and Rust panic; SIGKILL cannot run cleanup. Live save recovery is synced before transmission; synthetic demo recovery lasts only for the process.

`./scripts/freight-tui --demo --snapshot` prints the first load page without terminal escapes, including when piped. Use `--snapshot` without `--demo` to read the actual API. Interactive mode refuses non-terminal input/output with an explanation.

Use `--view loads|operations|billing|payables|integrations` to open a workspace directly. For example, `./scripts/freight-tui --view payables --snapshot` prints the current API's prepared settlements and signed balances.

## Verification

```bash
cargo test --locked -p stateset-freight-tui
cargo clippy --locked -p stateset-freight-tui --all-targets -- -D warnings
cargo build --locked -p stateset-freight-tui
python3 scripts/check-tui-terminal.py target/debug/freight-tui
```

The standard `scripts/check.sh` includes the terminal tests. Unit/component tests cover real HTTP fixtures for reads, writes and resolution; role/identity/version boundaries; exact original replay; stopped originals; lost stop responses; denied access; delayed reads; signed money; search/paging; and rendering at five sizes in three palettes. The PTY check runs the actual binary with `TERM=xterm-ghostty`, navigates views, resizes, checks bracketed paste, performs a simulated location save and reads its changed details, and verifies terminal restoration after keyboard exit, Ctrl+C and SIGTERM. These are terminal protocol tests, not a claim of a visual test inside a running Ghostty window or live identity commissioning.

Console index deployment requires migration 030 and refreshed runtime grants. Its version-2 cursors are intentionally incompatible with earlier tokens; press `r` to start at page one after a refused old cursor. Financial totals remain workspace-wide, and cold rebuilding never applies a business command.

## Cold console rebuilds

Cold rebuilds retain normal read, workspace and tenant-write admission. The builder locks the original tenant row and rechecks cache readiness before transferring original blocks. A process that waited while another builder published can use that cache without fetching and decoding the originals again. A genuinely invalid cache still reconstructs and validates the complete workspace, then publishes all three console views in one transaction. This preserves the existing schema, business format and running-writer compatibility.

Publication sends batches of at most 64 rows using typed PostgreSQL arrays. Summaries and variable-length search arrays retain JSON encoding; scalar metadata no longer makes a second complete JSON value tree. Exact cents, literal search values, private-field omission and atomic publication retain the same contracts.

For phase diagnostics, enable only `stateset_freight::store::console_store=debug` on the API. Events contain elapsed microseconds and the projected row count, with no tenant IDs, record IDs, payloads or credentials. `console_snapshot` includes the header-lock time reported separately by `console_header_lock`; do not add those overlapping durations. Decode, projection and publication durations are wall time, including scheduling or database waits, rather than CPU measurements.

The mixed-workload regression can print these events using an owned disposable database:

```sh
FREIGHT_PROFILE_CONSOLE=1 TEST_DATABASE_URL=... \
  cargo test --locked --test scaled_console_queues -- --ignored --nocapture
```

It retains 5,000 records in each of bills, settlements, outbound and inbound queues, verifies complete totals and bounded pages, and compares original physical records before and after cache construction. The fixture has a predeclared 30-second ingestion budget. Its timings establish neither sustained production capacity nor a latency service-level objective.

The opt-in `api_process` test uses a memory API by default. It prepares an issued bill, a settlement and a pending tender through actual API commands in the isolated synthetic demo, then checks their rows through both staff and viewer terminal reads. External integrations remain disabled. This fixture does not establish production financial approval or carrier acceptance.

Set `FREIGHT_TUI_TEST_DATABASE_URL` to a fresh disposable PostgreSQL database to exercise the same real terminal commands, nonempty console indexes and encrypted cleanup against PostgreSQL. The test explicitly forwards only this database setting to its owned API process; it does not inherit an ambient `DATABASE_URL`. The integration CI job provisions a separate `freight_tui` database for this mode. Repeating the same process case on both backends does not create two distinct test cases.

## Inline shipment evidence in Ghostty and Kitty

Press **v** on a shipment or its expanded details to open its stored POD, BOL and other evidence. Left/right or j/k selects documents; **g** toggles image/text; **r** rereads the catalog; **Esc** returns. The viewer displays filename, document kind, review status, byte count and the original SHA256. It shows how many of the total documents are in the newest-100 catalog. Previewing is read only. Review and original download remain in the web workspace. The synthetic demo includes a clearly labeled paper specimen for exercising the graphics path.

PNG, JPEG and PDF evidence use the authenticated server preview endpoint. PDF files open at page one; **Page Up/Page Down** selects pages and the metadata displays the current page and page count. Left/right still selects documents and resets the page. A page request clears the previous image immediately; obsolete replies cannot replace the current page. Corrupt, oversized or unsupported images leave metadata and navigation usable. Each fetch verifies the current staff tenant/operator through `/v1/access`; a changed identity or any access denial clears private records and images. Closed/replaced viewer requests cannot restore old evidence. Reads use the existing bounded worker and timeouts, so navigation and exit stay available during a server request. Auto-refresh pauses while inspecting evidence. Images and document catalogs are held in process memory and never written to disk.

The Linux API runtime needs Poppler (`pdfinfo` and `pdftoppm`) and util-linux (`prlimit` and `setpriv`) at their standard `/usr/bin` paths; the production Dockerfile includes them and DejaVu fonts. Native Debian/Ubuntu installations can install `poppler-utils util-linux fonts-dejavu-core`. Missing renderer tools return 503; invalid, encrypted, out-of-range and over-limit PDFs return 422. Originals remain downloadable and previews never approve evidence. The new `page` query defaults to one; responses bind `X-Preview-Page` and `X-Preview-Pages` alongside the existing original/document/load and RGB hashes. Image-only older servers remain compatible; PDF replies require explicit matching pagination.

PDF parsing runs in a separate process under the API service user, with an empty inherited environment, no shell, stdin input, stdout output and no client-selected paths. Each metadata/render process has a five-second wall limit, four CPU seconds, 256 MiB address-space limit, 32 file descriptors, an 8 MiB stack, zero-byte file/core size limits and no privilege gain. Metadata is capped at 32 KiB and rendered PNG at 4 MiB before the existing bounded 960-pixel RGB conversion. Files are limited to 1,000 pages and the existing upload size. One decoder permit remains owned until the work finishes or the child is killed and reaped, including cancelled HTTP requests; overload returns 503 without queuing. These limits do not create a filesystem/network namespace sandbox. Keep the container unprivileged and Poppler packages patched as part of runtime maintenance. The [upstream resource-limit command](https://man7.org/linux/man-pages/man1/prlimit.1.html) documents the enforced limits.

`--graphics auto` (default) sends one asynchronous, non-storing Kitty query for the exact zlib-compressed RGB direct transport used by previews. A matching success reply within 750ms enables graphics, including when terminal names are unrecognized. Missing, late, malformed or negative replies retain the character preview. Browsing and input continue during negotiation. Auto mode does not query when TMUX/ZELLIJ/STY is set, when TERM identifies tmux/screen, or when `TERM=dumb`; those paths retain the character preview. `--graphics kitty` explicitly requests direct protocol output without querying; use it only when the terminal path forwards Kitty APC sequences. `--graphics kitty-tmux` selects the explicit single-layer tmux path described below; Freight does not change multiplexer settings. `--graphics off` emits no graphics query or image commands; `--no-color` with graphics off preserves metadata without colored image cells. `--snapshot` stays plain text and does not query or fetch images.

The query contains one synthetic black pixel and the console's random image ID; it contains no document or credential. Complete query replies, including late replies, are consumed before application key handling. The client retains normal keyboard/paste/resize input in order and draws between ordinary input events. A partial Esc/Alt-_ prefix waits at most 60ms before ordinary input is released. Reply text is capped at 256 bytes; an unterminated frame expires after one second so it cannot hold keyboard capture indefinitely. This is protocol framing, not an authentication boundary against a terminal that controls keyboard input. Interactive cleanup uses the negotiated state, so fallback paths never delete an image or enable synchronized graphics updates.

The client checks media type, dimensions (1–960 per side), exact streamed byte count, original/source identity and SHA256 before rendering. It compresses verified RGB pixels on the API worker and sends zlib data directly in base64 chunks of at most 4096 characters. The terminal client uses no image files, shared-memory objects, subprocess renderers or external image URLs; PDF conversion stays on the API server. A single random image ID is owned by this console; only that ID is deleted. Repaints reuse transferred pixels, viewport resizing reconstructs the compressed image after the terminal clear (Ghostty can discard its backing image on clear), and hiding/closing/resizing below the minimum viewport deletes the placement. Normal exit, signals and the panic hook free the console's image before restoring the terminal. Synchronized output groups the text and image placement into one visible frame. Cursor save/restore and `C=1` prevent graphics from shifting the interface; quiet image commands suppress their replies, and the bounded input filter consumes negotiation responses. Direct placements specify only the limiting row or column dimension, letting the terminal preserve source aspect instead of stretching a rounded cell rectangle. Cell metric changes invalidate placement geometry while retaining verified pixel data. Terminal pixel/cell dimensions determine the limiting axis when reported, with an 8×16 fallback.


## Images inside tmux

Inside a tmux pane, run:

```sh
freight-tui --demo --graphics kitty-tmux
```

Use the same option with a live connection to inspect authenticated shipment evidence. This explicit mode requires a terminal that supports Kitty Unicode placeholders, UTF-8 and true color, and a tmux path that permits passthrough. On versions with the `allow-passthrough` option, enable it in the intended tmux server configuration. Older tmux may need a terminal-specific true-color declaration; the isolated tmux 3.0a qualification uses `xterm-ghostty:Tc`. Freight does not read or change your tmux configuration. Auto mode keeps its conservative multiplexer fallback. Nested tmux, GNU Screen and Zellij are not qualified by this mode.

Image uploads, virtual placement and cleanup use quiet (`q=2`) tmux DCS envelopes with correctly doubled escapes. Save/restore around every wrapped command preserves the outer terminal cursor while tmux retains its own cursor model. The ordinary pane text contains explicit row, column and full 32-bit image-ID placeholders. There is no bypassed physical image coordinate or broad deletion. One virtual placement belongs to the console's random image ID. The preview grid is capped at 256×256 cells and retains image aspect ratio. Upload precedes flushing placeholder text; normal redraws reuse pixels, while resize and close reconstruct or free only the owned image.

Color identifies placeholder images. `NO_COLOR` or `--no-color` therefore selects text metadata in tmux mode, without emitting placeholder cells or image commands. Direct Kitty rendering retains its existing behavior. All document/source identity, RGB dimensions/length/hash checks and original-data rules remain unchanged. Previewing does not approve or mutate evidence.

Initial integration failures and their corrections remain separate: missing placeholder colors with `NO_COLOR`; background repainting without complete outer cursor preservation; a close check that captured before tmux delivered Escape; and optical checks obscured by unrelated desktop notifications or Ghostty's 750ms resize overlay. The final checks retain full image/pane-boundary/aspect requirements, wait for observed scoped cleanup, and sample multiple unobscured sentinel columns. Screenshots and terminal wire bytes remain private locally, with exact hash/size records in Git. The server and dependency graph are unchanged; earlier capacity, image/browser/TLS and recovery records retain their original complete-source bindings.

## PDF worker fault qualification

On a Linux host with a local Docker Engine and host PID visibility, run `python3 scripts/check-document-preview-image.py` to build the current complete production Dockerfile and qualify that image. CI and the local release path invoke this gate through `scripts/check.sh`. It records the application/Docker build inputs before building, requires them to remain unchanged, and retains build logs, image identity, the qualification report and owned artifacts. It uses a separate Docker client configuration and the local daemon socket.

For a previously built local candidate, run:

```sh
python3 scripts/qualify-document-previews.py \
  --image your-local-freight-image \
  --output /tmp/new-preview-qualification.json
```

The standalone command qualifies the supplied image; it does not prove that image matches the current checkout. It starts a new API container with UID/GID 10001, a read-only root, dropped capabilities, no privilege gains, two CPUs, 512 MiB and 64 PIDs. It rejects baked service settings, volumes and unexpected entrypoints, uses synthetic credentials and an in-memory fixture, and selects an immutable local image ID without pulling. It uses no shared database or application volume. Report paths must be fresh, responses are bounded, and every mutation or signal verifies the owned container and live renderer identity.

The predeclared finite checks stop a real renderer and disconnect its HTTP request, verify the permit stays busy until timeout/reaping, kill another renderer and require recovery, then issue twelve simultaneous previews. Eleven must return bounded 503 responses; the admitted request must either render every independently specified pixel correctly or return a bounded 422. Faster valid rendering is accepted within the same deadline. Ordinary health/readiness reads must retain p95 at most 250ms and catalog reads 500ms, with at least 50 observations each. Busy responses must finish within 500ms and the admitted workload within seven seconds. Original bytes, pending-review metadata, exact text-preview retries, kernel memory/OOM counters and owned cleanup are checked. Failed observations reject the gate and still stop its monitoring thread and container. These finite fixtures do not establish sustained deployment capacity or qualify the large canonical workload alongside PDF traffic.

## Downloading production-image preview diagnostics

`python3 scripts/check-document-preview-image.py --output /absolute/path/to/fresh-image-check` builds the complete current production Dockerfile, then runs the finite PDF fault qualification. The output directory must not exist; an existing file, directory or symlink is refused before any build. Omit `--output` to allocate a private temporary directory. The gate freezes the application, Docker inputs, qualification tools and text fixture, and rejects changed inputs or a failed qualification report.

Both ordinary command failures and successes retain `result.json`, phase states, raw build/qualification output, and a `diagnostics.tar.gz` bundle with a per-file SHA-256 inventory. The gate preserves raw bytes and reports a missing Docker executable as a failure. The bundle selects known build/runtime diagnostics, excludes Docker client settings and unrelated files, and refuses selected symlinks or special files. Existing bundles are preserved. Abrupt process termination before finalization can leave no bundle; absence is not a pass.

The required integration workflow allocates a new directory for each execution and uploads `freight-preview-image-<run-id>-<attempt>` with 14-day retention even after an ordinary failed check. If an earlier matrix check prevents the image gate from starting, the artifact contains only the allocation marker; this does not demonstrate an image build. The uploader is pinned to [official upload-artifact v7.0.2](https://github.com/actions/upload-artifact/releases/tag/v7.0.2), using Node 24; the authorized self-hosted runner needs version 2.327.1 or later with that runtime. The job's existing fork exclusion and read-only repository permission remain in place. No remote upload has yet been qualified.

After downloading an artifact, use a new empty directory and check the bundle before opening it:

```sh
sha256sum --check diagnostics.sha256
tar -tzf diagnostics.tar.gz
tar -xzf diagnostics.tar.gz
cat result.json
```

The checksum detects changed archive bytes; it is not independent provenance or security approval. `result.json` must say `pass`, every phase must pass, and the recorded inputs/image must match the intended candidate. An allocation marker, synthetic guard result, or bundle replay cannot qualify the production image. A passing image preview gate remains only one component of the release, capacity and independent acceptance requirements.

## PDF worker syscall boundary

PDF metadata and page conversion now re-execute the same API executable in its fixed `--pdf-worker` role. That single-threaded entry runs before dotenv, credentials, listeners or Tokio. It changes to `/`, installs a default-deny seccomp policy, then execs only the fixed `pdfinfo -` or bounded `pdftoppm` page command with a cleared environment. No separate helper binary or runtime package is installed. The existing resource limits, stdin/stdout boundary, one decoder permit, deadlines and reap-before-release behavior remain in place. There is no unfiltered retry. Policy/launcher unavailability returns 503, including when it closes its input early; invalid/protected/over-limit renderer results remain 422 and originals remain available.

The syscall policy permits ordinary I/O/runtime calls for the tested native tools and denies socket creation/communication, child creation, process signalling and cross-process memory operations. Default denial also covers unlisted x32 syscall numbers; the independently instrumented runtime fixture verifies 17 actual denials, including a socket operation that the outer Docker policy permits through x32. The current policy also restricts `open`/`openat` to known read-only flags. Write access, creation, append, truncation (including `O_RDONLY | O_TRUNC`), path-only handles, temporary files and unknown flags are denied. `openat2` stays default-denied because its flags are passed through a pointer; BPF cannot safely inspect that structure. Zero-byte file-size limits alone do not prevent truncation. This remains **not filesystem read isolation**: unrelated paths can still be read, and exec/other required calls remain available. Native advisories, filesystem access and independent security acceptance remain open.

On the qualified Linux x86-64 host/runner, check the installed policy with:

```sh
cargo build --locked --bin stateset-freight
target/debug/stateset-freight --pdf-worker --probe-policy
```

The probe confirms sockets work before filtering, then checks six socket denials, refused child spawning, nine mutating/path/unknown-flag open variants, a single thread, no-new-privileges and an added kernel filter. The open controls use `/dev/null` so a broken filter cannot damage a fixture or application file. The probe includes read-only truncation and reports `readonly_file_opens: true`; integer flag comparisons use the same low 32-bit word as Linux open/openat. It deliberately reports `filesystem_isolation: false`. CI/release checks run this probe, and the image fixture additionally requires the original container filter and observes inherited filters on actual rendering processes. The image diagnostic bundle retains `runtime-policy.json`. The new `seccompiler` 0.5.0 dependency loads BPF directly; it does not depend on host libseccomp resolving Landlock syscall names. Other architectures/kernels need their own qualification.

## Complete production image qualification

The production Dockerfile compiles the API with `cargo build --release --locked --jobs 2`. BuildKit mounts bounded temporary storage for the Cargo target (2 GiB), registry (1 GiB) and `/tmp` (512 MiB). The compiler process tree inherits a 3 GiB per-process virtual-address-space limit. Only the optimized executable is copied out of the temporary target before the runtime stage; compiler/download scratch is not retained in new build layers. These are per-mount/per-process bounds, not an aggregate builder cgroup limit. See [Docker's tmpfs mount reference](https://docs.docker.com/reference/dockerfile/#run---mounttypetmpfs).

Run the existing complete image gate with a fresh diagnostic directory:

```sh
python3 scripts/check-document-preview-image.py --output /absolute/path/to/fresh-image-check
```

This finite local image run does not complete required remote CI, PostgreSQL/browser/TLS/sibling/recovery qualification of the complete deployment, combined financial/PDF capacity, independent acceptance or public distribution review. Earlier debug/API-bind diagnostics retain their original scopes.


## PDF worker read-only file opens

An instrumented fixed `pdfinfo` fixture runs against writable owned canaries with the ordinary outer Docker filter, no network, no capabilities and no-new-privileges. Both the direct baseline and preceding API worker truncate/create canaries despite `RLIMIT_FSIZE=0`. The current worker returns EPERM for all 78 denied calls, preserves all 33 existing canaries and keeps six new paths absent; all twelve permitted read opens and the exact read-content control pass. This fixture shadows only `pdfinfo`, with no API executable replacement. A deliberately unsafe preceding-image candidate is rejected and all its containers are removed. A separate private O_TRUNC policy mutation makes the harmless built-in probe fail closed.

The complete current production Dockerfile then passes ordinary PDF fault/recovery/admission/original checks without a renderer or API substitute. Separate actual-image HTTP checks verify every expected PNG/two-page PDF pixel, protected/out-of-range refusals, role/tenant boundaries and original/review retention; ten PNG/PDF API/TUI PTY modes pass. Earlier attempts stopped for socket permission or the predeclared disk reserve and remain retained. Only task-owned compiler caches were losslessly archived; all earlier evidence and tested executables remain available. This does not establish filesystem read isolation: unrelated file reads, inherited descriptors and other required calls remain a separate security boundary. Runtime packages and their retained native findings are unchanged; the full release/deployment/capacity and external acceptance requirements remain open.


## Isolated PDF renderer service

The default and restricted-role Compose profiles run PDF conversion in a separate service from the API. The database-TLS overlay retains that separation and supplies no database certificates or credentials to the renderer. It uses the same image with `--pdf-renderer-service`, UID 10002/GID 10001, its own PID and private IPC namespaces, a read-only filesystem, no network, no capabilities and no-new-privileges. It receives only a private tmpfs socket volume: no API credentials, database settings, document storage or application data mounts. Startup rejects unexpected environment variables, mounts, privileges and namespace settings. The broker is nondumpable, and children close inherited descriptors before processing input.

The API selects its renderer once at startup with `FREIGHT_PDF_RENDERER_SOCKET`; `FREIGHT_PDF_RENDERER_UID` defaults to 10002. The socket path must be absolute and bounded. The API verifies the peer UID before sending a PDF; the service accepts only the API/health UIDs. A bounded binary exchange grants admission before the body and binds the response to the requested page and original SHA-256. One service permit covers input, fixed Poppler metadata/rendering commands, child termination/reaping and output. Connection, transfer and execution deadlines bound stalled peers. An unavailable service returns 503 without retrying through the native backend. Original downloads remain available.

The service requires `/run/freight-pdf` to be an owned private tmpfs directory. An exclusive lock protects its socket lifecycle; startup preserves unexpected regular files and symlinks, and only removes a stale owned socket after confirming it is inactive. Graceful shutdown drains active work before removing its own socket. The API mounts the socket volume read-only. Use `stateset-freight --check-pdf-renderer` for the service health check; the API readiness endpoint remains independent so users can still retrieve documents during renderer outages.

The API retains its existing 512 MiB/two CPU/64 PID qualification limits. The separate renderer adds **384 MiB, one CPU and 16 PIDs**, with 16 MiB temporary storage and a 1 MiB socket volume. This separation adds a resource budget; it is not evidence of better throughput under the previous combined budget. The native parser remains Poppler, with the existing 256 MiB address-space, four CPU-second, five wall-second per-child limits and output/page bounds.

Outside Compose, the default native Linux worker now requires a fully enforced Landlock ABI 1 filesystem allowlist in addition to its default-deny syscall policy. It permits standard trusted library/font/Poppler runtime trees and fixed tool paths, closes inherited non-stdio descriptors even above the new descriptor limit, and denies application/home/proc environment reads. This is a runtime allowlist, not a promise to hide all operating-system metadata: `/etc/os-release`, for example, resolves into the allowed `/usr/lib` tree. Unsupported kernels or nonstandard runtime layouts fail closed with 503; there is no unrestricted fallback. Containers should use the separate service, since the outer Docker policy can block Landlock. Neither path changes the client RGB protocol, document authorization, original bytes or review state.
