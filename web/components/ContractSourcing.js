import { useRef, useState } from "react";
import { Modal } from "./Modal";
import { useFreightIdentity } from "./FreightIdentity";
import { contractMoney } from "./CarrierContracts";
export function ContractSourcing({ quote, workspace, onChanged }) {
  const identity = useFreightIdentity(),
    pending = useRef(null);
  const [results, setResults] = useState(null),
    [modal, setModal] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const canWrite = workspace.access.capabilities.includes("operations.write");
  async function evaluate() {
    setBusy(true);
    setError("");
    try {
      const r = await identity.fetch(
          `/api/freight/quotes/${quote.id}/contract-rates`,
        ),
        v = await r.json();
      if (!r.ok)
        throw new Error(v.error?.message || "Could not evaluate agreements");
      setResults(v);
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  async function submit(e) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const v = Object.fromEntries(new FormData(e.currentTarget)),
        replacement = modal.context.kind === "replacement",
        path = `/api/freight/quotes/${modal.context.quote_id}/${replacement ? "contract-reprocure" : "contract-offers"}`;
      const body = {
        expected_version: modal.context.quote_version,
        agreement_id: modal.rate.id,
        expected_rate_version: modal.rate.version,
        revision_sha256: modal.rate.basis_sha256,
        valid_until: new Date(v.valid_until + "Z").toISOString(),
        contact: v.contact,
        reference: v.reference,
        note: v.note,
        ...(replacement
          ? {
              expected_load_version: modal.context.load_version,
              reason: v.reason,
            }
          : {}),
      };
      const fingerprint = JSON.stringify([path, body]);
      if (pending.current?.fingerprint !== fingerprint)
        pending.current = { fingerprint, key: crypto.randomUUID() };
      const r = await identity.fetch(path, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "Idempotency-Key": pending.current.key,
          },
          body: JSON.stringify(body),
        }),
        data = await r.json();
      if (!r.ok)
        throw new Error(data.error?.message || "Contract sourcing failed");
      pending.current = null;
      setModal(null);
      setResults(null);
      await onChanged();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="contract-sourcing" aria-label="Contract rate sourcing">
      <h3>Source an agreed carrier rate</h3>
      <p>
        Compare applicable agreements, inspect the original terms, then record
        fresh carrier capacity confirmation. Prices alone do not establish
        available capacity.
      </p>
      <button className="secondary" disabled={busy} onClick={evaluate}>
        {results ? "Refresh contract rates" : "Find contract rates"}
      </button>
      {!modal && error && (
        <div className="form-error" role="alert">
          {error}
        </div>
      )}
      {results && (
        <div className="contract-grid">
          {!results.rates.length && (
            <p>No lane agreements have been recorded.</p>
          )}
          {results.rates.map((rate) => (
            <article
              className="contract-card"
              key={rate.id}
              aria-label={`Contract rate ${rate.number}`}
            >
              <div className="charge-heading">
                <strong>
                  {rate.number} · Revision {rate.revision}
                </strong>
                <span
                  className={`badge ${rate.eligible ? "approved" : "exception"}`}
                >
                  {rate.eligible ? "applicable" : "not applicable"}
                </span>
              </div>
              <p>
                {rate.carrier_name} ·{" "}
                <strong>{contractMoney(rate.rate)}</strong> inclusive / FTL
              </p>
              <p>
                {rate.rate.origin.country} {rate.rate.origin.state}{" "}
                {rate.rate.origin.postal_code} → {rate.rate.destination.country}{" "}
                {rate.rate.destination.state}{" "}
                {rate.rate.destination.postal_code}
              </p>
              <p className="partner-terms">{rate.rate.terms}</p>
              {rate.reasons.length > 0 && (
                <ul>
                  {rate.reasons.map((v) => (
                    <li key={v}>{v}</li>
                  ))}
                </ul>
              )}
              <div className="charge-actions">
                <a
                  className="text-link"
                  href={identity.href(
                    `/api/freight/carrier-contracts/${rate.id}/statement?revision=${rate.revision}`,
                  )}
                >
                  Inspect exact agreement
                </a>
                <button
                  className="primary"
                  disabled={!canWrite || !rate.eligible || busy}
                  onClick={() => {
                    setError("");
                    setModal({ rate, context: results });
                  }}
                >
                  Confirm capacity at this rate
                </button>
              </div>
            </article>
          ))}
        </div>
      )}
      {modal && (
        <Modal
          title="Confirm carrier capacity at agreed rate"
          subtitle={`${modal.rate.number} · Revision ${modal.rate.revision}`}
          close={() => !busy && setModal(null)}
        >
          <form className="modal-form" onSubmit={submit}>
            {error && (
              <div className="form-error" role="alert">
                {error}
              </div>
            )}
            <p>
              {modal.rate.carrier_name} ·{" "}
              <strong>{contractMoney(modal.rate.rate)}</strong> inclusive, for
              this exact quote plan.{" "}
              {modal.context.kind === "replacement"
                ? "Replacement changes the safely unassigned carrier buy rate and retains the accepted customer sell agreement."
                : "This records a carrier offer; customer acceptance and booking remain separate."}
            </p>
            <p className="partner-terms">{modal.rate.rate.terms}</p>
            <label className="field">
              <span>Capacity valid until (UTC)</span>
              <input
                name="valid_until"
                type="datetime-local"
                required
                defaultValue={new Date(Date.now() + 86400000)
                  .toISOString()
                  .slice(0, 16)}
              />
            </label>
            <label className="field">
              <span>Capacity confirmation contact</span>
              <input name="contact" required maxLength={200} />
            </label>
            <label className="field">
              <span>Capacity confirmation reference</span>
              <input name="reference" required maxLength={200} />
            </label>
            <label className="field">
              <span>Capacity confirmation findings</span>
              <textarea name="note" required maxLength={200} rows={3} />
            </label>
            {modal.context.kind === "replacement" && (
              <label className="field">
                <span>Replacement procurement reason</span>
                <input name="reason" required maxLength={200} />
              </label>
            )}
            <label className="partner-ack">
              <input type="checkbox" required />I inspected this exact agreement
              and obtained current carrier capacity confirmation at this
              inclusive price.
            </label>
            <div className="form-footer">
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => setModal(null)}
              >
                Cancel
              </button>
              <button className="primary" disabled={busy}>
                {busy ? "Recording…" : "Record confirmed contract rate"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
