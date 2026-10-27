import { useRef, useState } from "react";
import { Modal } from "./Modal";
import { useFreightIdentity } from "./FreightIdentity";
export function ItineraryExecution({ load, workspace, offer, onChanged }) {
  const identity = useFreightIdentity(),
    pending = useRef(null),
    native = !!offer;
  const [form, setForm] = useState(null),
    [busy, setBusy] = useState(false),
    [confirmed, setConfirmed] = useState(false),
    [error, setError] = useState("");
  const route = native ? offer.reporting.itinerary : load.itinerary;
  if (!route?.requires_native_handoff) return null;
  const version = native ? offer.load_version : load.version,
    active = native ? offer.is_current : !!load.tender_id,
    status = native ? offer.status : load.status;
  const canWrite =
    native || workspace.access.capabilities.includes("operations.write");
  const extra = route.stops.filter(
    (s) => s.id !== "pickup" && s.id !== "delivery",
  );
  async function submit(e) {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    let saved = confirmed;
    try {
      if (!saved) {
        const v = Object.fromEntries(new FormData(e.currentTarget));
        const path = native
          ? `portal/offers/${form.offerId}/stop-services`
          : form.service
            ? `stop-services/${form.service.id}/correct`
            : `loads/${form.loadId}/stop-services`;
        const body = {
          expected_load_version: form.version,
          stop_id: form.stop.id,
          observed_at: new Date(v.observed_at + "Z").toISOString(),
          contact: v.contact,
          reference: v.reference,
          note: v.note,
        };
        const fingerprint = JSON.stringify([path, body]);
        if (pending.current?.fingerprint !== fingerprint)
          pending.current = { fingerprint, key: crypto.randomUUID() };
        const r = await identity.fetch(`/api/freight/${path}`, {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              "Idempotency-Key": pending.current.key,
            },
            body: JSON.stringify(body),
          }),
          data = await r.json();
        if (!r.ok)
          throw new Error(data.error?.message || "Stop confirmation failed");
        pending.current = null;
        saved = true;
        setConfirmed(true);
      }
      if ((await onChanged()) === false)
        throw new Error("Refresh the current shipment before continuing.");
      setForm(null);
      setConfirmed(false);
    } catch (e) {
      setError(saved ? `Stop confirmation saved. ${e.message}` : e.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section
      className="itinerary-execution"
      aria-label="Multi-stop service confirmations"
    >
      <h3>Full route service confirmations</h3>
      <p>
        Record actual service at each intermediate stop in order. Arrival alone
        does not confirm pickup or unloading. Final delivery and finance require
        every intermediate confirmation.
      </p>
      {!native && (
        <p className="section-note">
          Native handoff required: the current EDI 204 adapter cannot encode
          intermediate stops. Publish the full carrier confirmation and obtain
          actual acceptance. EDI retries cannot bypass this boundary.
        </p>
      )}
      <ol className="route-stop-list">
        {extra.map((s, i) => (
          <li key={s.id}>
            <div>
              <strong>
                Stop {s.sequence} · {s.kind} · {s.stop.facility}
              </strong>
              <p>
                {s.stop.city}, {s.stop.state} · {s.stop.date}
              </p>
              <span
                className={`badge ${s.completed ? "delivered" : "planned"}`}
              >
                {s.completed ? "service confirmed" : "confirmation missing"}
              </span>
              {s.service && (
                <p>
                  Actual service{" "}
                  {new Date(s.service.observed_at).toLocaleString()}
                </p>
              )}
              {!native && s.service && (
                <details>
                  <summary>Original stop evidence</summary>
                  <p>
                    {s.service.contact} · {s.service.reference} ·{" "}
                    {s.service.recorded_by}
                  </p>
                  <p>{s.service.note}</p>
                </details>
              )}
            </div>
            {active &&
              ["in_transit", "delivered"].includes(status) &&
              (!s.completed || !native) && (
                <button
                  className="secondary"
                  disabled={
                    !canWrite ||
                    busy ||
                    (!s.completed &&
                      extra.slice(0, i).some((v) => !v.completed))
                  }
                  onClick={() => {
                    setError("");
                    setConfirmed(false);
                    setForm({
                      stop: s,
                      service: s.completed ? s.service : null,
                      version,
                      loadId: load?.id,
                      offerId: offer?.id,
                    });
                  }}
                >
                  {s.completed
                    ? `Correct stop ${s.sequence} service`
                    : `Confirm stop ${s.sequence} service`}
                </button>
              )}
          </li>
        ))}
      </ol>
      {form && (
        <Modal
          title={
            form.service
              ? "Correct intermediate stop service"
              : "Confirm intermediate stop service"
          }
          subtitle={`Stop ${form.stop.sequence} · ${form.stop.stop.facility}`}
          close={() => !busy && setForm(null)}
        >
          <form className="modal-form" onSubmit={submit}>
            {error && (
              <div className="form-error" role="alert">
                {error}
              </div>
            )}
            {confirmed && (
              <p role="status">
                Stop confirmation saved.{" "}
                {busy
                  ? "Refreshing current shipment…"
                  : "Refresh the current shipment to continue."}
              </p>
            )}
            <p>
              Confirm actual {form.stop.kind} service at this exact stop.
              Original evidence remains after corrections. This does not approve
              a charge or payment.
            </p>
            <label className="field">
              <span>Actual service time (UTC)</span>
              <input
                name="observed_at"
                disabled={busy || confirmed}
                type="datetime-local"
                step="0.001"
                required
                defaultValue={(
                  form.service?.observed_at || new Date().toISOString()
                ).replace(/Z$/, "")}
              />
            </label>
            <label className="field">
              <span>Actual stop contact</span>
              <input
                name="contact"
                required
                maxLength={200}
                disabled={busy || confirmed}
              />
            </label>
            <label className="field">
              <span>Stop service reference</span>
              <input
                name="reference"
                required
                maxLength={200}
                disabled={busy || confirmed}
              />
            </label>
            <label className="field">
              <span>Stop service findings</span>
              <textarea
                name="note"
                required
                maxLength={200}
                rows={3}
                disabled={busy || confirmed}
              />
            </label>
            <label className="partner-ack">
              <input type="checkbox" required disabled={busy || confirmed} />I
              verified actual service at this exact route stop against the
              supporting evidence.
            </label>
            <div className="form-footer">
              <button
                className="secondary"
                type="button"
                disabled={busy}
                onClick={() => setForm(null)}
              >
                {confirmed ? "Close" : "Cancel"}
              </button>
              <button
                className="primary"
                disabled={busy || (!confirmed && !canWrite)}
              >
                {busy
                  ? confirmed
                    ? "Refreshing…"
                    : "Recording…"
                  : confirmed
                    ? "Refresh saved confirmation"
                    : "Save stop confirmation"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
