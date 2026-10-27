import { useFreightIdentity } from "./FreightIdentity";
import { useRef, useState } from "react";
import { ReceiptText, Plus } from "lucide-react";
import { formatMoney, moneyInput, parseMoney } from "../lib/money.mjs";
const label = (value) =>
  value.replaceAll("_", " ").replace(/\b\w/g, (c) => c.toUpperCase());
const today = () => new Date().toISOString().slice(0, 10);
export function CarrierPayables({ load, workspace, onChanged }) {
  const currency = load.currency || "USD";
  const money = (cents) => formatMoney(cents, currency);
  const { fetch, href: identityHref } = useFreightIdentity();
  const [form, setForm] = useState(null),
    [kind, setKind] = useState("supplemental_invoice"),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const pending = useRef(null);
  const canFinance = workspace.access.capabilities.includes("finance.write"),
    invoice = workspace.invoices.find((v) => v.load_id === load.id),
    settlement = workspace.settlements.find((v) => v.load_id === load.id),
    adjustments = (workspace.carrier_adjustments || []).filter(
      (v) => v.load_id === load.id,
    ),
    documents = (workspace.carrier_documents || []).filter(
      (v) => v.load_id === load.id,
    );
  if (!invoice && !documents.length) return null;
  const posted = adjustments
      .filter((v) => v.status === "posted")
      .reduce((a, v) => a + v.amount_cents, 0),
    total = (invoice?.amount_cents || 0) + posted;
  function open(action, record = null) {
    setError("");
    const initial =
      record?.kind ||
      (record?.amount_cents < 0
        ? ""
        : record?.number?.toUpperCase() === invoice?.number.toUpperCase()
          ? "invoice_revision"
          : "supplemental_invoice");
    setKind(initial);
    setForm({ action, record, version: load.version, kind: initial });
  }
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const values = Object.fromEntries(new FormData(event.currentTarget));
      let path,
        body = { ...values, expected_load_version: form.version };
      if (["create", "classify", "correct"].includes(form.action)) {
        path = `loads/${load.id}/carrier-adjustments`;
        body.invoice_id = invoice.id;
        body.document_amount_cents = parseMoney(values.amount, load.currency, {
          min: 0,
        });
        delete body.amount;
        body.document_id = values.document_id || null;
        body.corrects_id = form.action === "correct" ? form.record.id : null;
        body.carrier_document_id =
          form.action === "classify"
            ? form.record.id
            : form.action === "correct"
              ? form.record.carrier_document_id
              : null;
      } else if (["dismiss", "apply-revision"].includes(form.action)) {
        path = `carrier-documents/${form.record.id}/${form.action}`;
        body.expected_version = form.record.version;
      } else {
        path = `carrier-adjustments/${form.record.id}/${form.action === "reverse" ? "reverse" : "review"}`;
        body.expected_version = form.record.version;
        if (form.action !== "reverse")
          body.decision = form.action === "approve" ? "approved" : "rejected";
      }
      const fingerprint = JSON.stringify([path, body]);
      if (pending.current?.fingerprint !== fingerprint)
        pending.current = { fingerprint, key: crypto.randomUUID() };
      const response = await fetch(`/api/freight/${path}`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Idempotency-Key": pending.current.key,
        },
        body: JSON.stringify(body),
      });
      const result = await response.json();
      if (!response.ok)
        throw new Error(
          result.error?.message || "Carrier payable update failed",
        );
      await onChanged();
      pending.current = null;
      setForm(null);
    } catch (problem) {
      setError(problem.message);
    } finally {
      setBusy(false);
    }
  }
  const drafts = ["create", "classify", "correct"].includes(form?.action),
    source = form?.action === "classify" ? form.record : null,
    boundSource =
      source ||
      documents.find((doc) => doc.id === form?.record?.carrier_document_id),
    negativeSource = !!boundSource && boundSource.amount_cents < 0;
  return (
    <section
      className="carrier-payables"
      aria-label="Carrier payable adjustments"
    >
      <div className="charge-heading">
        <h3>
          <ReceiptText size={17} /> Carrier payable adjustments
        </h3>
        {canFinance && invoice?.status === "approved" && (
          <button
            className="secondary"
            disabled={busy}
            onClick={() => open("create")}
          >
            <Plus size={14} /> Record carrier adjustment
          </button>
        )}
      </div>
      <p className="section-note">
        Keep the approved invoice and bank history intact. Classify actual
        supplemental invoices, revised totals, or credit memos before posting a
        separate difference.
      </p>
      {invoice?.status === "approved" && (
        <div className="commercial-totals">
          <div>
            <span>Original approved invoice</span>
            <strong>{money(invoice.amount_cents)}</strong>
            <small>{invoice.number}</small>
          </div>
          <div>
            <span>Posted adjustments</span>
            <strong>{money(posted)}</strong>
            <small>Recognized carrier cost {money(total)}</small>
          </div>
          <div>
            <span>
              {settlement?.balance_cents < 0
                ? "Carrier credit balance"
                : "Prepared payable remaining"}
            </span>
            <strong>
              {settlement
                ? money(Math.abs(settlement.balance_cents))
                : "Not prepared"}
            </strong>
            <small>
              {settlement?.balance_cents < 0
                ? "Reconcile an actual carrier refund in Bank reconciliation"
                : "Bank reconciliation records actual external transactions"}
            </small>
          </div>
        </div>
      )}
      {documents.map((doc) => (
        <article
          className="carrier-document"
          key={doc.id}
          aria-label={`Carrier document ${doc.number}`}
        >
          <div className="charge-row">
            <strong>
              EDI 210 · {doc.number} · {money(doc.amount_cents)}
            </strong>
            <span className={`badge ${doc.status}`}>{label(doc.status)}</span>
          </div>
          <p>
            Carrier-reported total. It may replace an earlier invoice or
            represent additional cost; receipt creates no payable.
          </p>
          <details>
            <summary>Original authenticated invoice evidence</summary>
            <pre>{JSON.stringify(doc.event, null, 2)}</pre>
            {doc.resolution && (
              <p>
                {doc.resolution.note} · {doc.resolution.recorded_by}
              </p>
            )}
          </details>
          {canFinance && doc.status === "pending" && (
            <div className="charge-actions">
              {invoice?.status === "approved" ? (
                <button
                  className="secondary"
                  disabled={busy}
                  onClick={() => open("classify", doc)}
                >
                  Classify carrier invoice
                </button>
              ) : (
                invoice && (
                  <button
                    className="secondary"
                    disabled={busy}
                    onClick={() => open("apply-revision", doc)}
                  >
                    Apply actual invoice revision
                  </button>
                )
              )}
              <button
                className="text-link"
                disabled={busy}
                onClick={() => open("dismiss", doc)}
              >
                Dismiss with reason
              </button>
            </div>
          )}
        </article>
      ))}
      {adjustments.map((item) => (
        <article
          className="payable-adjustment"
          key={item.id}
          aria-label={`Carrier adjustment ${item.number} ${item.id}`}
        >
          <div className="charge-row">
            <strong>
              {label(item.kind)} · {item.number}
            </strong>
            <span className={`badge ${item.status}`}>{label(item.status)}</span>
          </div>
          <p>{item.description}</p>
          <p>
            Document {money(item.document_amount_cents)} · payable difference{" "}
            <strong>{money(item.amount_cents)}</strong>
            {item.kind === "invoice_revision"
              ? ` against recognized ${money(item.basis_total_cents)}`
              : ""}
          </p>
          <small>
            Issued {item.issued_on} · {item.evidence_reference} · recorded by{" "}
            {item.created_by}
          </small>
          {item.document_id && (
            <a
              className="text-link"
              href={identityHref(
                `/api/freight/documents/${encodeURIComponent(item.document_id)}/content`,
              )}
            >
              Download adjustment evidence
            </a>
          )}
          <details>
            <summary>Posting and correction history</summary>
            <p>
              Original approved invoice {invoice?.number} remains unchanged.
            </p>
            {item.corrects_id && <p>Corrects {item.corrects_id}</p>}
            {item.corrected_by_id && <p>Corrected by {item.corrected_by_id}</p>}
            {[item.decision, item.reversal_decision]
              .filter(Boolean)
              .map((proof, i) => (
                <div key={i}>
                  <p>{proof.note}</p>
                  <small>
                    {proof.counterparty_contact} ·{" "}
                    {proof.confirmation_reference} · {proof.recorded_by} ·{" "}
                    {new Date(proof.recorded_at).toLocaleString()}
                  </small>
                </div>
              ))}
            {item.reversal && (
              <p>
                Reversing journal {item.reversal.journal_id} ·{" "}
                {item.reversal.reversed_on}
              </p>
            )}
          </details>
          {canFinance && (
            <div className="charge-actions">
              {item.status === "draft" && (
                <>
                  <button
                    className="secondary"
                    disabled={
                      busy ||
                      !load.pod_ready ||
                      !!load.tender_control?.open_incidents
                    }
                    onClick={() => open("approve", item)}
                  >
                    Approve and post adjustment
                  </button>
                  <button
                    className="text-link"
                    disabled={busy}
                    onClick={() => open("reject", item)}
                  >
                    Reject adjustment draft
                  </button>
                </>
              )}
              {item.status === "posted" && (
                <button
                  className="text-link"
                  disabled={busy}
                  onClick={() => open("reverse", item)}
                >
                  Reverse adjustment posting
                </button>
              )}
              {["rejected", "reversed"].includes(item.status) &&
                !item.corrected_by_id && (
                  <button
                    className="secondary"
                    disabled={busy}
                    onClick={() => open("correct", item)}
                  >
                    Correct carrier adjustment
                  </button>
                )}
            </div>
          )}
        </article>
      ))}
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {form && (
        <form
          key={`${form.action}-${form.record?.id || "new"}`}
          className="charge-form"
          aria-label="Carrier payable form"
          onSubmit={submit}
        >
          <h4>
            {drafts
              ? "Record an actual carrier financial document"
              : form.action === "approve"
                ? "Verify the carrier agreement and post its difference"
                : form.action === "reverse"
                  ? "Reverse the posting with retained evidence"
                  : "Record finance classification"}
          </h4>
          {drafts ? (
            <>
              <div className="form-grid">
                <label className="field">
                  <span>Document treatment</span>
                  <select
                    name="kind"
                    aria-label="Carrier document treatment"
                    value={kind}
                    required
                    onChange={(e) => setKind(e.target.value)}
                  >
                    {negativeSource && (
                      <option value="" disabled>
                        Select verified credit treatment
                      </option>
                    )}
                    {!negativeSource && (
                      <>
                        <option value="supplemental_invoice">
                          Supplemental invoice · add this amount
                        </option>
                        <option value="invoice_revision">
                          Revised invoice · replace recognized total
                        </option>
                      </>
                    )}
                    {(!boundSource || negativeSource) && (
                      <option value="credit_memo">
                        Carrier credit memo · subtract this amount
                      </option>
                    )}
                  </select>
                </label>
                <label className="field">
                  <span>Original approved invoice</span>
                  <input
                    value={invoice?.number || ""}
                    readOnly
                    aria-label="Original approved carrier invoice"
                  />
                </label>
                <label className="field">
                  <span>Actual document number</span>
                  <input
                    name="number"
                    aria-label="Carrier adjustment document number"
                    defaultValue={form.record?.number || ""}
                    readOnly={!!source || form.action === "correct"}
                    required
                    maxLength={200}
                    autoFocus
                  />
                </label>
                <label className="field">
                  <span>
                    {kind === "invoice_revision"
                      ? `New invoice total (${currency})`
                      : `Document amount (${currency})`}
                  </span>
                  <input
                    name="amount"
                    aria-label={`Carrier document amount (${currency})`}
                    defaultValue={
                      source
                        ? moneyInput(Math.abs(source.amount_cents), currency)
                        : form.record
                          ? moneyInput(
                              form.record.document_amount_cents,
                              currency,
                            )
                          : ""
                    }
                    readOnly={!!boundSource}
                    inputMode="decimal"
                    required
                  />
                </label>
                <label className="field">
                  <span>Document issue date</span>
                  <input
                    name="issued_on"
                    aria-label="Carrier document issue date"
                    type="date"
                    defaultValue={
                      source?.event?.payload?.invoice_date ||
                      form.record?.issued_on ||
                      today()
                    }
                    required
                  />
                </label>
                <label className="field">
                  <span>External evidence / revision reference</span>
                  <input
                    name="evidence_reference"
                    aria-label="Carrier adjustment evidence reference"
                    defaultValue={
                      source?.event.event_id ||
                      form.record?.evidence_reference ||
                      ""
                    }
                    readOnly={form.action === "correct"}
                    maxLength={200}
                    required
                  />
                </label>
              </div>
              <label className="field">
                <span>Adjustment description</span>
                <input
                  name="description"
                  aria-label="Carrier adjustment description"
                  defaultValue={form.record?.description || ""}
                  maxLength={200}
                  required
                />
              </label>
              <label className="field">
                <span>Stored supporting file (optional)</span>
                <select
                  name="document_id"
                  aria-label="Carrier adjustment supporting document"
                  defaultValue={form.record?.document_id || ""}
                >
                  <option value="">
                    Evidence held externally under the reference
                  </option>
                  {workspace.documents
                    .filter(
                      (v) =>
                        v.load_id === load.id &&
                        v.reviews.at(-1)?.decision !== "rejected",
                    )
                    .map((doc) => (
                      <option key={doc.id} value={doc.id}>
                        {doc.filename}
                      </option>
                    ))}
                </select>
              </label>
              <p className="section-note">
                {kind === "invoice_revision"
                  ? `Only the difference from currently recognized ${money(total)} will post. If other revisions post first, reject and correct this draft.`
                  : "The document amount is a separate signed change. A draft does not post a liability or execute a payment."}
              </p>
            </>
          ) : (
            <>
              {!["dismiss", "apply-revision"].includes(form.action) && (
                <div className="form-grid">
                  <label className="field">
                    <span>Carrier contact</span>
                    <input
                      name="counterparty_contact"
                      aria-label="Payable counterparty contact"
                      required
                      maxLength={200}
                      autoFocus
                    />
                  </label>
                  <label className="field">
                    <span>Actual carrier confirmation reference</span>
                    <input
                      name="confirmation_reference"
                      aria-label="Payable confirmation reference"
                      required
                      maxLength={200}
                    />
                  </label>
                </div>
              )}
              {form.action === "reverse" && (
                <label className="field">
                  <span>Reversal date</span>
                  <input
                    name="reversed_on"
                    aria-label="Carrier adjustment reversal date"
                    type="date"
                    defaultValue={today()}
                    required
                  />
                </label>
              )}
              <label className="field">
                <span>Review / decision evidence</span>
                <textarea
                  name="note"
                  aria-label="Carrier payable decision note"
                  maxLength={200}
                  required
                  autoFocus={["dismiss", "apply-revision"].includes(
                    form.action,
                  )}
                />
              </label>
              <p className="section-note">
                {form.action === "apply-revision"
                  ? "Apply this exact canonical amount to the unapproved primary invoice. It is re-audited; no payable is posted."
                  : "Inspect the original document and verify the amount with the carrier. Posting/reversal retains the approved invoice, original journal, and all bank allocations."}
              </p>
            </>
          )}
          <div className="form-actions">
            <button
              type="button"
              className="secondary"
              disabled={busy}
              onClick={() => {
                setForm(null);
                setError("");
              }}
            >
              Close payable form
            </button>
            <button className="primary" disabled={busy}>
              {busy
                ? "Saving…"
                : drafts
                  ? "Save adjustment draft"
                  : "Record payable decision"}
            </button>
          </div>
        </form>
      )}
    </section>
  );
}
