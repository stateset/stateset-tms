import { useRef, useState } from "react";
import { Modal } from "./Modal";
import { useFreightIdentity } from "./FreightIdentity";
const instant = (v) => new Date(v).toLocaleString();
export function MovementCorrections({ load, workspace, onChanged }) {
  const identity = useFreightIdentity(),
    pending = useRef(null);
  const [form, setForm] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const sources = load.movement_sources || [];
  if (!sources.length) return null;
  const canWrite = workspace.access.capabilities.includes("operations.write");
  async function submit(e) {
    e.preventDefault();
    setBusy(true);
    setError("");
    try {
      const v = Object.fromEntries(new FormData(e.currentTarget)),
        path = `carrier-submissions/${form.source.id}/time-corrections`,
        body = {
          expected_load_version: form.version,
          source_sha256: form.sourceHash,
          expected_previous_id: form.previousId,
          observed_at: new Date(v.observed_at + "Z").toISOString(),
          contact: v.contact,
          reference: v.reference,
          note: v.note,
        },
        fingerprint = JSON.stringify([path, body]);
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
        throw new Error(
          data.error?.message || "Movement time correction failed",
        );
      pending.current = null;
      setForm(null);
      await onChanged();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section
      className="movement-corrections"
      aria-label="Native movement time review"
    >
      <h3>Native movement time review</h3>
      <p className="section-note">
        Correct an actual carrier pickup or delivery time after reviewing the
        source and supporting evidence. Original reports remain. Shipment
        status, location, latest tracking time, rates, and payments stay
        unchanged.
      </p>
      <div className="movement-source-list">
        {sources.map((item) => {
          const source = item.source,
            last = item.corrections.at(-1);
          return (
            <article key={source.id} aria-label={`${source.kind} time source`}>
              <strong>
                {source.kind === "pickup"
                  ? "Carrier pickup"
                  : "Carrier delivery"}{" "}
                · {instant(item.effective_observed_at)}
              </strong>
              <p>
                Original actual time: {instant(source.observed_at)} ·{" "}
                {source.location}
              </p>
              <small>
                {source.reference} · {source.recorded_by}
              </small>
              {!item.evidence_valid && (
                <p className="form-error" role="alert">
                  Correction evidence cannot be verified. Reconcile the original
                  records before continuing.
                </p>
              )}
              {item.locked && (
                <p className="section-note">
                  Movement evidence is locked by financial records.
                </p>
              )}
              {!!item.corrections.length && (
                <details>
                  <summary>
                    Original correction history ({item.corrections.length})
                  </summary>
                  {item.corrections.map((c) => (
                    <div className="movement-correction-proof" key={c.id}>
                      <strong>{instant(c.observed_at)}</strong>
                      <p>
                        {c.contact} · {c.reference}
                      </p>
                      <p>{c.note}</p>
                      <small>
                        {c.recorded_by} · {instant(c.recorded_at)}
                      </small>
                    </div>
                  ))}
                </details>
              )}
              <button
                className="secondary"
                disabled={!canWrite || !item.can_correct || busy}
                onClick={() => {
                  setError("");
                  setForm({
                    source,
                    version: load.version,
                    sourceHash: item.source_sha256,
                    previousId: last?.id || null,
                    effective: item.effective_observed_at,
                  });
                }}
              >
                Correct {source.kind} time
              </button>
            </article>
          );
        })}
      </div>
      {form && (
        <Modal
          title={`Correct carrier ${form.source.kind} time`}
          subtitle={load.reference}
          close={() => !busy && setForm(null)}
        >
          <form className="modal-form" onSubmit={submit}>
            {error && (
              <div className="form-error" role="alert">
                {error}
              </div>
            )}
            <p>
              Original carrier report: {form.source.reference} ·{" "}
              {form.source.recorded_by}
            </p>
            <p>
              Original time: {instant(form.source.observed_at)}. This appends
              reviewed evidence and does not retract the physical movement.
            </p>
            <label className="field">
              <span>Corrected actual time (UTC)</span>
              <input
                name="observed_at"
                type="datetime-local"
                step="0.001"
                required
                defaultValue={form.effective.replace(/Z$/, "")}
              />
            </label>
            <label className="field">
              <span>Actual carrier contact</span>
              <input name="contact" required maxLength={200} />
            </label>
            <label className="field">
              <span>Movement correction reference</span>
              <input name="reference" required maxLength={200} />
            </label>
            <label className="field">
              <span>Movement correction findings</span>
              <textarea name="note" required maxLength={200} rows={3} />
            </label>
            <label className="partner-ack">
              <input type="checkbox" required />I reviewed this exact original
              carrier report and supporting evidence for the corrected actual
              time.
            </label>
            <div className="form-footer">
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => setForm(null)}
              >
                Cancel
              </button>
              <button className="primary" disabled={busy}>
                {busy ? "Recording…" : "Save movement correction"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
