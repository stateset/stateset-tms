import { useFreightIdentity } from "./FreightIdentity";
import { useRef, useState } from "react";
import { Download, Plus } from "lucide-react";
import { Modal } from "./Modal";
import { CarrierCreditApplications } from "./CarrierCreditApplications";
import { expectedAccess, journalOwner } from "../lib/save-journal.mjs";
import {
  amountStep,
  currencyCode,
  formatMoney,
  moneyInput,
  parseMoney,
  summaryTotals,
} from "../lib/money.mjs";

const identityChanged =
  "The billing identity changed while this form was open. Your entries are retained. Close and reopen Billing identity to review the current details before making another change.";

const title = (value) =>
  value.replaceAll("_", " ").replace(/\b\w/g, (c) => c.toUpperCase());
const today = () => new Date().toISOString().slice(0, 10);
const cash = (value, currency) => formatMoney(value, currency);
const cents = (value, currency) => parseMoney(value, currency, { min: 0 });
function Field({ title, children, ...props }) {
  return (
    <label className="field">
      <span>{title}</span>
      {children || <input aria-label={title} required {...props} />}
    </label>
  );
}
function State({ value }) {
  return <span className={`badge ${value}`}>{title(value)}</span>;
}
function CustomerConfirmation({ value, label }) {
  const { href } = useFreightIdentity();
  if (!value) return null;
  return (
    <details className="customer-agreement">
      <summary>{label}</summary>
      <p>
        {value.counterparty_contact} · {value.confirmation_reference}
      </p>
      <p>Evidence: {value.evidence_reference}</p>
      <p>{value.note}</p>
      <small>
        Recorded by {value.recorded_by} ·{" "}
        {new Date(value.recorded_at).toLocaleString()}
      </small>
      {value.document_id && (
        <>
          <a href={href(`/api/freight/documents/${value.document_id}/content`)}>
            Download customer evidence
          </a>
          <small>SHA-256: {value.document_sha256}</small>
        </>
      )}
    </details>
  );
}
export function FinanceWorkspace({
  workspace,
  section,
  onSection,
  onChanged,
  openLoad,
  children,
}) {
  const identity = useFreightIdentity();
  const { fetch, href: identityHref } = identity;
  const [modal, setModal] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const [direction, setDirection] = useState("receipt"),
    [party, setParty] = useState(""),
    [selected, setSelected] = useState({}),
    [cashCurrency, setCashCurrency] = useState("USD");
  const pending = useRef(null);
  const [billToAddress, setBillToAddress] = useState("");
  const canWrite = workspace.access.capabilities.includes("finance.write");
  const bills = workspace.bills || [],
    cashEntries = workspace.cash_entries || [],
    credits = workspace.credits || [];
  const finance = workspace.finance || {};
  const findLoad = (id) => workspace.loads.find((v) => v.id === id);
  // Bills snapshot their currency; settlements and fees follow their load.
  const loadCurrency = (id) => currencyCode(findLoad(id)?.currency);
  const billCurrency = (bill) => currencyCode(bill?.snapshot.currency);
  // Prefill the bank amount from exact allocation inputs, not float sums.
  const selectedTotal = () => {
    try {
      return moneyInput(
        Object.values(selected).reduce(
          (sum, v) => sum + cents(v.amount, cashCurrency),
          0,
        ),
        cashCurrency,
      );
    } catch {
      return "";
    }
  };
  // Summary totals never add across currencies: show each non-zero group.
  const totals = (field) => summaryTotals(finance, field);
  const draftLoads = workspace.loads.filter(
    (load) =>
      load.status === "delivered" &&
      !bills.some((bill) => bill.load_id === load.id && bill.status !== "void"),
  );
  const open = (value) => {
    if (value.kind === "profile")
      value = {
        ...value,
        profile: workspace.billing_profile
          ? { ...workspace.billing_profile }
          : null,
        reviewedAccess: expectedAccess(
          journalOwner(
            workspace,
            identity.mode,
            identity.organization,
            identity.session,
          ),
        ),
      };
    if (value.kind === "supplement")
      value = {
        ...value,
        loadVersion: findLoad(value.bill.load_id)?.version,
      };
    if (value.kind === "bill") {
      const customer =
        value.bill?.snapshot.customer ||
        workspace.customers.find((v) => v.id === draftLoads[0]?.customer_id);
      setBillToAddress(customer?.billing_address || "");
    }
    if (value.kind === "reverse_credit")
      value = {
        ...value,
        billVersion: bills.find((b) => b.id === value.entry.bill_id)?.version,
      };
    setError("");
    setModal(value);
  };
  const profileChanged =
    modal?.kind === "profile" &&
    (modal.profile?.version || 0) !== (workspace.billing_profile?.version || 0);
  const targets = ["receipt", "customer_refund"].includes(direction)
    ? bills
        .filter((v) =>
          direction === "customer_refund"
            ? v.totals.refund_due_cents > 0
            : v.totals.balance_cents > 0,
        )
        .map((bill) => ({
          id: bill.id,
          party: bill.customer_id,
          label: bill.number,
          load: bill.snapshot.load_reference,
          currency: billCurrency(bill),
          balance:
            direction === "customer_refund"
              ? bill.totals.refund_due_cents
              : bill.totals.balance_cents,
          version: bill.version,
        }))
    : workspace.settlements
        .filter((v) =>
          direction === "carrier_refund"
            ? v.balance_cents < 0
            : v.balance_cents > 0,
        )
        .map((v) => ({
          id: v.id,
          party: v.carrier_id,
          label:
            (workspace.cancellation_fees || []).find(
              (fee) => fee.target_id === v.id,
            )?.number ||
            findLoad(v.load_id)?.reference ||
            v.load_id,
          load: v.id,
          currency: loadCurrency(v.load_id),
          balance:
            direction === "carrier_refund" ? -v.balance_cents : v.balance_cents,
          version: v.version,
        }));
  const parties = ["receipt", "customer_refund"].includes(direction)
    ? workspace.customers
    : workspace.carriers;
  function cashModal(direction, entry = null, bill = null) {
    const currency = entry
      ? currencyCode(entry.currency)
      : bill
        ? billCurrency(bill)
        : "USD";
    setDirection(direction);
    setCashCurrency(currency);
    setParty(entry?.party_id || bill?.customer_id || "");
    const initial = {};
    if (bill)
      initial[bill.id] = {
        amount: moneyInput(
          direction === "customer_refund"
            ? bill.totals.refund_due_cents
            : bill.totals.balance_cents,
          currency,
        ),
        version: bill.version,
      };
    if (entry)
      for (const allocation of entry.allocations) {
        const target = ["receipt", "customer_refund"].includes(direction)
          ? bills.find((v) => v.id === allocation.target_id)
          : workspace.settlements.find((v) => v.id === allocation.target_id);
        const balance =
          direction === "customer_refund"
            ? target?.totals.refund_due_cents
            : direction === "receipt"
              ? target?.totals.balance_cents
              : direction === "carrier_refund"
                ? -(target?.balance_cents || 0)
                : target?.balance_cents;
        if (target && balance > 0)
          initial[target.id] = {
            amount: moneyInput(
              Math.min(allocation.amount_cents, balance),
              currency,
            ),
            version: target.version,
          };
      }
    setSelected(initial);
    open({ kind: "cash", entry });
  }
  async function mutate(path, body) {
    setBusy(true);
    setError("");
    const fingerprint = JSON.stringify([path, body]);
    if (pending.current?.fingerprint !== fingerprint)
      pending.current = { fingerprint, key: crypto.randomUUID() };
    try {
      const response = await fetch(`/api/freight/${path}`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Idempotency-Key": pending.current.key,
          ...(modal.kind === "profile"
            ? { "X-Freight-Expected-Access": modal.reviewedAccess }
            : {}),
        },
        body: JSON.stringify(body),
      });
      const result = await response.json();
      if (!response.ok)
        throw Object.assign(
          new Error(result.error?.message || "Finance action failed."),
          { status: response.status },
        );
      pending.current = null;
      await onChanged();
      setModal(null);
    } catch (problem) {
      setError(problem.message);
      if (modal.kind === "profile" && problem.status === 409) {
        // Refresh the reviewed source without replacing the open form's values
        // or version. An exact lost-response retry keeps its original key.
        try {
          await onChanged();
        } catch {
          // Retain the original conflict when the follow-up read also fails.
        }
      }
    } finally {
      setBusy(false);
    }
  }
  async function submit(event) {
    event.preventDefault();
    const values = Object.fromEntries(new FormData(event.currentTarget));
    try {
      switch (modal.kind) {
        case "profile": {
          const body = {
            ...values,
            expected_version: modal.profile?.version || 0,
          };
          if (
            profileChanged &&
            pending.current?.fingerprint !==
              JSON.stringify(["finance/profile", body])
          ) {
            setError(identityChanged);
            return;
          }
          return await mutate("finance/profile", body);
        }
        case "bill":
          return await mutate(
            modal.bill ? `bills/${modal.bill.id}/plan` : "bills",
            {
              number: values.number,
              due_date: values.due_date,
              bill_to_address: values.bill_to_address,
              ...(modal.bill
                ? { expected_version: modal.bill.version }
                : { load_id: values.load_id }),
            },
          );
        case "supplement":
          return await mutate(`bills/${modal.bill.id}/supplements`, {
            expected_version: modal.bill.version,
            expected_load_version: modal.loadVersion,
            number: values.number,
            due_date: values.due_date,
            amount_cents: cents(values.amount, billCurrency(modal.bill)),
            description: values.description,
            counterparty_contact: values.counterparty_contact,
            confirmation_reference: values.confirmation_reference,
            evidence_reference:
              modal.corrects?.supplement.evidence_reference ||
              values.evidence_reference,
            document_id: values.document_id || null,
            corrects_id: modal.corrects?.id || null,
          });
        case "issue":
          return await mutate(`bills/${modal.bill.id}/issue`, {
            expected_version: modal.bill.version,
          });
        case "void":
          return await mutate(`bills/${modal.bill.id}/void`, {
            expected_version: modal.bill.version,
            note: values.note,
          });
        case "credit":
          return await mutate(`bills/${modal.bill.id}/credits`, {
            expected_version: modal.bill.version,
            number: values.number,
            amount_cents: cents(values.amount, billCurrency(modal.bill)),
            reason: values.note,
            issued_on: values.credit_issued_on,
            ...(values.customer_contact
              ? {
                  confirmation: {
                    counterparty_contact: values.customer_contact,
                    confirmation_reference: values.customer_reference,
                    evidence_reference: values.customer_evidence,
                    note: values.note,
                    document_id: values.customer_document || null,
                  },
                }
              : {}),
          });
        case "reverse_cash":
        case "reverse_credit":
          return await mutate(
            `${modal.kind === "reverse_cash" ? "cash" : "credits"}/${modal.entry.id}/reverse`,
            {
              expected_version: modal.entry.version,
              ...(modal.kind === "reverse_credit"
                ? { expected_bill_version: modal.billVersion }
                : {}),
              reversed_on: values.reversed_on,
              note: values.note,
            },
          );
        case "cash":
          return await mutate("cash", {
            direction,
            ...(direction === "customer_refund"
              ? {
                  confirmation: {
                    counterparty_contact: values.customer_contact,
                    confirmation_reference: values.customer_reference,
                    evidence_reference: values.customer_evidence,
                    note: values.note,
                    document_id: values.customer_document || null,
                  },
                }
              : {}),
            currency: cashCurrency,
            account: values.account,
            external_reference: values.external_reference,
            cleared_on: values.cleared_on,
            note: values.note,
            amount_cents: cents(values.amount, cashCurrency),
            corrects_id: modal.entry?.id || null,
            allocations: Object.entries(selected).map(([target_id, value]) => ({
              target_id,
              amount_cents: cents(value.amount, cashCurrency),
              expected_version: value.version,
            })),
          });
      }
    } catch (problem) {
      setError(problem.message);
    }
  }
  return (
    <div className="finance-workspace">
      <div
        className="board-tabs finance-tabs"
        role="tablist"
        aria-label="Finance workspace"
      >
        {[
          ["carrier_audit", "Carrier audit"],
          ["cancellations", "Cancellation fees"],
          ["billing", "Customer billing"],
          ["cash", "Bank reconciliation"],
          ["journal", "Journal"],
        ].map(([id, label]) => (
          <button
            role="tab"
            aria-selected={section === id}
            key={id}
            className={section === id ? "selected" : ""}
            onClick={() => onSection(id)}
          >
            {label}
          </button>
        ))}
      </div>
      {section === "carrier_audit" && children}
      {section === "cancellations" && (
        <section className="panel" aria-label="Cancellation finance queue">
          <div className="panel-heading">
            <div>
              <h2>Cancellation agreements</h2>
              <p>
                Review separately agreed carrier costs and customer charges.
                Original freight rates do not post when a shipment is cancelled.
              </p>
            </div>
          </div>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Shipment / agreement</th>
                  <th>Counterparty</th>
                  <th>Amount</th>
                  <th>Status</th>
                  <th>Action</th>
                </tr>
              </thead>
              <tbody>
                {(workspace.cancellation_fees || []).map((fee) => (
                  <tr key={fee.id}>
                    <td>
                      <strong>{findLoad(fee.load_id)?.reference}</strong>
                      <small>{fee.number || fee.evidence_reference}</small>
                    </td>
                    <td>
                      {fee.party_snapshot.name}
                      <small>
                        {fee.side === "buy"
                          ? "Carrier cost"
                          : "Customer charge"}
                      </small>
                    </td>
                    <td>{cash(fee.amount_cents, fee.currency)}</td>
                    <td>
                      <State value={fee.status} />
                    </td>
                    <td>
                      <button
                        className="secondary"
                        onClick={() => openLoad(fee.load_id)}
                      >
                        {fee.status === "proposed"
                          ? "Review cancellation fee"
                          : "Open cancelled shipment"}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            {!(workspace.cancellation_fees || []).length && (
              <p className="empty">No cancellation agreements recorded.</p>
            )}
          </div>
        </section>
      )}
      {section === "billing" && (
        <>
          <section className="panel billing-identity">
            <div className="panel-heading">
              <div>
                <h2>Billing identity</h2>
                <p>
                  {workspace.billing_profile?.legal_name ||
                    "Configure your issuing company before issuing customer invoices."}
                </p>
              </div>
              <button
                className="secondary"
                disabled={!canWrite}
                onClick={() => open({ kind: "profile" })}
              >
                Edit billing identity
              </button>
            </div>
          </section>
          <div className="metrics three">
            <section className="metric">
              <span>Customer receivables</span>
              <strong>{totals("receivable_cents")}</strong>
              <p>Positive balances; customer credits remain separate</p>
            </section>
            <section className="metric">
              <span>Past due</span>
              <strong>{totals("overdue_cents")}</strong>
              <p>Outstanding invoices beyond their due date</p>
            </section>
            <section className="metric">
              <span>Customer credit / refund due</span>
              <strong>{totals("customer_credit_cents")}</strong>
              <p>Cleared funds or credits owed back to customers</p>
            </section>
          </div>
          <section className="panel">
            <div className="panel-heading">
              <div>
                <h2>Customer invoices</h2>
                <p>
                  Bill the agreed sell rate and preserve the issued customer and
                  shipment snapshot.
                </p>
              </div>
              <button
                className="primary"
                disabled={!canWrite || !draftLoads.length}
                onClick={() => open({ kind: "bill" })}
              >
                <Plus size={14} />
                Draft customer invoice
              </button>
            </div>
            <div className="table-scroll">
              <table className="customer-invoice-table">
                <thead>
                  <tr>
                    <th>Invoice / customer</th>
                    <th>Shipment</th>
                    <th>Amount</th>
                    <th>Received / credits</th>
                    <th>Due / balance</th>
                    <th>Status</th>
                    <th>Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {bills.map((bill) => (
                    <tr key={bill.id}>
                      <td>
                        <strong>{bill.number}</strong>
                        {(workspace.cancellation_fees || []).some(
                          (fee) => fee.target_id === bill.id,
                        ) && (
                          <small>
                            Cancellation invoice · shipment not delivered
                          </small>
                        )}
                        <small>{bill.snapshot.customer.name}</small>
                        {bill.supplement && (
                          <>
                            <small>
                              Supplement to {bill.supplement.parent_number}
                            </small>
                            <details className="customer-agreement">
                              <summary>Customer agreement</summary>
                              <p>{bill.supplement.description}</p>
                              <p>
                                Recorded by {bill.created_by} ·{" "}
                                {new Date(bill.created_at).toLocaleString()}
                              </p>
                              <p>
                                {bill.supplement.counterparty_contact} ·{" "}
                                {bill.supplement.confirmation_reference}
                              </p>
                              <p>
                                Evidence: {bill.supplement.evidence_reference}
                              </p>
                              {bill.void_reason && (
                                <p>Void reason: {bill.void_reason}</p>
                              )}
                              {bill.supplement.document_id && (
                                <a
                                  href={identityHref(
                                    `/api/freight/documents/${bill.supplement.document_id}/content`,
                                  )}
                                >
                                  Download agreement evidence
                                </a>
                              )}
                              {bill.supplement.corrects_id && (
                                <p>
                                  Corrects{" "}
                                  {
                                    bills.find(
                                      (v) =>
                                        v.id === bill.supplement.corrects_id,
                                    )?.number
                                  }
                                </p>
                              )}
                              {bill.supplement.corrected_by_id && (
                                <p>
                                  Corrected by{" "}
                                  {
                                    bills.find(
                                      (v) =>
                                        v.id ===
                                        bill.supplement.corrected_by_id,
                                    )?.number
                                  }
                                </p>
                              )}
                            </details>
                          </>
                        )}
                      </td>
                      <td>
                        <button
                          className="text-link"
                          onClick={() => openLoad(bill.load_id)}
                        >
                          {bill.snapshot.load_reference}
                        </button>
                      </td>
                      <td>
                        {cash(bill.snapshot.amount_cents, billCurrency(bill))}
                      </td>
                      <td>
                        {cash(bill.totals.received_cents, billCurrency(bill))}
                        <small>
                          {cash(bill.totals.credited_cents, billCurrency(bill))}{" "}
                          credits
                          {!!bill.totals.refunded_cents && (
                            <span>
                              {" "}
                              ·{" "}
                              {cash(
                                bill.totals.refunded_cents,
                                billCurrency(bill),
                              )}{" "}
                              refunded
                            </span>
                          )}
                        </small>
                      </td>
                      <td>
                        {bill.due_date}
                        <small
                          className={bill.totals.overdue_days ? "red" : ""}
                        >
                          {bill.totals.balance_cents < 0
                            ? `${cash(bill.totals.refund_due_cents, billCurrency(bill))} customer refund due`
                            : `${cash(bill.totals.balance_cents, billCurrency(bill))} due`}
                          {bill.totals.overdue_days
                            ? ` · ${bill.totals.overdue_days} days overdue`
                            : ""}
                        </small>
                      </td>
                      <td>
                        <State value={bill.status} />
                      </td>
                      <td>
                        <div className="finance-actions">
                          <a
                            className="secondary"
                            href={identityHref(
                              `/api/freight/bills/${bill.id}/statement`,
                            )}
                            aria-label={`Download invoice ${bill.number}`}
                          >
                            <Download size={13} />
                            Download invoice
                          </a>
                          {bill.status === "draft" && (
                            <>
                              <button
                                className="secondary"
                                disabled={!canWrite || busy}
                                onClick={() => open({ kind: "bill", bill })}
                              >
                                Edit draft
                              </button>
                              <button
                                className="secondary"
                                disabled={
                                  !canWrite ||
                                  busy ||
                                  !findLoad(bill.load_id)?.pod_ready ||
                                  !!findLoad(bill.load_id)?.commercial
                                    ?.pending_sell
                                }
                                onClick={() => open({ kind: "issue", bill })}
                              >
                                Issue invoice
                              </button>
                            </>
                          )}
                          {bill.issued_at &&
                            bill.status !== "void" &&
                            bill.totals.balance_cents > 0 && (
                              <>
                                <button
                                  className="secondary"
                                  disabled={!canWrite || busy}
                                  onClick={() =>
                                    cashModal("receipt", null, bill)
                                  }
                                >
                                  Record receipt
                                </button>
                              </>
                            )}
                          {bill.issued_at &&
                            bill.status !== "void" &&
                            bill.totals.remaining_creditable_cents > 0 && (
                              <button
                                className="secondary"
                                disabled={!canWrite || busy}
                                onClick={() => open({ kind: "credit", bill })}
                              >
                                Credit note
                              </button>
                            )}
                          {bill.totals.refund_due_cents > 0 && (
                            <button
                              className="secondary"
                              disabled={!canWrite || busy}
                              onClick={() =>
                                cashModal("customer_refund", null, bill)
                              }
                            >
                              Record customer refund
                            </button>
                          )}
                          {bill.status !== "void" &&
                            bill.issued_at &&
                            !(workspace.cancellation_fees || []).some(
                              (fee) => fee.target_id === bill.id,
                            ) &&
                            !bill.supplement && (
                              <button
                                className="secondary"
                                disabled={
                                  !canWrite ||
                                  busy ||
                                  !findLoad(bill.load_id)?.pod_ready
                                }
                                onClick={() =>
                                  open({ kind: "supplement", bill })
                                }
                              >
                                Draft supplement
                              </button>
                            )}
                          {bill.status === "void" &&
                            bill.supplement &&
                            !bill.supplement.corrected_by_id && (
                              <button
                                className="secondary"
                                disabled={
                                  !canWrite ||
                                  busy ||
                                  bills.find(
                                    (v) =>
                                      v.id === bill.supplement.parent_bill_id,
                                  )?.status === "void"
                                }
                                onClick={() =>
                                  open({
                                    kind: "supplement",
                                    bill: bills.find(
                                      (v) =>
                                        v.id === bill.supplement.parent_bill_id,
                                    ),
                                    corrects: bill,
                                  })
                                }
                              >
                                Correct void supplement
                              </button>
                            )}
                          {bill.status !== "void" &&
                            !(workspace.cancellation_fees || []).some(
                              (fee) => fee.target_id === bill.id,
                            ) &&
                            bill.totals.received_cents ===
                              bill.totals.refunded_cents &&
                            !bill.totals.credited_cents && (
                              <button
                                className="text-link"
                                disabled={!canWrite || busy}
                                onClick={() => open({ kind: "void", bill })}
                              >
                                Void invoice
                              </button>
                            )}
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {!bills.length && (
                <div className="empty">
                  <h3>No customer invoices yet</h3>
                  <p>
                    Draft an invoice for a delivered shipment. Reviewed POD is
                    required to issue it.
                  </p>
                </div>
              )}
            </div>
          </section>
          <section className="panel finance-secondary">
            <div className="panel-heading">
              <h2>Credit notes</h2>
            </div>
            <div className="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>Credit note / invoice</th>
                    <th>Amount</th>
                    <th>Reason</th>
                    <th>Status</th>
                    <th>Action</th>
                  </tr>
                </thead>
                <tbody>
                  {credits.map((credit) => (
                    <tr key={credit.id}>
                      <td>
                        <strong>{credit.number}</strong>
                        <small>
                          {bills.find((v) => v.id === credit.bill_id)?.number}
                        </small>
                      </td>
                      <td>
                        {cash(
                          credit.amount_cents,
                          billCurrency(
                            bills.find((v) => v.id === credit.bill_id),
                          ),
                        )}
                        <small>
                          Issued{" "}
                          {credit.issued_on || credit.created_at.slice(0, 10)}
                        </small>
                      </td>
                      <td className="finance-note">
                        {credit.reason}
                        <CustomerConfirmation
                          value={credit.confirmation}
                          label="Customer confirmation"
                        />
                      </td>
                      <td>
                        <State
                          value={credit.reversal ? "reversed" : "posted"}
                        />
                      </td>
                      <td>
                        <button
                          className="secondary"
                          disabled={!canWrite || busy || !!credit.reversal}
                          onClick={() =>
                            open({ kind: "reverse_credit", entry: credit })
                          }
                        >
                          Reverse credit
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {!credits.length && (
                <p className="empty">No credit notes recorded.</p>
              )}
            </div>
          </section>
        </>
      )}
      {section === "cash" && (
        <>
          <section className="panel">
            <div className="panel-heading">
              <div>
                <h2>External bank reconciliation</h2>
                <p>
                  Record transactions confirmed in your bank or accounting
                  system. No transfers are initiated here.
                </p>
              </div>
              <button
                className="primary"
                disabled={!canWrite}
                onClick={() => cashModal("receipt")}
              >
                <Plus size={14} />
                Record bank transaction
              </button>
            </div>
            <div className="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>Reference / account</th>
                    <th>Direction / cleared</th>
                    <th>Amount</th>
                    <th>Allocations</th>
                    <th>Status</th>
                    <th>Actions</th>
                  </tr>
                </thead>
                <tbody>
                  {[...cashEntries].reverse().map((entry) => (
                    <tr key={entry.id}>
                      <td>
                        <strong>{entry.external_reference}</strong>
                        <small>
                          {entry.account} · {entry.recorded_by}
                        </small>
                        <details className="cash-evidence">
                          <summary>Evidence and history</summary>
                          <p>{entry.note}</p>
                          <small>Record {entry.id}</small>
                          {entry.reversal && (
                            <p>
                              Reversed {entry.reversal.reversed_on} by{" "}
                              {entry.reversal.actor}: {entry.reversal.note}
                            </p>
                          )}
                          {entry.corrects_id && (
                            <small>Corrects {entry.corrects_id}</small>
                          )}
                          {entry.corrected_by_id && (
                            <small>Corrected by {entry.corrected_by_id}</small>
                          )}
                        </details>
                      </td>
                      <td>
                        {title(entry.direction)}
                        <CustomerConfirmation
                          value={entry.customer_confirmation}
                          label="Customer refund confirmation"
                        />
                        <small>{entry.cleared_on}</small>
                      </td>
                      <td>{cash(entry.amount_cents, entry.currency)}</td>
                      <td>
                        {entry.allocations.map((allocation) => (
                          <small key={allocation.target_id}>
                            {findLoad(allocation.load_id)?.reference} ·{" "}
                            {cash(allocation.amount_cents, entry.currency)}
                          </small>
                        ))}
                      </td>
                      <td>
                        <State
                          value={entry.reversal ? "reversed" : "recorded"}
                        />
                        {entry.corrects_id && (
                          <small>Corrected allocation</small>
                        )}
                      </td>
                      <td>
                        <div className="finance-actions">
                          <button
                            className="secondary"
                            disabled={!canWrite || busy || !!entry.reversal}
                            onClick={() =>
                              open({ kind: "reverse_cash", entry })
                            }
                          >
                            Reverse reconciliation
                          </button>
                          {entry.reversal && !entry.corrected_by_id && (
                            <button
                              className="secondary"
                              disabled={!canWrite || busy}
                              onClick={() => cashModal(entry.direction, entry)}
                            >
                              Correct reversed entry
                            </button>
                          )}
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {!cashEntries.length && (
                <div className="empty">
                  <h3>No cleared transactions recorded</h3>
                  <p>
                    Allocate a bank receipt to customer invoices, or a bank
                    payout to prepared carrier settlements.
                  </p>
                </div>
              )}
            </div>
          </section>
          <section className="panel finance-secondary">
            <div className="panel-heading">
              <h2>Carrier settlement balances</h2>
              <span>{totals("prepared_payable_cents")} outstanding</span>
            </div>
            <div className="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>Shipment / carrier</th>
                    <th>Original / adjusted amount</th>
                    <th>Payable / carrier credit</th>
                    <th>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {workspace.settlements.map((settlement) => (
                    <tr key={settlement.id}>
                      <td>
                        <strong>
                          {findLoad(settlement.load_id)?.reference}
                        </strong>
                        <small>
                          {
                            workspace.carriers.find(
                              (v) => v.id === settlement.carrier_id,
                            )?.name
                          }
                        </small>
                      </td>
                      <td>
                        {cash(
                          settlement.amount_cents,
                          loadCurrency(settlement.load_id),
                        )}
                        <small>
                          Adjusted{" "}
                          {cash(
                            settlement.total_amount_cents ??
                              settlement.amount_cents,
                            loadCurrency(settlement.load_id),
                          )}
                        </small>
                      </td>
                      <td>
                        {settlement.balance_cents < 0
                          ? `${cash(-settlement.balance_cents, loadCurrency(settlement.load_id))} carrier credit`
                          : cash(
                              settlement.balance_cents,
                              loadCurrency(settlement.load_id),
                            )}
                        {!!settlement.credit_application_cents && (
                          <small>
                            {settlement.credit_application_cents > 0
                              ? "Net credit applied elsewhere"
                              : "Net credit applied here"}{" "}
                            {cash(
                              Math.abs(settlement.credit_application_cents),
                              loadCurrency(settlement.load_id),
                            )}
                          </small>
                        )}
                      </td>
                      <td>
                        <State value={settlement.status} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        </>
      )}
      {section === "cash" && (
        <CarrierCreditApplications
          workspace={workspace}
          onChanged={onChanged}
          openLoad={openLoad}
        />
      )}
      {section === "journal" && (
        <>
          <section className="panel">
            <div className="panel-heading">
              <div>
                <h2>Freight subledger</h2>
                <p>
                  Balanced postings per currency for billing, carrier
                  liabilities, credits, and reconciled cash.
                </p>
              </div>
              <a
                className="secondary"
                href={identityHref("/api/freight/finance/journal.csv")}
              >
                <Download size={14} />
                Export journal CSV
              </a>
            </div>
            <div className="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>Account</th>
                    <th>Debits</th>
                    <th>Credits</th>
                    <th>Net debit balance</th>
                  </tr>
                </thead>
                <tbody>
                  {finance.accounts?.map((row) => (
                    <tr key={`${row.account}:${row.currency}`}>
                      <td>
                        {title(row.account)}
                        {currencyCode(row.currency) !== "USD" && (
                          <small>{row.currency}</small>
                        )}
                      </td>
                      <td>{cash(row.debit_cents, row.currency)}</td>
                      <td>{cash(row.credit_cents, row.currency)}</td>
                      <td>{cash(row.net_debit_cents, row.currency)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
          <section className="panel finance-secondary">
            <div className="panel-heading">
              <h2>Posting history</h2>
              <span>{workspace.journal?.length || 0} entries</span>
            </div>
            <div className="journal-list">
              {[...(workspace.journal || [])].reverse().map((entry) => (
                <details key={entry.id}>
                  <summary>
                    {entry.posted_on} · {title(entry.source_kind)} ·{" "}
                    {entry.memo}
                  </summary>
                  <p>
                    {entry.actor} · {entry.id}
                    {entry.reversal_of
                      ? ` · Reverses ${entry.reversal_of}`
                      : ""}
                  </p>
                  <div className="table-scroll">
                    <table>
                      <thead>
                        <tr>
                          <th>Shipment</th>
                          <th>Account</th>
                          <th>Debit</th>
                          <th>Credit</th>
                        </tr>
                      </thead>
                      <tbody>
                        {entry.lines.map((line, index) => (
                          <tr key={index}>
                            <td>{findLoad(line.load_id)?.reference}</td>
                            <td>{title(line.account)}</td>
                            <td>{cash(line.debit_cents, entry.currency)}</td>
                            <td>{cash(line.credit_cents, entry.currency)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </details>
              ))}
              {!workspace.journal?.length && (
                <div className="empty">
                  Postings appear when invoices are issued or carrier invoices
                  are approved.
                </div>
              )}
            </div>
          </section>
        </>
      )}
      {modal && (
        <Modal
          title={
            {
              bill: modal.bill
                ? "Edit draft customer invoice"
                : "Draft customer invoice",
              issue: "Issue customer invoice",
              void: "Void customer invoice",
              credit: "Create customer credit note",
              supplement: modal.corrects
                ? "Correct void customer supplement"
                : "Draft supplemental customer invoice",
              cash: modal.entry
                ? "Correct reversed bank entry"
                : "Reconcile cleared bank transaction",
              reverse_cash: "Reverse cash reconciliation",
              reverse_credit: "Reverse credit note",
              profile: "Billing issuer identity",
            }[modal.kind]
          }
          close={() => {
            if (!busy) setModal(null);
          }}
          wide={modal.kind === "cash"}
        >
          <form className="modal-form" onSubmit={submit}>
            {(error || profileChanged) && (
              <p className="form-error" role="alert">
                {error}
                {profileChanged && error !== identityChanged && (
                  <>
                    {error ? " " : ""}
                    {identityChanged}
                  </>
                )}
              </p>
            )}
            {modal.kind === "profile" && (
              <>
                <Field
                  title="Issuing legal name"
                  name="legal_name"
                  maxLength={200}
                  defaultValue={modal.profile?.legal_name || ""}
                />
                <Field title="Issuing address">
                  <textarea
                    aria-label="Issuing address"
                    name="address"
                    required
                    maxLength={1000}
                    defaultValue={modal.profile?.address || ""}
                  />
                </Field>
                <Field
                  title="Billing contact"
                  name="contact"
                  maxLength={200}
                  defaultValue={modal.profile?.contact || ""}
                />
                <Field title="Payment instructions">
                  <textarea
                    aria-label="Payment instructions"
                    name="payment_instructions"
                    required
                    maxLength={2000}
                    defaultValue={modal.profile?.payment_instructions || ""}
                  />
                </Field>
                <p>
                  Newly issued invoices capture these details. Existing issued
                  invoice snapshots remain unchanged.
                </p>
              </>
            )}
            {modal.kind === "bill" && (
              <>
                {!modal.bill && (
                  <Field title="Delivered shipment">
                    <select
                      aria-label="Delivered shipment"
                      name="load_id"
                      required
                      onChange={(event) =>
                        setBillToAddress(
                          workspace.customers.find(
                            (customer) =>
                              customer.id ===
                              findLoad(event.target.value)?.customer_id,
                          )?.billing_address || "",
                        )
                      }
                    >
                      {draftLoads.map((load) => (
                        <option key={load.id} value={load.id}>
                          {load.reference} ·{" "}
                          {
                            workspace.customers.find(
                              (v) => v.id === load.customer_id,
                            )?.name
                          }{" "}
                          ·{" "}
                          {cash(
                            load.commercial?.sell?.total_amount_cents ??
                              load.sell_cents,
                            load.currency,
                          )}
                        </option>
                      ))}
                    </select>
                  </Field>
                )}
                <Field
                  title="Customer invoice number"
                  name="number"
                  defaultValue={modal.bill?.number || `BILL-${Date.now()}`}
                  maxLength={60}
                />
                <Field title="Bill-to address">
                  <textarea
                    aria-label="Bill-to address"
                    name="bill_to_address"
                    required
                    maxLength={1000}
                    value={billToAddress}
                    onChange={(event) => setBillToAddress(event.target.value)}
                  />
                </Field>
                <Field
                  title="Invoice due date"
                  name="due_date"
                  type="date"
                  min={today()}
                  defaultValue={
                    modal.bill?.due_date ||
                    new Date(Date.now() + 30 * 86400000)
                      .toISOString()
                      .slice(0, 10)
                  }
                />
                <p>
                  {modal.bill?.supplement
                    ? "This draft retains its documented additional amount. To correct the amount or agreement, void it and create an explicit correction."
                    : "The invoice uses the contracted sell amount. Issuance freezes its customer and shipment snapshot."}
                </p>
              </>
            )}
            {modal.kind === "supplement" && (
              <>
                <p>
                  Additional billing for {modal.bill.number} ·{" "}
                  {modal.bill.snapshot.customer.name}. Record the customer's
                  actual agreement. Issuance posts a separate receivable and
                  keeps the original invoice intact.
                </p>
                <Field
                  title="Supplement invoice number"
                  name="number"
                  maxLength={60}
                />
                <Field
                  title={`Supplement amount (${billCurrency(modal.bill)})`}
                  name="amount"
                  inputMode="decimal"
                  defaultValue={
                    modal.corrects
                      ? moneyInput(
                          modal.corrects.snapshot.amount_cents,
                          billCurrency(modal.bill),
                        )
                      : ""
                  }
                />
                <Field
                  title="Invoice due date"
                  name="due_date"
                  type="date"
                  min={today()}
                  defaultValue={new Date(Date.now() + 30 * 86400000)
                    .toISOString()
                    .slice(0, 10)}
                />
                <Field
                  title="Customer charge description"
                  name="description"
                  maxLength={200}
                  defaultValue={modal.corrects?.supplement.description || ""}
                />
                <Field
                  title="Customer agreement contact"
                  name="counterparty_contact"
                  maxLength={200}
                />
                <Field
                  title="Customer confirmation reference"
                  name="confirmation_reference"
                  maxLength={200}
                />
                <Field
                  title="Customer agreement reference"
                  name="evidence_reference"
                  maxLength={200}
                  defaultValue={
                    modal.corrects?.supplement.evidence_reference || ""
                  }
                  readOnly={!!modal.corrects}
                />
                <Field title="Stored agreement evidence (optional)">
                  <select
                    name="document_id"
                    aria-label="Stored agreement evidence (optional)"
                    defaultValue=""
                  >
                    <option value="">External reference recorded above</option>
                    {(workspace.documents || [])
                      .filter(
                        (v) =>
                          v.load_id === modal.bill.load_id &&
                          v.status !== "rejected",
                      )
                      .map((v) => (
                        <option key={v.id} value={v.id}>
                          {v.filename} · {v.sha256.slice(0, 12)}
                        </option>
                      ))}
                  </select>
                </Field>
              </>
            )}
            {modal.kind === "issue" && (
              <div>
                <p>
                  Issue {modal.bill.number} for{" "}
                  {cash(
                    modal.bill.snapshot.amount_cents,
                    billCurrency(modal.bill),
                  )}
                  . This posts the receivable and revenue. Download the invoice
                  for your billing process; no message is sent automatically.
                </p>
                {modal.bill.supplement && (
                  <p>
                    Verify customer agreement:{" "}
                    {modal.bill.supplement.description} ·{" "}
                    {modal.bill.supplement.counterparty_contact} ·{" "}
                    {modal.bill.supplement.confirmation_reference}. Evidence:{" "}
                    {modal.bill.supplement.evidence_reference}.
                  </p>
                )}
              </div>
            )}
            {modal.kind === "credit" && (
              <>
                <Field
                  title="Credit note number"
                  name="number"
                  defaultValue={`CR-${Date.now()}`}
                  maxLength={60}
                />
                <Field
                  title="Credit issue date"
                  name="credit_issued_on"
                  type="date"
                  defaultValue={today()}
                  min={modal.bill.issued_at?.slice(0, 10)}
                  max={today()}
                />
                <Field
                  title={`Credit amount (${billCurrency(modal.bill)})`}
                  name="amount"
                  type="number"
                  min={amountStep(billCurrency(modal.bill))}
                  step={amountStep(billCurrency(modal.bill))}
                  max={moneyInput(
                    modal.bill.totals.remaining_creditable_cents,
                    billCurrency(modal.bill),
                  )}
                />
                <p>
                  A credit reduces the invoice amount and can create a customer
                  refund balance after payment. Reconcile an actual cleared
                  refund separately; the original receipt stays recorded.
                </p>
              </>
            )}
            {(modal.kind === "credit" ||
              (modal.kind === "cash" && direction === "customer_refund")) && (
              <fieldset className="customer-refund-proof">
                <legend>Customer agreement and refund evidence</legend>
                <p>
                  Record the actual customer confirmation. A bank reconciliation
                  records cleared funds; it does not send a refund.
                </p>
                <Field
                  title="Actual customer contact"
                  name="customer_contact"
                  required={
                    modal.kind === "cash" ||
                    modal.bill.totals.received_cents > 0
                  }
                  maxLength={200}
                  defaultValue={
                    modal.entry?.customer_confirmation?.counterparty_contact ||
                    ""
                  }
                />
                <Field
                  title="Customer confirmation reference"
                  name="customer_reference"
                  required={
                    modal.kind === "cash" ||
                    modal.bill.totals.received_cents > 0
                  }
                  maxLength={200}
                  defaultValue={
                    modal.entry?.customer_confirmation
                      ?.confirmation_reference || ""
                  }
                />
                <Field
                  title="Customer agreement or bank evidence reference"
                  name="customer_evidence"
                  required={
                    modal.kind === "cash" ||
                    modal.bill.totals.received_cents > 0
                  }
                  maxLength={200}
                  defaultValue={
                    modal.entry?.customer_confirmation?.evidence_reference || ""
                  }
                />
                <Field title="Customer evidence file (optional)">
                  <select
                    name="customer_document"
                    defaultValue={
                      modal.entry?.customer_confirmation?.document_id || ""
                    }
                  >
                    <option value="">Evidence held externally</option>
                    {(workspace.documents || [])
                      .filter(
                        (d) =>
                          d.status !== "rejected" &&
                          (modal.kind === "credit"
                            ? d.load_id === modal.bill.load_id
                            : Object.keys(selected).some(
                                (id) =>
                                  bills.find((b) => b.id === id)?.load_id ===
                                  d.load_id,
                              )),
                      )
                      .map((d) => (
                        <option key={d.id} value={d.id}>
                          {d.filename}
                        </option>
                      ))}
                  </select>
                </Field>
              </fieldset>
            )}
            {["void", "credit", "reverse_cash", "reverse_credit"].includes(
              modal.kind,
            ) && <Field title="Finance reason" name="note" maxLength={200} />}
            {["reverse_cash", "reverse_credit"].includes(modal.kind) && (
              <>
                <Field
                  title="Reversal date"
                  name="reversed_on"
                  type="date"
                  max={today()}
                  defaultValue={today()}
                />
                <p>
                  This appends a reversing posting and reopens the affected
                  balances. It does not reverse a bank transfer.
                </p>
              </>
            )}
            {modal.kind === "cash" && (
              <>
                <Field title="Transaction direction">
                  <select
                    aria-label="Transaction direction"
                    value={direction}
                    disabled={!!modal.entry}
                    onChange={(event) => {
                      setDirection(event.target.value);
                      setParty("");
                      setSelected({});
                    }}
                  >
                    <option value="receipt">Customer receipt</option>
                    <option value="payout">Carrier payout</option>
                    <option value="customer_refund">
                      Customer refund paid
                    </option>
                    <option value="carrier_refund">
                      Carrier refund received
                    </option>
                  </select>
                </Field>
                <div className="form-grid">
                  <Field
                    title="Bank account label"
                    name="account"
                    key={cashCurrency}
                    defaultValue={
                      modal.entry?.account ||
                      `operating-${cashCurrency.toLowerCase()}`
                    }
                    readOnly={!!modal.entry}
                    maxLength={200}
                  />
                  <Field
                    title="Cleared bank reference"
                    name="external_reference"
                    defaultValue={modal.entry?.external_reference}
                    readOnly={!!modal.entry}
                    maxLength={200}
                  />
                  <Field
                    title="Cleared date"
                    type="date"
                    name="cleared_on"
                    defaultValue={modal.entry?.cleared_on || today()}
                    max={today()}
                  />
                  <Field
                    title={`Bank amount (${cashCurrency})`}
                    name="amount"
                    type="number"
                    min={amountStep(cashCurrency)}
                    step={amountStep(cashCurrency)}
                    defaultValue={
                      modal.entry
                        ? moneyInput(modal.entry.amount_cents, cashCurrency)
                        : Object.values(selected).length
                          ? selectedTotal()
                          : ""
                    }
                  />
                </div>
                <Field title="Bank currency">
                  <select
                    aria-label="Bank currency"
                    value={cashCurrency}
                    disabled={!!modal.entry}
                    onChange={(event) => {
                      setCashCurrency(event.target.value);
                      setParty("");
                      setSelected({});
                    }}
                  >
                    {[
                      ...new Set([
                        "USD",
                        cashCurrency,
                        ...targets.map((t) => t.currency),
                      ]),
                    ]
                      .sort()
                      .map((code) => (
                        <option key={code}>{code}</option>
                      ))}
                  </select>
                </Field>
                <Field title="Counterparty">
                  <select
                    aria-label="Counterparty"
                    required
                    value={party}
                    onChange={(event) => {
                      setParty(event.target.value);
                      setSelected({});
                    }}
                  >
                    <option value="">
                      Choose a{" "}
                      {["receipt", "customer_refund"].includes(direction)
                        ? "customer"
                        : "carrier"}
                    </option>
                    {parties
                      .filter((v) =>
                        targets.some(
                          (target) =>
                            target.party === v.id &&
                            target.currency === cashCurrency,
                        ),
                      )
                      .map((value) => (
                        <option key={value.id} value={value.id}>
                          {value.name}
                        </option>
                      ))}
                  </select>
                </Field>
                <div className="table-scroll allocation-table">
                  <table>
                    <thead>
                      <tr>
                        <th>Allocate</th>
                        <th>Invoice / shipment</th>
                        <th>Outstanding</th>
                        <th>Apply ({cashCurrency})</th>
                      </tr>
                    </thead>
                    <tbody>
                      {targets
                        .filter(
                          (v) =>
                            v.party === party && v.currency === cashCurrency,
                        )
                        .map((target) => (
                          <tr key={target.id}>
                            <td>
                              <input
                                aria-label={`Allocate ${target.label}`}
                                type="checkbox"
                                checked={!!selected[target.id]}
                                onChange={(event) =>
                                  setSelected((current) => {
                                    const next = { ...current };
                                    if (event.target.checked)
                                      next[target.id] = {
                                        amount: moneyInput(
                                          target.balance,
                                          cashCurrency,
                                        ),
                                        version: target.version,
                                      };
                                    else delete next[target.id];
                                    return next;
                                  })
                                }
                              />
                            </td>
                            <td>
                              {target.label}
                              <small>{target.load}</small>
                            </td>
                            <td>{cash(target.balance, cashCurrency)}</td>
                            <td>
                              <input
                                aria-label={`Allocation amount ${target.label}`}
                                type="number"
                                min={amountStep(cashCurrency)}
                                step={amountStep(cashCurrency)}
                                max={moneyInput(target.balance, cashCurrency)}
                                disabled={!selected[target.id]}
                                value={selected[target.id]?.amount || ""}
                                onChange={(event) =>
                                  setSelected((current) => ({
                                    ...current,
                                    [target.id]: {
                                      ...current[target.id],
                                      amount: event.target.value,
                                    },
                                  }))
                                }
                              />
                            </td>
                          </tr>
                        ))}
                    </tbody>
                  </table>
                </div>
                <p>
                  Allocate the exact cleared amount to one or more obligations
                  for this counterparty. Previously used bank references cannot
                  be posted twice.
                </p>
                <Field
                  title="Reconciliation note"
                  name="note"
                  maxLength={200}
                />
              </>
            )}
            <div className="form-footer">
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => setModal(null)}
              >
                Cancel
              </button>
              <button
                className="primary"
                disabled={
                  busy ||
                  !canWrite ||
                  (modal.kind === "cash" && !Object.keys(selected).length)
                }
              >
                {busy
                  ? "Saving…"
                  : {
                      bill: modal.bill ? "Save draft" : "Create draft",
                      issue: "Confirm issue",
                      void: "Confirm void",
                      credit: "Post credit note",
                      supplement: "Create supplemental draft",
                      cash: "Record reconciliation",
                      reverse_cash: "Confirm reversal",
                      reverse_credit: "Confirm reversal",
                      profile: "Save billing identity",
                    }[modal.kind]}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </div>
  );
}
