import { useRef, useState } from "react";
import { useFreightIdentity } from "./FreightIdentity";
import { formatMoney, moneyInput, parseMoney } from "../lib/money.mjs";

const today = () => new Date().toISOString().slice(0, 10);
function Field({ label, children, ...props }) {
  return (
    <label className="field">
      <span>{label}</span>
      {children || <input aria-label={label} required {...props} />}
    </label>
  );
}
export function CancellationFees({ load, workspace, onChanged }) {
  const { fetch, href } = useFreightIdentity();
  const money = (v) => formatMoney(v, load.currency);
  const [form, setForm] = useState(null),
    [side, setSide] = useState("sell"),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const pending = useRef(null);
  if (load.status !== "cancelled") return null;
  const fees = (workspace.cancellation_fees || []).filter(
    (v) => v.load_id === load.id,
  );
  const tenders = (workspace.tenders || []).filter(
    (v) =>
      v.load_id === load.id &&
      v.status === "released" &&
      (workspace.releases || []).some(
        (r) => r.tender_id === v.id && r.status === "confirmed",
      ),
  );
  const canPropose = workspace.access.capabilities.includes("charges.write"),
    canReview = workspace.access.capabilities.includes("finance.write");
  const held = !!load.tender_control?.open_incidents;
  const target = (fee) =>
    fee.side === "buy"
      ? workspace.settlements.find((v) => v.id === fee.target_id)
      : workspace.bills.find((v) => v.id === fee.target_id);
  const open = (kind, fee = null) => {
    setError("");
    setSide(fee?.side || (tenders.length ? "buy" : "sell"));
    setForm({
      kind,
      fee,
      loadVersion: load.version,
      targetVersion: fee ? target(fee)?.version : null,
    });
  };
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const v = Object.fromEntries(new FormData(event.currentTarget));
      let path,
        body = { expected_load_version: form.loadVersion };
      if (["propose", "correct"].includes(form.kind)) {
        path = `loads/${load.id}/cancellation-fees`;
        body = {
          ...body,
          side,
          amount_cents: parseMoney(v.amount, load.currency),
          description: v.description,
          evidence_reference:
            form.fee?.evidence_reference || v.evidence_reference,
          tender_id: side === "buy" ? form.fee?.tender_id || v.tender_id : null,
          document_id: v.document_id || null,
          corrects_id: form.fee?.id || null,
        };
      } else {
        body = {
          ...body,
          expected_version: form.fee.version,
          counterparty_contact: v.counterparty_contact,
          confirmation_reference: v.confirmation_reference,
          note: v.note,
        };
        if (form.kind === "reverse") {
          path = `cancellation-fees/${form.fee.id}/reverse`;
          body.expected_target_version = form.targetVersion;
          body.reversed_on = v.reversed_on;
        } else {
          path = `cancellation-fees/${form.fee.id}/review`;
          body.decision = form.kind;
          if (form.kind === "approve") {
            body.number = v.number;
            if (form.fee.side === "sell") body.due_date = v.due_date;
          }
        }
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
          result.error?.message || "Cancellation fee update failed",
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
  return (
    <section className="shipment-charges" aria-label="Cancellation fees">
      <div className="charge-heading">
        <h3>Cancellation fees</h3>
        {canPropose && (
          <button
            className="secondary"
            disabled={busy || held}
            onClick={() => open("propose")}
          >
            Propose cancellation fee
          </button>
        )}
      </div>
      <p className="section-note">
        This shipment remains cancelled. Record only an actual carrier or
        customer agreement. Original transport rates do not become payable, and
        no delivery or POD is created.
      </p>
      {held && (
        <p className="error" role="status">
          Resolve the carrier custody conflict before recording more fee
          decisions.
        </p>
      )}
      {!fees.length && (
        <p className="section-note">No cancellation fees recorded.</p>
      )}
      <div className="charge-list">
        {fees.map((fee) => {
          const record = target(fee);
          return (
            <article className="charge-card" key={fee.id}>
              <div className="charge-heading">
                <strong>
                  {fee.side === "buy" ? "Carrier payable" : "Customer invoice"}{" "}
                  · {money(fee.amount_cents)}
                </strong>
                <span className={`badge ${fee.status}`}>{fee.status}</span>
              </div>
              <p>{fee.description}</p>
              <p>
                {fee.party_snapshot.name} · Agreement {fee.evidence_reference}
              </p>
              {fee.number && (
                <p>
                  Invoice {fee.number} · Balance{" "}
                  {money(
                    fee.side === "buy"
                      ? record?.balance_cents || 0
                      : record?.totals.balance_cents || 0,
                  )}
                </p>
              )}
              {fee.decision && (
                <p className="section-note">
                  Reviewed by {fee.decision.recorded_by} with{" "}
                  {fee.decision.counterparty_contact} ·{" "}
                  {fee.decision.confirmation_reference}
                  <br />
                  {fee.decision.note}
                </p>
              )}
              {fee.reversal_confirmation && (
                <p className="section-note">
                  Reversed by {fee.reversal_confirmation.recorded_by} ·{" "}
                  {fee.reversal_confirmation.confirmation_reference}
                  <br />
                  {fee.reversal_confirmation.note}
                </p>
              )}
              {fee.document_id && (
                <a
                  className="text-link"
                  href={href(
                    `/api/freight/documents/${fee.document_id}/content`,
                  )}
                  download
                >
                  Download cancellation evidence
                </a>
              )}
              {fee.side === "sell" && fee.target_id && (
                <a
                  className="text-link"
                  href={href(`/api/freight/bills/${fee.target_id}/statement`)}
                  download
                >
                  Download cancellation invoice
                </a>
              )}
              {fee.corrects_id && <small>Corrects {fee.corrects_id}</small>}
              {fee.corrected_by_id && (
                <small>Corrected by {fee.corrected_by_id}</small>
              )}
              <div className="charge-actions">
                {canReview && fee.status === "proposed" && (
                  <>
                    <button
                      className="secondary"
                      disabled={busy || held}
                      onClick={() => open("approve", fee)}
                    >
                      Approve cancellation fee
                    </button>
                    <button
                      className="secondary"
                      disabled={busy || held}
                      onClick={() => open("reject", fee)}
                    >
                      Reject cancellation fee
                    </button>
                  </>
                )}
                {canReview && fee.status === "posted" && (
                  <button
                    className="secondary"
                    disabled={busy || held}
                    onClick={() => open("reverse", fee)}
                  >
                    Reverse cancellation fee
                  </button>
                )}
                {canPropose &&
                  ["rejected", "reversed"].includes(fee.status) &&
                  !fee.corrected_by_id && (
                    <button
                      className="secondary"
                      disabled={busy || held}
                      onClick={() => open("correct", fee)}
                    >
                      Correct cancellation fee
                    </button>
                  )}
              </div>
            </article>
          );
        })}
      </div>
      {form && (
        <form
          className="charge-form"
          onSubmit={submit}
          aria-label="Cancellation fee form"
        >
          <h4>
            {
              {
                propose: "Propose cancellation fee",
                correct: "Correct cancellation fee",
                approve: "Approve cancellation fee",
                reject: "Reject cancellation fee",
                reverse: "Reverse cancellation fee",
              }[form.kind]
            }
          </h4>
          {["propose", "correct"].includes(form.kind) ? (
            <>
              <Field label="Cancellation fee side">
                <select
                  aria-label="Cancellation fee side"
                  name="side"
                  value={side}
                  disabled={!!form.fee}
                  onChange={(e) => setSide(e.target.value)}
                >
                  <option value="sell">Customer charge</option>
                  {tenders.length > 0 && (
                    <option value="buy">Carrier cost</option>
                  )}
                </select>
              </Field>
              {side === "buy" && (
                <Field label="Released carrier attempt">
                  <select
                    aria-label="Released carrier attempt"
                    name="tender_id"
                    required
                    disabled={!!form.fee}
                    defaultValue={form.fee?.tender_id || tenders[0]?.id}
                  >
                    {tenders.map((t) => (
                      <option key={t.id} value={t.id}>
                        {t.carrier.name} · attempt {t.sequence} ·{" "}
                        {t.shipment_reference}
                      </option>
                    ))}
                  </select>
                </Field>
              )}
              <Field
                label={`Cancellation fee amount (${load.currency || "USD"})`}
                name="amount"
                inputMode="decimal"
                defaultValue={
                  form.fee
                    ? moneyInput(form.fee.amount_cents, form.fee.currency)
                    : ""
                }
              />
              <Field
                label="Cancellation fee description"
                name="description"
                maxLength={200}
                defaultValue={form.fee?.description || ""}
              />
              <Field
                label="Cancellation agreement reference"
                name="evidence_reference"
                disabled={!!form.fee}
                defaultValue={form.fee?.evidence_reference || ""}
              />
              <Field label="Cancellation evidence file">
                <select
                  aria-label="Cancellation evidence file"
                  name="document_id"
                  defaultValue={form.fee?.document_id || ""}
                >
                  <option value="">
                    Evidence held externally at the agreement reference
                  </option>
                  {(workspace.documents || [])
                    .filter(
                      (d) =>
                        d.load_id === load.id &&
                        d.reviews.at(-1)?.decision !== "rejected",
                    )
                    .map((d) => (
                      <option key={d.id} value={d.id}>
                        {d.filename}
                      </option>
                    ))}
                </select>
              </Field>
            </>
          ) : (
            <>
              <Field
                label="Cancellation counterparty contact"
                name="counterparty_contact"
                maxLength={200}
              />
              <Field
                label="Cancellation confirmation reference"
                name="confirmation_reference"
                maxLength={200}
              />
              <Field
                label="Cancellation decision note"
                name="note"
                maxLength={200}
              />
              {form.kind === "approve" && (
                <>
                  <Field
                    label={
                      form.fee.side === "buy"
                        ? "Actual carrier cancellation invoice number"
                        : "Customer cancellation invoice number"
                    }
                    name="number"
                    maxLength={60}
                  />
                  {form.fee.side === "sell" && (
                    <Field
                      label="Cancellation invoice due date"
                      name="due_date"
                      type="date"
                      min={today()}
                      defaultValue={today()}
                    />
                  )}
                </>
              )}
              {form.kind === "reverse" && (
                <>
                  <Field
                    label="Cancellation reversal date"
                    name="reversed_on"
                    type="date"
                    max={today()}
                    defaultValue={today()}
                  />
                  <p className="section-note">
                    This appends a reversal and retains the original invoice and
                    bank history. Reverse active customer credits first; genuine
                    receipts stay recorded and can become customer refund
                    balances. A carrier fee already paid leaves a credit balance
                    for an actual refund or agreed credit application.
                  </p>
                </>
              )}
            </>
          )}
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          <div className="charge-actions">
            <button type="submit" className="primary" disabled={busy}>
              {busy ? "Saving…" : "Save cancellation fee"}
            </button>
            <button
              type="button"
              className="secondary"
              disabled={busy}
              onClick={() => setForm(null)}
            >
              Close fee form
            </button>
          </div>
        </form>
      )}
      {fees.some((v) => v.posted_at) && (
        <p className="section-note">
          Reconcile cleared receipts, payouts, and customer/carrier refunds in
          Finance → Bank reconciliation. Recording a fee does not send an
          invoice or transfer money.
        </p>
      )}
    </section>
  );
}
