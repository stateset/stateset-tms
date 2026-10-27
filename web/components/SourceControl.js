import { useRef, useState } from "react";
import { AlertTriangle, RefreshCw, ShieldCheck } from "lucide-react";
import { useFreightIdentity } from "./FreightIdentity";
const when = (value) =>
  value ? new Date(value).toLocaleString() : "Not checked";
const needsConsent = (snapshot) =>
  !["pending", "processing", "shipped", "fulfilled", "delivered"].includes(
    String(snapshot?.status).trim().toLowerCase(),
  );

export function SourceControl({ load, workspace, onChanged }) {
  const { fetch } = useFreightIdentity();
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [form, setForm] = useState(null);
  const pending = useRef(null);
  if (!load.sync_order_id) return null;
  const control = load.source_control || {},
    check = control.check,
    reviews = (workspace.source_reviews || []).filter(
      (v) => v.load_id === load.id,
    ),
    review = reviews.find((v) => v.id === check?.active_review_id),
    writable = workspace.access.capabilities.includes("operations.write");
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
        },
        body: JSON.stringify(body),
      });
      const result = await response.json();
      if (!response.ok)
        throw new Error(result.error?.message || "Source action failed");
      pending.current = null;
      setForm(null);
      await onChanged();
    } catch (problem) {
      setError(problem.message);
      await onChanged();
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="source-control" aria-label="Commerce source control">
      <h3>
        <ShieldCheck size={17} /> Commerce order review
      </h3>
      <div
        className={
          control.held || control.unavailable ? "release-hold" : "active-tender"
        }
        role="status"
      >
        {control.held || control.unavailable ? (
          <AlertTriangle size={17} />
        ) : (
          <ShieldCheck size={17} />
        )}
        <div>
          <strong>
            {control.held
              ? "Source change requires review"
              : control.unavailable
                ? "Source verification needed"
                : "Source checked"}
          </strong>
          <p>
            Order {load.order_number || load.sync_order_id} · original status{" "}
            {load.source_order?.status || "unknown"} · latest status{" "}
            {check?.snapshot?.status || "unknown"}
          </p>
          <small>
            Checked {when(check?.checked_at)} · by {check?.checked_by || "—"}
          </small>
          {check?.last_error && <p role="alert">{check.last_error}</p>}
        </div>
      </div>
      <p className="section-note">
        New booking, tendering, and dispatch check the commerce order again.
        Carrier custody, physical updates, agreed prices, and financial history
        remain recorded.
      </p>
      {!control.writeback_safe && (
        <p className="source-warning">
          Tracking writeback is held for this source status. Physical freight
          updates remain available here.
        </p>
      )}
      <button
        className="secondary"
        disabled={!writable || busy}
        onClick={() => mutate(`loads/${load.id}/source/refresh`, {})}
      >
        <RefreshCw size={14} /> Refresh commerce order
      </button>
      {error && (
        <p role="alert" className="form-error">
          {error}
        </p>
      )}
      {review && (
        <article className="source-review">
          <h4>Review current source changes</h4>
          <p>
            Changed: {review.changed_fields.join(", ")} · observed{" "}
            {when(review.observed_at)} by {review.observed_by}
          </p>
          <div className="source-snapshots">
            <details>
              <summary>Previous authorized order</summary>
              <pre>{JSON.stringify(review.baseline, null, 2)}</pre>
            </details>
            <details>
              <summary>Latest canonical order</summary>
              <pre>{JSON.stringify(review.snapshot, null, 2)}</pre>
            </details>
          </div>
          <button
            className="secondary"
            disabled={!writable || busy || !!form}
            onClick={() => {
              setError("");
              setForm({
                id: review.id,
                expected_load_version: load.version,
                expected_version: review.version,
                expected_digest: review.digest,
                explicit: needsConsent(review.snapshot),
              });
            }}
          >
            Record customer source decision
          </button>
        </article>
      )}
      {form && (
        <form
          className="source-decision"
          aria-label="Customer source decision"
          onSubmit={(event) => {
            event.preventDefault();
            const values = Object.fromEntries(
              new FormData(event.currentTarget),
            );
            mutate(`source-reviews/${form.id}/resolve`, {
              expected_load_version: form.expected_load_version,
              expected_version: form.expected_version,
              expected_digest: form.expected_digest,
              ...values,
            });
          }}
        >
          <label className="field">
            <span>Source decision</span>
            <select name="decision" required defaultValue="">
              <option value="" disabled>
                Select the confirmed customer decision
              </option>
              {form.explicit ? (
                <option value="authorize_transport">
                  Customer explicitly authorizes transport despite source status
                </option>
              ) : (
                <option value="confirm_plan">
                  Customer confirms the current freight plan
                </option>
              )}
              <option
                value="cancel_transport"
                disabled={load.status !== "cancelled"}
              >
                Close review after safe shipment cancellation
              </option>
            </select>
          </label>
          <p className="section-note">
            Record actual customer confirmation of the current shipment plan. To
            stop transport, first use the shipment’s carrier release or unsent
            withdrawal process, then cancel it.
          </p>
          <label className="field">
            <span>Actual customer contact</span>
            <input name="contact" required maxLength={200} />
          </label>
          <label className="field">
            <span>Customer source confirmation reference</span>
            <input name="reference" required maxLength={200} />
          </label>
          <label className="field">
            <span>Source review findings</span>
            <textarea name="note" required maxLength={200} />
          </label>
          <div className="charge-actions">
            <button
              className="primary"
              disabled={!writable || busy}
              type="submit"
            >
              Save source decision
            </button>
            <button
              className="secondary"
              type="button"
              disabled={busy}
              onClick={() => setForm(null)}
            >
              Dismiss decision
            </button>
          </div>
        </form>
      )}
      {check && (
        <details className="source-original">
          <summary>Latest verified order evidence</summary>
          <pre>{JSON.stringify(check.snapshot, null, 2)}</pre>
        </details>
      )}
      <details className="source-original">
        <summary>Original imported order evidence</summary>
        <pre>{JSON.stringify(load.source_order, null, 2)}</pre>
      </details>
      {reviews.length > 0 && (
        <details className="source-original">
          <summary>Source decision history ({reviews.length})</summary>
          {reviews
            .slice()
            .reverse()
            .map((v) => (
              <article key={v.id} className="source-review">
                <strong>
                  {v.status} · {v.changed_fields.join(", ")}
                </strong>
                <small>{when(v.observed_at)}</small>
                {v.decision && (
                  <p>
                    {v.decision.quote_id &&
                      `Amendment ${workspace.quotes?.find((q) => q.id === v.decision.quote_id)?.reference || v.decision.quote_id} · `}
                    {v.decision.decision} · {v.decision.contact} ·{" "}
                    {v.decision.reference} · {v.decision.note} · recorded by{" "}
                    {v.decision.recorded_by} at {when(v.decision.recorded_at)}
                  </p>
                )}
              </article>
            ))}
        </details>
      )}
    </section>
  );
}
export function SourceQueue({ workspace, openLoad }) {
  const held = workspace.loads.filter(
    (v) =>
      v.source_control?.held ||
      (v.source_control?.unavailable &&
        !["cancelled", "delivered"].includes(v.status)),
  );
  if (!held.length) return null;
  return (
    <section className="panel" aria-label="Commerce source exceptions">
      <div className="panel-heading">
        <div>
          <h2>Commerce source exceptions</h2>
          <p>
            Review source changes before making new transportation commitments.
          </p>
        </div>
      </div>
      {held.map((v) => (
        <button
          key={v.id}
          className="charge-queue-item"
          onClick={() => openLoad(v.id)}
        >
          <span>
            <strong>{v.reference}</strong>
            <small>{v.order_number || v.sync_order_id}</small>
          </span>
          <span>
            {v.source_control.held ? "Review required" : "Verification needed"}
          </span>
        </button>
      ))}
    </section>
  );
}
