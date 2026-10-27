import { useFreightIdentity } from "./FreightIdentity";
import { useRef, useState } from "react";
import { ClipboardCheck, Plus } from "lucide-react";
import {
  amountStep,
  formatMoney,
  moneyInput,
  parseMoney,
} from "../lib/money.mjs";

const title = (value) =>
  value.replaceAll("_", " ").replace(/\b\w/g, (v) => v.toUpperCase());
export function ShipmentCharges({ load, workspace, onChanged }) {
  const { fetch, href: identityHref } = useFreightIdentity();
  const cash = (value) => formatMoney(value, load.currency);
  const [form, setForm] = useState(null),
    [side, setSide] = useState("buy"),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const pending = useRef(null),
    charges = (workspace.charges || []).filter((v) => v.load_id === load.id),
    commercial = load.commercial || {};
  const canPropose = workspace.access.capabilities.includes("charges.write"),
    canReview = workspace.access.capabilities.includes("finance.write");
  const eligible = [
    "accepted",
    "dispatched",
    "in_transit",
    "delivered",
  ].includes(load.status);
  function open(kind, charge = null) {
    setError("");
    const initialSide =
      charge?.side || (commercial.buy_locked ? "sell" : "buy");
    setSide(initialSide);
    setForm({ kind, charge, loadVersion: load.version, side: initialSide });
  }
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const values = Object.fromEntries(new FormData(event.currentTarget));
      let path,
        body = { ...values, expected_load_version: form.loadVersion };
      if (["propose", "correct"].includes(form.kind)) {
        path = `loads/${load.id}/charges`;
        body.amount_cents = parseMoney(values.amount, load.currency);
        delete body.amount;
        body.document_id = values.document_id || null;
        body.corrects_id = form.kind === "correct" ? form.charge.id : null;
        if (form.kind === "correct") {
          body.side = form.charge.side;
          body.evidence_reference = form.charge.evidence_reference;
        }
      } else {
        body.expected_version = form.charge.version;
        if (form.kind === "void") path = `charges/${form.charge.id}/void`;
        else {
          path = `charges/${form.charge.id}/review`;
          body.decision = form.kind === "approve" ? "approved" : "rejected";
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
        throw new Error(result.error?.message || "Charge update failed");
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
    <section
      className="shipment-charges"
      aria-label="Additional freight charges"
    >
      <div className="charge-heading">
        <h3>
          <ClipboardCheck size={17} /> Additional freight charges
        </h3>
        {canPropose &&
          eligible &&
          !(commercial.buy_locked && commercial.sell_locked) && (
            <button
              className="secondary"
              disabled={busy}
              onClick={() => open("propose")}
            >
              <Plus size={14} /> Propose charge
            </button>
          )}
      </div>
      <p className="section-note">
        Keep the original freight rate. Add only documented amounts agreed with
        the carrier or customer; finance reviews each side separately. Recorded
        dwell alone creates no charge.
      </p>
      <div className="commercial-totals">
        {["buy", "sell"].map((key) => {
          const basis = commercial[key],
            base = key === "buy" ? load.buy_cents : load.sell_cents;
          return (
            <div key={key}>
              <span>
                {key === "buy"
                  ? "Original carrier agreement"
                  : "Original customer agreement"}
              </span>
              <strong>{cash(basis?.total_amount_cents ?? base)}</strong>
              <small>
                Base {cash(base)} + approved{" "}
                {cash((basis?.total_amount_cents ?? base) - base)}
              </small>
              <small>
                {commercial[`${key}_locked`]
                  ? "Posted billing · agreement locked"
                  : `${commercial[`pending_${key}`] || 0} proposals awaiting review`}
              </small>
            </div>
          );
        })}
        <div>
          <span>Current gross margin</span>
          <strong
            className={
              (commercial.margin_cents ?? load.sell_cents - load.buy_cents) < 0
                ? "red"
                : "green"
            }
          >
            {cash(commercial.margin_cents ?? load.sell_cents - load.buy_cents)}
          </strong>
          <small>Proposals excluded until approved</small>
          {!!commercial.customer_supplement_cents && (
            <small>
              Issued customer supplements:{" "}
              {cash(commercial.customer_supplement_cents)}
            </small>
          )}
          {!!commercial.customer_credit_cents && (
            <small>
              Customer credits: −{cash(commercial.customer_credit_cents)}
            </small>
          )}
        </div>
      </div>
      {charges.length === 0 && (
        <p className="charge-empty">No additional charges recorded.</p>
      )}
      <div className="charge-list">
        {[...charges].reverse().map((charge) => (
          <article
            key={charge.id}
            aria-label={`Charge ${charge.evidence_reference} ${charge.side}`}
          >
            <div className="charge-row">
              <div>
                <strong>
                  {title(charge.kind)} · {cash(charge.amount_cents)}
                </strong>
                <p>
                  {charge.side === "buy" ? "Carrier cost" : "Customer charge"} ·{" "}
                  {charge.party_name}
                </p>
              </div>
              <span className={`badge ${charge.status}`}>
                {title(charge.status)}
              </span>
            </div>
            <p>{charge.description}</p>
            <small>
              Evidence: {charge.evidence_reference} · proposed by{" "}
              {charge.created_by}
            </small>
            {charge.document_id && (
              <a
                href={identityHref(
                  `/api/freight/documents/${encodeURIComponent(charge.document_id)}/content`,
                )}
                className="text-link"
              >
                Download supporting document
              </a>
            )}
            <details>
              <summary>Decision and correction history</summary>
              <p>
                Original proposal:{" "}
                {new Date(charge.created_at).toLocaleString()} ·{" "}
                {formatMoney(charge.amount_cents, charge.currency)}
              </p>
              {charge.document_sha256 && (
                <small>
                  Original document SHA-256: {charge.document_sha256}
                </small>
              )}
              {charge.tender_id && (
                <small>Carrier attempt: {charge.tender_id}</small>
              )}
              {charge.corrects_id && (
                <small>Corrects {charge.corrects_id}</small>
              )}
              {charge.corrected_by_id && (
                <small>Corrected by {charge.corrected_by_id}</small>
              )}
              {charge.decisions.map((decision, i) => (
                <div className="charge-decision" key={i}>
                  <strong>
                    {title(decision.decision)} · {decision.recorded_by}
                  </strong>
                  <p>{decision.note}</p>
                  {decision.counterparty_contact && (
                    <p>
                      Counterparty contact: {decision.counterparty_contact} ·{" "}
                      {decision.confirmation_reference}
                    </p>
                  )}
                  <small>
                    {new Date(decision.recorded_at).toLocaleString()}
                  </small>
                </div>
              ))}
            </details>
            <div className="charge-actions">
              {canReview && charge.status === "proposed" && (
                <>
                  <button
                    className="secondary"
                    disabled={
                      busy ||
                      commercial[`${charge.side}_locked`] ||
                      load.tender_control?.release_pending ||
                      !!load.tender_control?.open_incidents ||
                      (charge.side === "buy" &&
                        charge.tender_id !== load.tender_id)
                    }
                    onClick={() => open("approve", charge)}
                  >
                    Approve {charge.side === "buy" ? "carrier" : "customer"}{" "}
                    charge
                  </button>
                  <button
                    className="text-link"
                    disabled={busy}
                    onClick={() => open("reject", charge)}
                  >
                    Reject charge
                  </button>
                </>
              )}
              {canReview &&
                charge.status === "approved" &&
                !commercial[`${charge.side}_locked`] && (
                  <button
                    className="text-link"
                    disabled={busy}
                    onClick={() => open("void", charge)}
                  >
                    Void charge approval
                  </button>
                )}
              {canPropose &&
                ["rejected", "voided"].includes(charge.status) &&
                !charge.corrected_by_id &&
                !commercial[`${charge.side}_locked`] &&
                (charge.side === "sell" ||
                  charge.tender_id === load.tender_id) &&
                eligible && (
                  <button
                    className="secondary"
                    disabled={busy}
                    onClick={() => open("correct", charge)}
                  >
                    Correct charge proposal
                  </button>
                )}
            </div>
          </article>
        ))}
      </div>
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {form && (
        <form
          key={`${form.kind}-${form.charge?.id || "new"}`}
          className="charge-form"
          aria-label="Freight charge form"
          onSubmit={submit}
        >
          <h4>
            {
              {
                propose: "Propose a documented additional charge",
                correct: "Correct the prior charge proposal",
                approve: "Record counterparty agreement and approve",
                reject: "Reject the charge proposal",
                void: "Void the approval with supporting evidence",
              }[form.kind]
            }
          </h4>
          {["propose", "correct"].includes(form.kind) ? (
            <>
              <div className="form-grid">
                <label className="field">
                  <span>Agreement side</span>
                  <select
                    aria-label="Charge agreement side"
                    name="side"
                    value={side}
                    onChange={(event) => setSide(event.target.value)}
                    disabled={form.kind === "correct"}
                  >
                    <option value="buy" disabled={commercial.buy_locked}>
                      Carrier buy cost
                    </option>
                    <option value="sell" disabled={commercial.sell_locked}>
                      Customer sell charge
                    </option>
                  </select>
                </label>
                <label className="field">
                  <span>Charge kind</span>
                  <select
                    aria-label="Charge kind"
                    name="kind"
                    defaultValue={form.charge?.kind || "detention"}
                  >
                    {[
                      "detention",
                      "lumper",
                      "fuel",
                      "layover",
                      "extra_stop",
                      "other",
                    ].map((kind) => (
                      <option key={kind} value={kind}>
                        {title(kind)}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="field">
                  <span>Amount ({load.currency || "USD"})</span>
                  <input
                    name="amount"
                    aria-label={`Charge amount (${load.currency || "USD"})`}
                    inputMode="decimal"
                    defaultValue={
                      form.charge
                        ? moneyInput(
                            form.charge.amount_cents,
                            form.charge.currency,
                          )
                        : ""
                    }
                    required
                    autoFocus
                  />
                </label>
                <label className="field">
                  <span>Evidence reference (unique for this side)</span>
                  <input
                    name="evidence_reference"
                    aria-label="Charge evidence reference"
                    defaultValue={form.charge?.evidence_reference || ""}
                    readOnly={form.kind === "correct"}
                    maxLength={200}
                    required
                  />
                </label>
              </div>
              <label className="field">
                <span>Agreed service description</span>
                <input
                  name="description"
                  aria-label="Charge service description"
                  defaultValue={form.charge?.description || ""}
                  maxLength={200}
                  required
                />
              </label>
              <label className="field">
                <span>Supporting document (optional)</span>
                <select
                  name="document_id"
                  aria-label="Charge supporting document"
                  defaultValue={form.charge?.document_id || ""}
                >
                  <option value="">
                    Evidence is held externally under the reference above
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
                {side === "sell"
                  ? "This description appears on customer billing if approved. Keep internal notes and carrier buy costs out of it."
                  : "This proposal belongs to the current carrier attempt. It is excluded from the payable agreement until finance approves."}
              </p>
            </>
          ) : (
            <>
              <p>
                {form.charge.side === "buy"
                  ? "Carrier cost"
                  : "Customer charge"}
                : {cash(form.charge.amount_cents)} · {form.charge.party_name} ·{" "}
                {form.charge.description}
              </p>
              {form.kind !== "reject" && (
                <div className="form-grid">
                  <label className="field">
                    <span>Counterparty contact</span>
                    <input
                      name="counterparty_contact"
                      aria-label="Charge counterparty contact"
                      required
                      maxLength={200}
                      autoFocus
                    />
                  </label>
                  <label className="field">
                    <span>External agreement reference</span>
                    <input
                      name="confirmation_reference"
                      aria-label="Charge agreement confirmation"
                      required
                      maxLength={200}
                    />
                  </label>
                </div>
              )}
              <label className="field">
                <span>
                  {form.kind === "approve"
                    ? "Finance review and agreement evidence"
                    : "Decision reason and evidence"}
                </span>
                <textarea
                  name="note"
                  aria-label="Charge decision note"
                  required
                  maxLength={200}
                  autoFocus={form.kind === "reject"}
                />
              </label>
              <p className="section-note">
                {form.kind === "approve"
                  ? "Inspect the original evidence and verify this amount with the named counterparty. Approval changes the agreement basis; invoice posting still requires reviewed delivery evidence."
                  : "The original proposal and every prior decision remain. Posted financial documents keep their frozen basis."}
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
              Close charge form
            </button>
            <button className="primary" disabled={busy}>
              {busy ? "Saving…" : "Save charge decision"}
            </button>
          </div>
        </form>
      )}
    </section>
  );
}
