import { useRef, useState } from "react";
import { Modal } from "./Modal";
import { currencyCode, formatMoney } from "../lib/money.mjs";
const utc = (value) =>
  new Intl.DateTimeFormat("en-US", {
    timeZone: "UTC",
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value)) + " UTC";
export function CustomerQuotes({ quotes, identity, onChanged }) {
  const [modal, setModal] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [message, setMessage] = useState("");
  const pending = useRef(null);
  async function refreshSource(q) {
    setBusy(true);
    setError("");
    setMessage("");
    try {
      const r = await identity.fetch(
        `/api/freight/portal/customer-quotes/${q.id}/source/refresh`,
        {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "Idempotency-Key": crypto.randomUUID(),
          },
          body: "{}",
        },
      );
      const v = await r.json();
      if (!r.ok)
        throw new Error(v.error?.message || "Source verification unavailable");
      await onChanged();
      setMessage(
        "Commerce order status verified. Review it before accepting this transportation plan.",
      );
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const fields = new FormData(event.currentTarget),
      q = modal.quote;
    const body = {
      expected_version: q.version,
      revision: q.snapshot.revision,
      document_sha256: q.document_sha256,
      decision: modal.decision,
      contact: fields.get("contact"),
      reference: fields.get("reference"),
      note: fields.get("note"),
    };
    if (modal.decision === "accept" && q.snapshot.amendment_of && q.source) {
      body.source_digest = q.source.digest;
      body.authorize_transport = fields.get("authorize_transport") === "on";
    }
    const path = `/api/freight/portal/customer-quotes/${q.id}/response`,
      fingerprint = JSON.stringify([path, body]);
    if (pending.current?.fingerprint !== fingerprint)
      pending.current = { fingerprint, key: crypto.randomUUID() };
    try {
      const r = await identity.fetch(path, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Idempotency-Key": pending.current.key,
        },
        body: JSON.stringify(body),
      });
      const result = await r.json();
      if (!r.ok)
        throw new Error(
          result.error?.message || "Customer decision could not be recorded",
        );
      pending.current = null;
      setModal(null);
      setMessage(
        modal.decision === "accept"
          ? "Acceptance recorded for this exact published revision. Your transportation team will book or activate it separately."
          : modal.decision === "withdraw"
            ? "Withdrawal recorded. Your original decision remains in history."
            : "Decline recorded for this published revision.",
      );
      await onChanged();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  function open(q, decision) {
    setError("");
    setMessage("");
    setModal({ quote: q, decision });
  }
  return (
    <section aria-label="Your customer quotes">
      <h2>Your quotes and amendments</h2>
      {error && !modal && (
        <div className="form-error" role="alert">
          {error}
        </div>
      )}
      {message && (
        <p className="partner-message" role="status">
          {message}
        </p>
      )}
      {!quotes.length && <p>No quotes have been published to your account.</p>}
      {quotes
        .slice()
        .reverse()
        .map((q) => (
          <article
            className="partner-card"
            key={q.id}
            aria-label={`Customer quote ${q.snapshot.reference} revision ${q.snapshot.revision}`}
          >
            <div className="partner-card-heading">
              <h3>
                {q.snapshot.reference} · revision {q.snapshot.revision}
              </h3>
              <span className={`badge ${q.status}`}>{q.status}</span>
            </div>
            <strong className="partner-price">
              {formatMoney(q.snapshot.amount_cents, q.snapshot.currency)}{" "}
              inclusive {currencyCode(q.snapshot.currency)}
            </strong>
            {q.snapshot.amendment_of && (
              <p>
                Amendment of {q.snapshot.amendment_of}. The existing agreement
                stays effective until your transportation team activates this
                amendment after carrier coordination.
              </p>
            )}
            {q.snapshot.shipment_reference && (
              <p>
                Shipment {q.snapshot.shipment_reference}
                {q.snapshot.order_number
                  ? ` · Order ${q.snapshot.order_number}`
                  : ""}
              </p>
            )}
            <div className="partner-stops">
              {[
                ["Pickup", q.snapshot.plan.origin],
                ["Delivery", q.snapshot.plan.destination],
              ].map(([title, s]) => (
                <section className="partner-stop" key={title}>
                  <h4>{title}</h4>
                  <strong>{s.facility}</strong>
                  <p>
                    {s.address_lines.join(", ")}
                    <br />
                    {s.city}, {s.state} {s.postal_code} {s.country}
                    <br />
                    {s.date}
                  </p>
                  {s.instructions && <p>{s.instructions}</p>}
                </section>
              ))}
            </div>
            <p>
              {q.snapshot.plan.equipment} · {q.snapshot.plan.commodity} ·{" "}
              {q.snapshot.plan.weight_lbs.toLocaleString()} lb
            </p>
            <p>
              Acceptance and booking deadline: {utc(q.snapshot.valid_until)}
            </p>
            <details>
              <summary>Published customer terms</summary>
              <p className="partner-terms">{q.snapshot.terms}</p>
            </details>
            {q.source && (
              <p>
                Current commerce order{" "}
                {q.source.order_number || q.source.order_id}:{" "}
                {q.source.status || "not verified"}
                {q.source.available
                  ? " · verified"
                  : " · verification unavailable or expired"}
                . Acceptance authorizes the published transportation plan; it
                does not change the commerce order.
                {q.source.checked_at && (
                  <> Verified at {utc(q.source.checked_at)}.</>
                )}
              </p>
            )}
            {q.is_current && q.status === "issued" && !q.can_accept && (
              <p>
                Acceptance is on hold. Refresh available source verification or
                ask your transportation team for current terms.
              </p>
            )}
            <div className="partner-actions">
              <a
                className="secondary text-link"
                href={identity.href(
                  `/api/freight/portal/customer-quotes/${q.id}/statement`,
                )}
              >
                Download published quote
              </a>
              {q.can_refresh_source && (
                <button
                  className="secondary"
                  disabled={busy}
                  onClick={() => refreshSource(q)}
                >
                  Verify commerce order
                </button>
              )}
              {q.can_accept && (
                <button
                  className="primary"
                  disabled={busy}
                  onClick={() => open(q, "accept")}
                >
                  Accept customer quote
                </button>
              )}
              {q.can_decline && (
                <button
                  className="secondary"
                  disabled={busy}
                  onClick={() => open(q, "decline")}
                >
                  Decline customer quote
                </button>
              )}
              {q.can_withdraw && (
                <button
                  className="secondary"
                  disabled={busy}
                  onClick={() => open(q, "withdraw")}
                >
                  Withdraw quote or acceptance
                </button>
              )}
            </div>
            {!!q.decisions.length && (
              <details>
                <summary>Your quote decisions ({q.decisions.length})</summary>
                {q.decisions.map((d, i) => (
                  <section key={i}>
                    <h4>
                      {d.decision} · {d.reference}
                    </h4>
                    <p>{d.note}</p>
                    {d.source && (
                      <p>
                        Source at acceptance: order{" "}
                        {d.source.order_number || d.source.order_id} ·{" "}
                        {d.source.status}
                        {d.source.checked_at
                          ? ` · verified ${utc(d.source.checked_at)}`
                          : ""}
                        .
                        {d.authorize_transport
                          ? " Customer explicitly authorized the amended transportation plan despite this order status."
                          : " Acceptance remains tied to this source verification."}
                      </p>
                    )}
                    <p>
                      {d.contact} · {d.recorded_by} ·{" "}
                      {new Date(d.recorded_at).toLocaleString()}
                    </p>
                  </section>
                ))}
              </details>
            )}
          </article>
        ))}
      {modal && (
        <Modal
          title={
            modal.decision === "accept"
              ? "Accept published customer quote"
              : modal.decision === "withdraw"
                ? "Withdraw quote or acceptance"
                : "Decline published customer quote"
          }
          subtitle={modal.quote.snapshot.reference}
          close={() => !busy && setModal(null)}
        >
          <form onSubmit={submit}>
            <div className="form-body">
              {error && (
                <div className="form-error" role="alert">
                  {error}
                </div>
              )}
              <p>
                {formatMoney(
                  modal.quote.snapshot.amount_cents,
                  modal.quote.snapshot.currency,
                )}{" "}
                inclusive {currencyCode(modal.quote.snapshot.currency)} ·
                revision {modal.quote.snapshot.revision}
              </p>
              {modal.decision === "accept" && (
                <>
                  <p>
                    Acceptance and booking must occur before{" "}
                    {new Date(
                      modal.quote.snapshot.valid_until,
                    ).toLocaleString()}
                    . Acceptance alone does not book freight, release a carrier,
                    or approve payment.
                  </p>
                  <label className="partner-ack">
                    <input type="checkbox" required />I reviewed the exact
                    published document, shipment plan, price, and terms and
                    accept this revision.
                  </label>
                  {modal.quote.source && (
                    <p>
                      Verified commerce order{" "}
                      {modal.quote.source.order_number ||
                        modal.quote.source.order_id}{" "}
                      · status {modal.quote.source.status}. A changed source
                      requires renewed verification.
                    </p>
                  )}
                  {modal.quote.source?.requires_authorization && (
                    <label className="partner-ack">
                      <input
                        name="authorize_transport"
                        type="checkbox"
                        required
                      />
                      I explicitly authorize this amended transportation plan
                      despite the commerce order's current status.
                    </label>
                  )}
                </>
              )}
              {modal.decision === "withdraw" && (
                <p>
                  Record your actual withdrawal of this unbooked quote or
                  acceptance. This retains earlier decisions and does not cancel
                  a booked shipment or release a carrier.
                </p>
              )}
              <label className="field">
                <span>Your customer contact</span>
                <input name="contact" required maxLength={200} />
              </label>
              <label className="field">
                <span>Your decision reference</span>
                <input name="reference" required maxLength={200} />
              </label>
              <label className="field">
                <span>Customer decision note</span>
                <textarea name="note" required maxLength={200} rows={3} />
              </label>
            </div>
            <div className="modal-footer">
              <button
                className="secondary"
                type="button"
                disabled={busy}
                onClick={() => setModal(null)}
              >
                Cancel
              </button>
              <button className="primary" disabled={busy}>
                {busy ? "Recording…" : "Record customer decision"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
