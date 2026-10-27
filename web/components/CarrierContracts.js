import { cloneElement, useRef, useState } from "react";
import { Modal } from "./Modal";
import { useFreightIdentity } from "./FreightIdentity";
import {
  currencyCode,
  formatMoney,
  moneyInput,
  parseMoney,
} from "../lib/money.mjs";
// Contract rates omit `currency` for USD; formatMoney applies that default.
export const contractMoney = (rate) =>
  formatMoney(rate.amount_cents, rate.currency);
const today = () => new Date().toISOString().slice(0, 10);
function Field({ label, children, ...props }) {
  return (
    <label className="field">
      <span>{label}</span>
      {children ? (
        cloneElement(children, { "aria-label": label })
      ) : (
        <input required aria-label={label} {...props} />
      )}
    </label>
  );
}
export function CarrierContracts({ workspace, onChanged }) {
  const identity = useFreightIdentity(),
    pending = useRef(null);
  const [modal, setModal] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const contracts = workspace.carrier_contracts || [],
    canWrite = workspace.access.capabilities.includes("operations.write"),
    r = modal?.contract?.revisions.at(-1),
    rate = r?.rate;
  const open = (kind, contract = null) => {
    setError("");
    setModal({ kind, contract });
  };
  async function submit(e) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const v = Object.fromEntries(new FormData(e.currentTarget));
      let path = "carrier-contracts",
        body;
      const proof = {
        contact: v.contact,
        reference: v.reference,
        note: v.note,
      };
      if (modal.kind === "withdraw") {
        path += `/${modal.contract.id}/withdraw`;
        body = { expected_version: modal.contract.version, ...proof };
      } else {
        const point = (name) => ({
          country: v[`${name}_country`],
          state: v[`${name}_state`],
          postal_code: v[`${name}_postal`],
        });
        body = {
          number: modal.contract?.number || v.number,
          carrier_id: modal.contract?.carrier_id || v.carrier_id,
          rate: {
            customer_id: v.customer_id || null,
            origin: point("origin"),
            destination: point("destination"),
            equipment: v.equipment,
            max_weight_lbs: Number(v.max_weight_lbs),
            max_miles: Number(v.max_miles),
            effective_from: v.effective_from,
            effective_through: v.effective_through,
            amount_cents: parseMoney(v.amount, currencyCode(v.currency)),
            currency: currencyCode(v.currency),
            terms: v.terms,
          },
          ...proof,
          reason: v.reason,
        };
        if (modal.kind === "revise") {
          path += `/${modal.contract.id}/revise`;
          body.expected_version = modal.contract.version;
        }
      }
      const fingerprint = JSON.stringify([path, body]);
      if (pending.current?.fingerprint !== fingerprint)
        pending.current = { fingerprint, key: crypto.randomUUID() };
      const response = await identity.fetch(`/api/freight/${path}`, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "Idempotency-Key": pending.current.key,
          },
          body: JSON.stringify(body),
        }),
        data = await response.json();
      if (!response.ok)
        throw new Error(data.error?.message || "Carrier agreement failed");
      pending.current = null;
      setModal(null);
      await onChanged();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section
      className="panel contract-workspace"
      aria-label="Carrier lane agreements"
    >
      <div className="panel-heading">
        <div>
          <h2>Carrier lane agreements</h2>
          <p>
            Retain reviewed inclusive rates. Confirm current capacity separately
            when sourcing a quote.
          </p>
        </div>
        <button
          className="secondary"
          disabled={!canWrite}
          onClick={() => open("create")}
        >
          Record carrier agreement
        </button>
      </div>
      {!contracts.length && (
        <p className="empty">No carrier lane agreements recorded.</p>
      )}
      <div className="contract-grid">
        {contracts.map((c) => {
          const r = c.revisions.at(-1);
          return (
            <article
              className="contract-card"
              key={c.id}
              aria-label={`Carrier agreement ${c.number}`}
            >
              <div className="charge-heading">
                <strong>{c.number}</strong>
                <span
                  className={`badge ${c.withdrawal ? "withdrawn" : "approved"}`}
                >
                  {c.withdrawal
                    ? "withdrawn"
                    : r.rate.effective_through < today()
                      ? "expired"
                      : "recorded"}
                </span>
              </div>
              <p>
                {r.carrier.name} · Revision {r.revision}
              </p>
              <h3>
                {contractMoney(r.rate)} <small>inclusive / FTL</small>
              </h3>
              <p>
                {r.rate.origin.country} {r.rate.origin.state}{" "}
                {r.rate.origin.postal_code} → {r.rate.destination.country}{" "}
                {r.rate.destination.state} {r.rate.destination.postal_code}
              </p>
              <p>
                {r.rate.equipment} · Up to{" "}
                {r.rate.max_weight_lbs.toLocaleString()} lbs /{" "}
                {r.rate.max_miles.toLocaleString()} mi
              </p>
              <p>
                {r.rate.effective_from} through {r.rate.effective_through} ·{" "}
                {r.customer?.name || "All customers"}
              </p>
              <details>
                <summary>Original terms and revision history</summary>
                {c.revisions
                  .slice()
                  .reverse()
                  .map((r) => (
                    <section key={r.revision}>
                      <p>
                        Revision {r.revision} · {contractMoney(r.rate)} ·{" "}
                        {r.evidence.contact} · {r.evidence.reference}
                      </p>
                      <p className="partner-terms">{r.rate.terms}</p>
                      <p>
                        {r.evidence.note} · {r.evidence.recorded_by}
                      </p>
                      <a
                        className="text-link"
                        href={identity.href(
                          `/api/freight/carrier-contracts/${c.id}/statement?revision=${r.revision}`,
                        )}
                      >
                        Download agreement revision {r.revision}
                      </a>
                    </section>
                  ))}
                {c.withdrawal && (
                  <p>
                    Withdrawn: {c.withdrawal.reference} · {c.withdrawal.note} ·{" "}
                    {c.withdrawal.recorded_by}
                  </p>
                )}
              </details>
              {!c.withdrawal && (
                <div className="charge-actions">
                  <button
                    className="secondary"
                    disabled={!canWrite}
                    onClick={() => open("revise", c)}
                  >
                    Revise carrier agreement
                  </button>
                  <button
                    className="text-link"
                    disabled={!canWrite}
                    onClick={() => open("withdraw", c)}
                  >
                    Withdraw for new sourcing
                  </button>
                </div>
              )}
            </article>
          );
        })}
      </div>
      {modal && (
        <Modal
          wide
          title={
            modal.kind === "withdraw"
              ? "Withdraw carrier agreement"
              : modal.kind === "revise"
                ? "Revise carrier agreement"
                : "Record carrier agreement"
          }
          subtitle={
            modal.contract?.number ||
            "Staff-attested external agreement · inclusive full truckload"
          }
          close={() => !busy && setModal(null)}
        >
          <form className="modal-form" onSubmit={submit}>
            {error && (
              <div className="form-error" role="alert">
                {error}
              </div>
            )}
            {modal.kind === "withdraw" ? (
              <p>
                Stop new sourcing from this agreement. Previously confirmed
                offers, bookings, tenders and invoices keep their original terms
                and evidence.
              </p>
            ) : (
              <>
                <p>
                  Review the original carrier agreement. This retains external
                  terms and the named operator's attestation; it does not
                  reserve capacity or send a tender. Exact directed postal codes
                  apply; ZIP5 and ZIP9 are separate lanes.
                </p>
                <div className="form-grid">
                  <Field
                    label="Agreement number"
                    name="number"
                    maxLength={200}
                    defaultValue={modal.contract?.number}
                    disabled={!!modal.contract}
                  />
                  <Field label="Agreement carrier">
                    <select
                      name="carrier_id"
                      required
                      defaultValue={modal.contract?.carrier_id || ""}
                      disabled={!!modal.contract}
                    >
                      <option value="">Select carrier</option>
                      {workspace.carriers.map((c) => (
                        <option key={c.id} value={c.id}>
                          {c.name} · {c.scac}
                        </option>
                      ))}
                    </select>
                  </Field>
                </div>
                <Field label="Customer scope">
                  <select
                    name="customer_id"
                    defaultValue={rate?.customer_id || ""}
                  >
                    <option value="">All customers</option>
                    {workspace.customers.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.name}
                      </option>
                    ))}
                  </select>
                </Field>
                {["origin", "destination"].map((name) => (
                  <fieldset className="contract-lane" key={name}>
                    <legend>
                      {name === "origin" ? "Origin" : "Destination"} postal lane
                    </legend>
                    <div className="form-grid three">
                      <Field
                        label={`${name === "origin" ? "Origin" : "Destination"} country`}
                      >
                        <select
                          name={`${name}_country`}
                          defaultValue={rate?.[name].country || "US"}
                        >
                          <option value="US">United States</option>
                          <option value="CA">Canada</option>
                        </select>
                      </Field>
                      <Field
                        label={`${name === "origin" ? "Origin" : "Destination"} state/province`}
                        name={`${name}_state`}
                        maxLength={2}
                        defaultValue={rate?.[name].state}
                      />
                      <Field
                        label={`${name === "origin" ? "Origin" : "Destination"} postal code`}
                        name={`${name}_postal`}
                        maxLength={12}
                        defaultValue={rate?.[name].postal_code}
                      />
                    </div>
                  </fieldset>
                ))}
                <div className="form-grid three">
                  <Field label="Agreement equipment">
                    <select
                      name="equipment"
                      defaultValue={rate?.equipment || "Dry van"}
                    >
                      {["Dry van", "Reefer", "Flatbed"].map((v) => (
                        <option key={v}>{v}</option>
                      ))}
                    </select>
                  </Field>
                  <Field
                    label="Maximum weight (lbs)"
                    name="max_weight_lbs"
                    type="number"
                    min={1}
                    max={80000}
                    defaultValue={rate?.max_weight_lbs || 45000}
                  />
                  <Field
                    label="Maximum distance (miles)"
                    name="max_miles"
                    type="number"
                    min={1}
                    max={10000}
                    defaultValue={rate?.max_miles || 1000}
                  />
                </div>
                <div className="form-grid three">
                  <Field
                    label="Effective from"
                    name="effective_from"
                    type="date"
                    defaultValue={rate?.effective_from || today()}
                  />
                  <Field
                    label="Effective through"
                    name="effective_through"
                    type="date"
                    defaultValue={rate?.effective_through}
                  />
                  <Field
                    label="Rate currency"
                    name="currency"
                    maxLength={3}
                    pattern="[A-Za-z]{3}"
                    defaultValue={currencyCode(rate?.currency)}
                  />
                  <Field
                    label="Inclusive rate"
                    name="amount"
                    inputMode="decimal"
                    defaultValue={
                      rate ? moneyInput(rate.amount_cents, rate.currency) : ""
                    }
                  />
                </div>
                <Field label="Original inclusive rate terms">
                  <textarea
                    name="terms"
                    required
                    maxLength={2000}
                    rows={3}
                    defaultValue={rate?.terms}
                  />
                </Field>
                <Field
                  label="Agreement revision reason"
                  name="reason"
                  maxLength={200}
                />
              </>
            )}
            <Field
              label="Actual carrier agreement contact"
              name="contact"
              maxLength={200}
            />
            <Field
              label="Original carrier agreement reference"
              name="reference"
              maxLength={200}
            />
            <Field label="Private agreement findings">
              <textarea name="note" required maxLength={200} rows={3} />
            </Field>
            <label className="partner-ack">
              <input type="checkbox" required />I reviewed the original carrier
              evidence, exact scope and inclusive terms for this decision.
            </label>
            <div className="form-footer">
              <button
                className="secondary"
                type="button"
                disabled={busy}
                onClick={() => setModal(null)}
              >
                Cancel
              </button>
              <button className="primary" disabled={busy}>
                {busy
                  ? "Recording…"
                  : modal.kind === "withdraw"
                    ? "Withdraw agreement"
                    : "Save carrier agreement"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
