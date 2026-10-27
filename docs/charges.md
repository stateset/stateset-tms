# Additional freight charges

Shipment details now separate the original buy/sell rates from approved additions. Operations or finance can propose detention, lumper, fuel, layover, extra-stop, or other documented additional service. Finance approves/rejects each carrier cost or customer charge separately. Proposed amounts never affect the agreement total, margin, invoice matching, or journal.

## Record and review an addition

1. Open the accepted/dispatched/in-transit/delivered shipment and select **Propose charge**. Select carrier buy or customer sell, kind, an explicit positive amount in the shipment's currency, a service description, and a line-specific evidence reference. For example, use `RATE-123/DETENTION-1` to identify a service within a larger agreement. Select an immutable supporting file or identify the evidence held externally.
2. Finance opens **Freight audit → Carrier audit → Charges awaiting review**, or **Review shipment charges** beside an invoice. Inspect the original evidence and verify the amount with the named counterparty. Approval requires the actual contact, external agreement reference, and finance note. Reject an unsupported request with a reason.
3. Approved buy additions re-audit the received carrier invoice against base buy plus approved additions. Its received amount is never changed to manufacture a match. An invoice already containing the additional service can become matched after approval; an invoice lacking that service remains a variance until the carrier supplies an actual corrected invoice. POD review and delivery remain required for approval/posting.
4. Approved sell additions update any customer draft's amount, agreement lines, source load version, and draft version. A person confirming an older draft cannot issue it under the stale version. Issuance freezes the base amount and approved lines. The customer statement itemizes base freight and customer-approved descriptions/amounts; internal notes, carrier costs, agreement contacts, and proof references stay local.

The base rate is inclusive of the original freight agreement. Do not add an already-included service again. An amount, kind, gate duration, or 210 header does not determine contract eligibility; the finance decision records the verified counterparty agreement. No charge is calculated automatically from dwell or emitted as a carrier/customer message.

## Agreement and evidence identity

Every proposal preserves its original amount, description, side, counterparty snapshot, creator/time, and evidence reference. A carrier charge also retains the exact tender attempt. A former carrier's proposal cannot be approved against a replacement tender, even if the carrier is the same. Such proposals remain visible and can be rejected with the actual findings.

An evidence reference identifies one active proposal/approval per load/side, case-insensitively. A supporting document can cover several separately identified service lines. Stored evidence must belong to the same tenant/load and cannot be rejected. Proposal metadata captures its original SHA-256 digest. The approval decision records finance's review of that specific charge; a separate generic file-review status is not a substitute for counterparty agreement, and charge approval does not approve a POD.

Before approving a charge with a stored file, PostgreSQL reads and verifies the original bytes inside the business transaction. Missing/corrupt bytes roll back the charge, re-audited invoice/draft, load version, audit, and receipt. Carrier invoice approval, customer invoice issuance, and settlement preparation check their side's approved charge files as well as stored POD bytes. A file supporting an approved charge cannot subsequently be rejected until that charge is reconciled. No external evidence URL is fetched automatically.

## Pending reviews, corrections, and posting locks

Pending carrier proposals hold carrier invoice approval. Pending customer proposals hold customer invoice issuance. These holds are independent: a customer negotiation does not change the carrier's agreement. Finance can reject unsupported pending requests without treating them as money owed.

To correct a rejected proposal, choose **Correct charge proposal**. To retract an approved amount before financial posting, use **Void charge approval** with actual counterparty confirmation and a reason, then correct it if appropriate. Corrections append a new proposal linked to the same load, side, counterparty, tender, and evidence reference. Prior amounts/decisions remain, and a correction chain cannot branch or reuse an active reference. Re-auditing never changes the received carrier invoice amount. Draft changes bump its version; approvals/voids do not themselves post a journal.

Approved charges block carrier release, cancellation, withdrawal, and re-planning until finance reconciles them. A pending release/custody incident blocks new charge approval. This prevents an additional agreement from silently following a different carrier or rewritten freight plan.

Once a carrier invoice is approved or a settlement exists, its original buy-charge basis is locked. Use [carrier payable adjustments](payables.md) for actual supplemental invoices, revised totals, or carrier credit memos after approval; those append separate postings while preserving the primary invoice. An issued non-void customer invoice locks sell additions/voids. Use [separate customer supplements](customer-supplements.md) for later documented customer additions; the original snapshot stays fixed. Cancelled shipments use [separate cancellation agreements and independent fee targets](cancellations.md), without requiring or fabricating delivery. Negative pre-posting contract amendments and automatic detention pricing remain further TMS work. One primary carrier invoice and one non-void original customer invoice remain the account anchors per load.

## Financial and integration behavior

Carrier expense/payable and prepared settlement use the matched approved invoice amount, including approved buy additions. Customer receivable/revenue use the issued base-plus-additions total. The existing external bank allocation/reversal rules apply to those totals. No transfer, tax calculation, remote GL posting, or invoice delivery is implied.

The inspected `stateset-edi` 210 codec exposes the header net amount and references; it does not provide certified L0/L1 accessorial breakdowns. Freight does not infer or auto-approve charge lines from that total. The real-codec harness verifies a 132,551-cent 210 against an original 125,000-cent buy plus a documented/approved 7,551-cent unloading service. A 204 original has no supported price-amendment/charge transmission field in this gateway model. Record actual carrier/customer coordination outside Freight.

Sync-server tracking remains operational data. Freight charge approvals, buy/sell amounts, counterparty agreement evidence, journals, and customer statements are not written into commerce order totals or tracking. Finance remains inside the tenant's existing Freight PostgreSQL transaction.