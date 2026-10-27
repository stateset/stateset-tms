# Supplemental customer billing

Use this workflow when a customer agrees to additional freight billing after the original invoice has issued, including after that invoice is fully paid. The original invoice, its customer/shipment snapshot, bank allocations, and journal remain retained.

## Operator workflow

1. Open **Freight audit → Customer billing** and find the issued original invoice.
2. Select **Draft supplement**. Record a unique invoice number, positive inclusive amount in the original invoice's currency, due date, customer-facing charge description, actual customer agreement contact, confirmation, and agreement reference. Optionally select a stored file for that shipment. The finance user records the actual evidence; the application does not contact the customer to obtain agreement.
3. Review the draft and its retained agreement. A draft has no receivable or revenue posting. Its amount is the additional agreed amount, not a replacement shipment total.
4. Select **Issue invoice**. Accepted delivery evidence, an active issued original, a billing issuer, a bill-to address, and unchanged/non-rejected supporting evidence are required. Stored POD, original charge files, and the supplement's agreement file are checked inside the same tenant database transaction.
5. Download the supplemental invoice for your billing process. It identifies the original invoice and customer confirmation, describes the additional service, and shows only its own amount, credits, receipts, and balance. Buy cost, margin, internal evidence references, and agreement contacts are excluded. Dynamic text is escaped. Issuance does not send the document or establish customer receipt.
6. Record actual cleared receipts against the supplement, or a batch of that customer's invoices. Its own outstanding balance/version controls allocation. Credit notes and their reversals work on supplements as they do on originals.

The agreement is inspectable in the billing row, including supporting-file download and correction links. All tenant staff may read; only finance/admin may draft, issue, credit, reconcile, or void.

## Frozen records and correction rules

- The supplement inherits the original customer and shipment snapshot, even after master data changes. Its issuing identity/payment instructions are captured at its own issuance. Draft revisions can change number, due date, and bill-to address. Amount/agreement corrections require voiding and explicitly correcting the draft/document.
- Numbers are unique across original and supplemental customer invoices. Reusing a number from a void record is forbidden.
- Each load's agreement reference is trimmed and compared without ASCII case distinctions. It can support one active supplement. Separate genuinely agreed additions need separate references; splitting or duplicating the same agreement under new references is not an automatic billing policy.
- Voiding retains the original and, if issued, appends a reversing journal. No active credits or net retained customer funds may remain. Preserve genuine receipts; use [paid-customer credits and actual refund reconciliation](customer-refunds.md) when funds must be returned. Reverse a reconciliation only to correct an erroneous bank record. A customer bank transfer is never reversed by bookkeeping.
- A replacement for a void supplement must supply `corrects_id` pointing to that uncorrected void record, keep its original-invoice identity and agreement reference, and use a new invoice number. Both records link to each other. The correction may repair the amount, description, confirmation, or supporting file. A chain cannot branch.
- An original invoice cannot be voided while any non-void supplement depends on it, including a draft. There are no supplements to supplements.
- Optional supporting files bind to their original SHA-256 digest. Rejection is blocked while an issued non-void supplement uses the file. Corrupted/missing stored bytes abort issuance, journal, audit, and idempotency receipt together. Restoring the bytes permits the original failed request to succeed.
- Creation checks both original-invoice and load versions and increments them. Issuance, credits, and receipts use each invoice's version. Concurrent attempts cannot issue twice or consume the same outstanding amount twice.

## Amounts and verification

Each invoice supports 1–100,000,000 integer minor units in the original invoice's currency. Original contracted sell amounts stay visible separately. Current load sell/margin includes issued non-void supplements and subtracts active customer credits; draft supplements do not add revenue or margin. Reversal/void updates derived totals while preserving old journal lines.

The load-board CSV exports effective buy, sell, and margin alongside original base rates and net additions/adjustments/credits. Those net differences are not labeled as original charge approvals. The known paid/credited supplement exports sell USD 1,590.00, margin USD 410.00, base sell USD 1,490.00, and net sell additions/credits USD 100.00.

The known scenario starts with an issued/paid original of 149,000 cents. A supplement of 12,501 cents, credit of 2,501, and real receipt of 10,000 yield 159,000 cents of revenue/net receipts and zero receivable. Reversing that receipt and credit, then voiding the supplement, leaves the original paid and restores revenue/cash to 149,000 cents. Tests inspect these independently expected journal totals, original snapshots, escaped statements, correction chains, role denial, stale/duplicate/forged requests, PostgreSQL corruption/races/reconnect, and desktop/mobile browser operation.

This adds inclusive operational billing in the original invoice's [currency](multi-currency.md), not tax determination, invoice delivery, electronic customer acceptance, bank execution, customer credit applications, currency conversion, or accounting-period policy. Those remain additional workflows/integrations. Shipment tracking writeback to `stateset-sync-server` does not change upstream product-order/payment totals.