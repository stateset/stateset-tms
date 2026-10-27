# Paid-customer credits and refund reconciliation

Use **Freight audit → Customer billing** when an actual customer agreement reduces an issued invoice, including one already paid. The original invoice and genuine receipt remain. A credit changes revenue and receivable; a separately recorded bank refund changes cash. Neither action contacts the customer or sends funds.

## Operator workflow

1. Find the original, supplemental, or cancellation invoice. Select **Credit note**, enter its unique number, exact amount in the invoice's currency, actual issue date, and reason. Total active credits cannot exceed the original invoice amount. The issue date may precede entry but cannot precede invoice issuance or be in the future; the authenticated entry time remains separate.
2. If the credit reduces funds already received, record the actual customer contact, confirmation reference, evidence reference, and findings. Optionally attach a stored file from that shipment. These fields attest to evidence inspected by the finance user; the application does not independently authenticate external correspondence. Supporting files retain their original SHA-256 digest.
3. A credit exceeding the current outstanding amount leaves a signed negative receivable, displayed as **customer refund due**. Received, credited, and refunded amounts stay separate. Positive customer debts and credits owed are separately totaled; one customer's credit never hides another customer's receivable.
4. After money actually clears externally, select **Record customer refund**. Record the account label, unique bank reference, actual clearing date, amount, reconciliation note, and actual customer confirmation/evidence. Allocate only the amount returned, using current invoice versions. Multiple partial refunds or a batch across 1–100 credit-bearing invoices for the same customer are supported, including void cancellation invoices. Every positive allocation must fit its target's remaining refund due; their sum must equal the bank amount exactly.
5. Download the invoice statement. It retains the original amount and shows credits, original receipts, refunds, and the current balance. A void/retracted invoice explicitly offsets its original charge. Internal contact/evidence details, private bank references, buy costs, and margins are excluded.

Refund clearing cannot precede the latest recorded invoice financial activity: issuance, credit issue/reversal, bank clearing/reversal, or void/retraction. This uses the credit's actual issue date rather than its entry timestamp. It does not reconstruct historical balances or implement accounting periods. Dates use UTC and cannot be in the future.

## Corrections and cancellation

- Reverse an erroneous bank reconciliation with the actual correction date and reason. It appends the opposite journal, retains the original allocation, and reopens its refund liability. It does not recover money from the bank. An explicit `corrects_id` can link one replacement with the same account, bank reference, and direction; branching and implicit bank-line reuse fail.
- Reverse an erroneous credit with current credit and invoice versions when refund history exists. Genuine refunds remain recorded. This can reopen a customer receivable; do not reverse a real refund to conceal that debt. The reversal date cannot precede later recorded financial activity.
- Retracting a customer cancellation fee requires reversing active credits, but preserves genuine receipts. It voids that fee's invoice and exposes only the amount actually received as a refund liability. Use the normal customer refund reconciliation after the external return clears. A 20,001-cent fee with 10,001 cents received produces a 10,001-cent refund liability, never a fictitious 20,001-cent payment.
- Ordinary invoice voiding requires zero net customer funds retained and no active credits. Correct an erroneous receipt only when the evidence supports that correction. When a paid ordinary invoice must be removed, a full credit and actual refund can return the funds; reverse that credit with current versions before voiding. Original bank history remains throughout.
- Customer credits are not automatically transferred to another invoice. Customer credit applications, prepayments, overpayments, external bank execution/feeds, tax qualification, currency conversion, and period-close policy remain separate work.

## API and integrity

`POST /v1/bills/{id}/credits` accepts `{expected_version, number, amount_cents, reason, issued_on?, confirmation?}`. A confirmation is `{counterparty_contact, confirmation_reference, evidence_reference, note, document_id?}`. It is required when the credit exceeds the current positive receivable, including after full payment.

`POST /v1/cash` uses `direction: "customer_refund"`, `currency` equal to the allocated invoices' currency, the existing bank/allocation fields, and mandatory `confirmation` of the same shape. Allocations identify the invoice and its current `expected_version`; callers cannot set customer, status, digest, or recorded subject. `POST /v1/credits/{id}/reverse` also accepts `expected_bill_version`, required once that invoice has any refund history. All writes require `finance.write` and an `Idempotency-Key`.

Workspace bills expose `totals.received_cents` (original active receipts), `credited_cents`, `refunded_cents`, signed `balance_cents`, positive `refund_due_cents`, `remaining_creditable_cents`, and `activity_date`. Finance summary exposes positive `receivable_cents`, separate `customer_credit_cents`, and signed `net_receivable_cents`. An active negative invoice has status `credit_due`; a retracted invoice remains `void` while its liability is visible.

Credit creation debits freight revenue and credits accounts receivable. Actual refund reconciliation debits accounts receivable and credits cash. Stored credit/refund agreement files, plus applicable supplement/cancellation evidence, are checked inside the tenant transaction. Missing/corrupt bytes roll back business state, journal, audit, and idempotency together. Proof supporting active credits or retained refund history cannot be invalidated through generic document rejection. Corrections preserve the original evidence and actor.

Storage migration `008_customer_refunds.sql` advances format 5 to **6**, preserving all existing records and bytes while fencing older writers. See [deployment and rollback](storage.md).

## Qualification

An independently expected 149,000-cent invoice/receipt, 9,001-cent credit, and actual refunds of 3,001 plus 6,000 leave 139,999 cents of revenue/net cash and zero receivable. Reversing the credit after refund reopens 9,001 cents of receivable while retaining both refunds and the original receipt. Tests cover same-customer original/supplement batching, mixed-customer rejection, duplicate references, stale versions, late-entered actual dates, correction chains, evidence ownership/locking, and paid cancellation retraction.

PostgreSQL qualification corrupts actual stored bytes, verifies complete rollback, restores them, and reuses the failed command key. Independent instances cannot both consume one credit balance; reconnect preserves proof, original receipts, journal totals, and tenant isolation. Production browser qualification covers paid credit creation, file selection, partial refunds, statement privacy, correction, mobile width, and viewer denial. All evidence is synthetic; no live customer agreement or bank transfer is claimed as verified.