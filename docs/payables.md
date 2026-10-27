# Carrier invoice adjustments and credit balances

Carrier payable adjustments extend the financial lifecycle after the primary invoice has been approved. The original invoice, original settlement principal, original journal, and recorded bank allocations remain intact. Finance records an actual supplemental invoice, replacement total, or carrier credit memo as a separate versioned draft, verifies the carrier's document/agreement, and posts its signed difference. A draft creates no obligation or payment.

## Classify an additional 210

An additional authenticated 210 for an assigned shipment no longer repeatedly fails solely because its primary invoice already exists. Freight retains the exact normalized event as a `carrier_document`, without posting another liability. The signed inbox shows **Finance review** and links to the shipment. The original authenticated notification/signature and processing attempt remain. Transport Retry does not apply to this business decision.

The gateway's codec supplies a header total, invoice number, date, and references; it does not establish whether the document is a supplement, revised total, or credit. Finance must inspect that evidence. A repeat of the primary number/amount is ignored only while that amount remains the recognized total. A new document returning an already-adjusted invoice to its original amount still requires classification.

- Before the primary is approved, **Apply actual invoice revision** updates its number/amount from the canonical document, retains the old/new facts in activity, and re-audits against the agreed rate. No payable is posted. A replacement number cannot collide with another invoice for that carrier.
- After approval, **Classify carrier invoice** creates an adjustment draft bound to the original invoice, carrier, tender, and canonical document. The source number, total, currency, and supplied document date cannot be changed. A canonical source cannot support two active drafts/postings. An approved or dismissed item becomes `finance_reviewed` in the inbox while its original processing history remains.
- **Dismiss with reason** retains an unsupported, duplicate, or otherwise inapplicable document without changing money.

Negative 210 totals retain their original sign and enter finance review without creating a negative primary invoice or automatic credit posting. Finance must explicitly select verified credit-memo treatment and record carrier confirmation; the adjustment uses the positive magnitude while preserving the signed source. A positive 210 header alone cannot establish a credit designation: record the actual carrier credit document held externally or uploaded to Freight. Credits received before a primary exists remain unposted until an approved account anchor is available. Retired-carrier documents still use the existing custody/liability incident workflow rather than transferring onto a replacement assignment.

## Supplemental invoices, revised totals, and credits

Open shipment details → **Carrier payable adjustments → Record carrier adjustment**. Enter the actual document number/date, treatment, amount, description, evidence reference, and optional immutable supporting file. Document numbers preserve supplier text after trimming and ASCII case normalization; they may contain spaces. Only finance/admin can create, classify, approve, reject, correct, or reverse these financial records.

| Treatment | Effect when approved |
| --- | --- |
| Supplemental invoice | Add the document amount to recognized carrier cost/payable |
| Revised invoice total | Post only the difference between the document's replacement total and the current recognized invoice total |
| Credit memo | Reduce cost/payable by the actual credit amount |

Approval requires current load/adjustment versions, the carrier contact, external confirmation reference, and a substantive review note. The carrier/tender must still match, with delivered status and reviewed POD. Original approved charge evidence remains locked. Supporting file IDs bind to the same tenant/load and their captured SHA-256. Approval verifies stored bytes transactionally alongside POD and original agreement evidence. Missing/corrupt evidence rolls back state, journal, audit, and receipt together.

Revised-total drafts retain the recognized amount used to calculate their difference. If another adjustment changes that amount first, the stale revision cannot post even after refreshing its load version. Reject it and create an explicit correction. A zero difference creates no draft/posting. Positive document amounts in the shipment's currency and nonnegative recognized totals are bounded to 100,000,000 cents; a replacement total may be zero. Credits cannot exceed the invoice plus its active supplemental amounts.

Rejected/reversed documents require an explicit correction link. Original amounts and decisions remain, and correction chains cannot branch. Reversal records the actual carrier retraction, its own actor/contact/reference/date, and an appended opposite journal linked to the original. It preserves the original approval decision. Reversing a debit cannot leave dependent credits greater than the recognized obligation; reconcile those credits first.

## Settlement totals and actual refunds

The settlement's `amount_cents` stays its original approved principal. Its `total_amount_cents` includes active posted differences, and its signed `balance_cents` subtracts net external payouts (payouts minus carrier refunds). Posting/reversing an adjustment bumps an existing settlement's version; stale bank allocations cannot consume its old balance. Adjustments can also post before settlement preparation and are included when it is prepared. Primary payable recognition remains idempotent.

Credits after payment can create a negative settlement balance, labeled **Carrier credit balance**/`credit_due`. That credit is shown separately from positive prepared payables; it is not reported as a refund received or netted against another carrier's debt.

Once an actual refund clears, use **Bank reconciliation → Record bank transaction → Carrier refund received**. Choose the carrier and credit-bearing settlement(s), record the real account/reference/date/note, and allocate exactly the cleared amount. Allocations are positive, cannot exceed the available credit, and share one carrier. A refund cannot predate its supporting carrier credit document/reversal. Existing unique bank-reference, expected-version, reversal, and explicit correction rules apply. Reversing a refund reopens the credit; reversing the underlying credit after an actual refund can reopen a payable. Neither action reverses a bank transfer.

Use [carrier credit applications](carrier-credit-applications.md) to record actual same-carrier credit allocation across prepared invoices, with retained reversals and explicit corrections. Those reclassify payable balances while preserving costs and cash. The read model and margin include posted carrier differences. Customer sell rates, issued customer invoices, and commerce order totals remain independent. The bank and freight subledger record actual external evidence; they do not execute payments, fetch bank feeds, independently verify carrier authenticity, or post to a remote GL.

## Posting and verification

Supplemental debits post freight expense / carrier payable; credits post carrier payable / freight expense. Refund receipts post cash / carrier payable. Reversals append the opposite entry, retaining source/load IDs and authenticated actors. Preparing a settlement with posted adjustments checks their stored evidence again.

An independently expected scenario starts with 118,000 cents paid, adds/pays a 12,001-cent supplement, then posts a 4,501-cent credit and reconciles its actual refund: recognized cost and net cash paid are 125,500 cents, with zero payable. Reversing that carrier credit after the refund reopens 4,501 cents while preserving the original invoice and bank records. Tests cover revised totals/corrections, exact signed 210 retention/classification, actual sibling-codec parsing, malformed/duplicate/stale requests, role denial, PostgreSQL evidence corruption, concurrent posting/refund attempts, and desktop/mobile operation.