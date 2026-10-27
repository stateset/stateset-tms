# Applying carrier credits to other invoices

Use **Freight audit → Bank reconciliation → Carrier credit applications** when a carrier confirms that an existing credit balance covers other prepared invoices. This is a recorded non-cash allocation between that carrier's settlements. It does not issue a new credit memo, change invoice costs, create a bank transaction, or contact the carrier.

## Record the actual confirmation

1. Approve the actual carrier credit/reducing invoice revision using the [payable adjustment workflow](payables.md). After payment, that can leave a negative settlement balance labeled **Carrier credit balance**.
2. Prepare the other approved carrier invoices. The credit source and every target must belong to the same carrier, have reviewed delivery evidence, and have no release/custody hold.
3. Select **Apply carrier credit** and choose the source. Select 1–100 outstanding prepared invoices for that carrier and enter the actual confirmed allocation on each. The total must be positive, fit the available source credit, and fit each target's current payable. A source cannot allocate to itself; duplicate targets fail.
4. Record the carrier's application reference, actual contact, confirmation reference, decision note, and application date. Optionally select an immutable supporting file on the source shipment. These are finance's recorded external facts; the application does not independently establish remote carrier acknowledgment.
5. Confirm. The original invoices, original settlement principals, recognized costs/margins, credit memos, and bank allocations remain retained. Effective balances and versions change on every affected settlement. A stale payout/refund/allocation cannot consume the prior balance.

Only finance/admin can apply, reverse, or correct a carrier application. All tenant staff can inspect the source, target invoice/load references, amounts, contact/confirmation, recorded actor/date, supporting-file digest, and correction history. Viewer controls remain read-only, and the API enforces the same role boundary before mutation or idempotency replay.

## Posting, dates, and evidence

An application debits **carrier payable on each target load** and credits **carrier payable on the source load** by the same total. It leaves aggregate carrier payable, freight expense/revenue, and cash unchanged. Individual source/target balances are reclassified. `credit_application_cents` is the signed net balance effect: positive on a source whose credit was used elsewhere; negative on a target covered by another credit. Costs are not transferred between shipments.

Balances are current, not a reconstructed historical statement. The application/reversal date must be on or after existing payable journal activity and posted carrier document/reversal dates on every affected settlement, and cannot be in the future. Comparisons use UTC dates, consistent with the rest of this subledger. The form exposes the current minimum date. Legacy approved liabilities without a recognition journal are recognized once as a current opening liability, consistent with settlement preparation. This workflow does not reconstruct historical books or supply accounting-period policy.

Optional application files bind to their source shipment and captured SHA-256. Posting checks that file and the source/targets' stored POD, original approved charge evidence, and active carrier-adjustment evidence inside the same tenant transaction. Missing/corrupt bytes roll back balances, journals, audit, correction links, and idempotency receipts together. A generic document rejection cannot invalidate an active application's own supporting file; reverse the application first. Restoring original bytes permits retry of the original failed request key.

## Reversal and correction

**Reverse credit application** records a fresh carrier contact, retraction confirmation, note, and date. It requires the application's version and the current version of its source and every original target. Its journal appends the exact opposite load/account lines, retaining the original posting, confirmation, bank records, and supporting-file digest. Reversal may reopen target payables and the source credit even when the targets were already paid. It does not reverse a bank transfer.

**Correct reversed application** creates a new application with `corrects_id`. The original must be reversed and uncorrected, and the carrier and normalized application reference must match. The correction can repair the source or target allocation within that carrier, amounts, confirmation, and source-bound evidence, subject to current credit/payable balances. Original/new records link both ways; the chain cannot branch. Reusing a reversed reference without a correction link or reusing an active reference fails. Reference uniqueness is per carrier, with trimmed ASCII case-insensitive comparison.

Reversing the underlying carrier credit memo is a separate decision. An active application remains retained; the retracted source credit can reopen its source payable. There is no automatic rewriting of target invoices, old applications, or cash records.

## Independent expected result

A source invoice of 118,000 cents is paid, then reduced by an actual 4,501-cent memo. Applying 1,251 to a 20,000-cent target and 3,250 to a 10,000-cent target leaves source credit zero and target payables 18,749 and 6,750. Applying the credit itself changes neither cash nor expense. A separate 15,000-cent payable for another carrier remains untouched.

Paying all remaining targets closes the subledger at 158,499 cents of cost/net cash paid and zero payable. Reversing the application reopens source credit 4,501 and target payables 1,251/3,250. A corrected application of 3,001 leaves source credit 1,500 and target payable 1,500, shown separately until another confirmed allocation or actual refund/payment occurs. Reversing the original credit memo after reapplication reopens 4,501 on the source while retaining all applications and bank entries.

Tests check those cent-exact totals, per-load payable/ledger agreement, unchanged invoices and bank records, stale/forged/mixed-carrier/duplicate/over-allocation rejection, original/reversal/correction proof, legacy recognition, PostgreSQL corruption rollback and reconnect, and races between applications and actual refunds. Browser checks cover split allocation, later payout/reversal/correction, mobile dialogs, and viewer denial.