import { useRef, useState } from "react";
import { Modal } from "./Modal";
const utc = (v) =>
  new Intl.DateTimeFormat("en-US", {
    timeZone: "UTC",
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(v)) + " UTC";
export function CarrierReleaseRequests({ requests, identity, onChanged }) {
  const [modal, setModal] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [message, setMessage] = useState("");
  const pending = useRef(null);
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const values = new FormData(event.currentTarget),
      p = modal.request;
    const body = {
      expected_load_version: p.load_version,
      document_sha256: p.document_sha256,
      decision: modal.decision,
      contact: values.get("contact"),
      reference: values.get("reference"),
      note: values.get("note"),
      ...(modal.decision === "release"
        ? { no_custody: values.get("no_custody") === "on" }
        : {}),
    };
    const path = `/api/freight/portal/releases/${p.id}/response`,
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
      const v = await r.json();
      if (!r.ok)
        throw new Error(v.error?.message || "Carrier release response failed");
      pending.current = null;
      setModal(null);
      setMessage(
        v.data.outcome === "awaiting_operations"
          ? "Your acknowledgement is retained. The assignment stays on hold for transportation team review."
          : modal.decision === "release"
            ? "Carrier release applied to this exact assignment."
            : "The existing assignment is retained.",
      );
      await onChanged();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  if (!requests.length) return null;
  return (
    <section aria-label="Carrier release requests">
      <h2>Carrier release requests</h2>
      {message && (
        <p className="partner-message" role="status">
          {message}
        </p>
      )}
      {requests
        .slice()
        .reverse()
        .map((p) => (
          <article
            key={p.id}
            className="partner-card"
            aria-label={`Carrier release request ${p.snapshot.load_reference} ${p.id}`}
          >
            <div className="partner-card-heading">
              <h3>{p.snapshot.load_reference}</h3>
              <span className={`badge ${p.status}`}>
                {p.status.replaceAll("_", " ")}
              </span>
            </div>
            <p>
              {p.snapshot.shipment_reference} · {p.snapshot.carrier_name}
            </p>
            <p>
              Pickup: {p.snapshot.origin}
              <br />
              Delivery: {p.snapshot.destination}
            </p>
            <p className="partner-terms">{p.snapshot.message}</p>
            <p>
              After release:{" "}
              {p.snapshot.disposition === "cancel"
                ? "Cancel this shipment"
                : "Return this shipment to planning"}
              .
            </p>
            <p>
              Release response and application deadline:{" "}
              {utc(p.snapshot.respond_by)}
            </p>
            {p.application_pending && (
              <p role="status">
                Your release acknowledgement is retained. Operations must
                resolve current holds before reassignment.
              </p>
            )}
            {p.response && (
              <details>
                <summary>Your release response</summary>
                <p>
                  {p.response.decision} · {p.response.reference} ·{" "}
                  {p.response.contact}
                </p>
                <p>{p.response.note}</p>
                <p>
                  Recorded {utc(p.response.recorded_at)} ·{" "}
                  {p.response.recorded_by}
                </p>
                {p.response.application && (
                  <p>
                    Applied {utc(p.response.application.applied_at)} ·{" "}
                    {p.response.application.mode === "operations"
                      ? "Operations review"
                      : "Native carrier response"}
                  </p>
                )}
              </details>
            )}
            <div className="partner-actions">
              <a
                className="secondary text-link"
                href={identity.href(
                  `/api/freight/portal/releases/${p.id}/statement`,
                )}
              >
                Download exact release request
              </a>
              {p.can_release && (
                <button
                  className="primary"
                  disabled={busy}
                  onClick={() => {
                    setError("");
                    setModal({ request: p, decision: "release" });
                  }}
                >
                  Confirm carrier release
                </button>
              )}
              {p.can_retain && (
                <button
                  className="secondary"
                  disabled={busy}
                  onClick={() => {
                    setError("");
                    setModal({ request: p, decision: "retain" });
                  }}
                >
                  Retain assignment
                </button>
              )}
            </div>
          </article>
        ))}
      {modal && (
        <Modal
          title={
            modal.decision === "release"
              ? "Confirm release of this assignment"
              : "Retain this carrier assignment"
          }
          subtitle={modal.request.snapshot.shipment_reference}
          close={() => !busy && setModal(null)}
        >
          <form onSubmit={submit}>
            <div className="form-body">
              {error && (
                <div className="form-error" role="alert">
                  {error}
                </div>
              )}
              <p>{modal.request.snapshot.message}</p>
              {modal.decision === "release" && (
                <p>
                  Release will{" "}
                  {modal.request.snapshot.disposition === "cancel"
                    ? "cancel the shipment"
                    : "return the shipment to planning"}
                  . Current movement, financial records or a live handoff can
                  hold application for operations review.
                </p>
              )}
              {modal.decision === "release" && (
                <label className="partner-ack">
                  <input name="no_custody" type="checkbox" required />I reviewed
                  this exact request. Our carrier has not picked up and does not
                  have custody of this shipment and agrees to release this
                  assignment before {utc(modal.request.snapshot.respond_by)}.
                </label>
              )}
              {modal.decision === "retain" && (
                <p>
                  Your carrier will remain assigned. This response does not
                  accept a tender or establish pickup. Report actual pickup
                  through shipment progress if freight has moved.
                </p>
              )}
              <label className="field">
                <span>Carrier contact</span>
                <input name="contact" required maxLength={200} />
              </label>
              <label className="field">
                <span>Carrier response reference</span>
                <input name="reference" required maxLength={200} />
              </label>
              <label className="field">
                <span>Carrier release findings</span>
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
                {busy ? "Recording…" : "Record carrier release response"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
