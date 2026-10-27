import { useRef, useState } from "react";
import { FileText, Download } from "lucide-react";
import { Modal } from "./Modal";
import { useFreightIdentity } from "./FreightIdentity";
import { formatMoney } from "../lib/money.mjs";
function Field({ title, children, ...props }) {
  return (
    <label className="field">
      <span>{title}</span>
      {children || <input required {...props} />}
    </label>
  );
}
export function RateConfirmations({ load, workspace, onChanged }) {
  const { fetch, href } = useFreightIdentity();
  const [modal, setModal] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const pending = useRef(null);
  const docs = (workspace.rate_confirmations || [])
    .filter((v) => v.load_id === load.id)
    .slice()
    .reverse();
  const latest = docs.find((v) => v.tender_id === load.tender_id);
  const canOperate = workspace.access.capabilities.includes("operations.write");
  const prePickup =
    !!load.carrier_id &&
    ["tendered", "accepted", "dispatched"].includes(load.status);
  const canIssue =
    prePickup &&
    !load.tender_control?.release_pending &&
    !load.tender_control?.open_incidents;
  const open = (value) => {
    setError("");
    setModal({ ...value, loadVersion: load.version });
  };
  async function submit(event) {
    event.preventDefault();
    setError("");
    setBusy(true);
    const values = Object.fromEntries(new FormData(event.currentTarget));
    let path,
      body = { expected_load_version: modal.loadVersion };
    if (modal.kind === "issue") {
      path = `loads/${load.id}/rate-confirmations`;
      body = {
        ...body,
        expected_previous_id: modal.doc?.id || null,
        terms: values.terms,
        reason: values.note,
      };
    } else if (modal.kind === "publish") {
      path = `loads/${load.id}/portal-offers`;
      body = {
        ...body,
        rate_confirmation_id: modal.doc.id,
        expected_previous_id:
          (workspace.portal_offers || [])
            .filter((v) => v.tender_id === modal.doc.tender_id)
            .at(-1)?.id || null,
      };
    } else {
      body.expected_version = modal.doc.version;
      path = `rate-confirmations/${modal.doc.id}/handoffs`;
      if (modal.kind === "handoff")
        body = {
          ...body,
          sent_at: `${values.sent_at}Z`,
          channel: values.channel,
          carrier_contact: values.carrier_contact,
          reference: values.reference,
          note: values.note,
          document_id: values.document_id || null,
        };
      else {
        body.handoff_id = modal.handoff.id;
        body.note = values.note;
        path += modal.kind === "resolve" ? "/resolve" : "/retract";
        if (modal.kind === "resolve")
          body = {
            ...body,
            carrier_contact: values.carrier_contact,
            confirmation_reference: values.reference,
          };
      }
    }
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
        throw new Error(
          result.error?.message || "Rate confirmation action failed",
        );
      pending.current = null;
      await onChanged();
      setModal(null);
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section
      className="rate-confirmations"
      aria-label="Carrier rate confirmations"
    >
      <div className="rate-confirmation-heading">
        <h3>
          <FileText size={17} /> Carrier rate confirmations
        </h3>
        {prePickup && (
          <button
            className="secondary"
            disabled={!canOperate || busy || !canIssue}
            onClick={() => open({ kind: "issue", doc: latest })}
          >
            {latest
              ? "Issue revised confirmation"
              : "Issue carrier confirmation"}
          </button>
        )}
      </div>
      <p className="rate-confirmation-help">
        Freeze the actual carrier rate and stop instructions for this tender.
        Download for your operating channel; record its external handoff
        separately. Issuance does not establish carrier acceptance.
      </p>
      {!docs.length && (
        <p className="rate-confirmation-help">
          No carrier rate confirmation issued.
        </p>
      )}
      {docs.map((doc) => (
        <article
          className="rate-confirmation-card"
          key={doc.id}
          aria-label={`Carrier confirmation revision ${doc.revision} ${doc.snapshot.shipment_reference}`}
        >
          <div className="rate-confirmation-heading">
            <strong>
              {doc.snapshot.carrier.name} · revision {doc.revision}
            </strong>
            <span className={`badge ${doc.current ? "accepted" : "cancelled"}`}>
              {doc.current ? "Current tender" : "Historical"}
            </span>
          </div>
          <p>
            {doc.snapshot.shipment_reference} ·{" "}
            {formatMoney(
              doc.snapshot.buy.total_amount_cents,
              doc.snapshot.currency,
            )}{" "}
            inclusive {doc.snapshot.currency || "USD"}
          </p>
          <small>
            Issued {new Date(doc.issued_at).toLocaleString()} by {doc.issued_by}
          </small>
          {doc.current && doc.basis_changed && (
            <p className="release-hold" role="status">
              The current rate, issuing identity, or stop details differ from
              this snapshot. Review and issue a new revision before sharing
              updated instructions.
            </p>
          )}
          <div className="rate-confirmation-actions">
            <a
              className="secondary text-link"
              href={href(`/api/freight/rate-confirmations/${doc.id}/statement`)}
            >
              <Download size={14} /> Download carrier confirmation
            </a>
            <button
              className="secondary"
              disabled={!canOperate || busy}
              onClick={() => open({ kind: "handoff", doc })}
            >
              Record actual handoff
            </button>
            {doc.current &&
              !(workspace.portal_offers || []).some(
                (v) => v.rate_confirmation_id === doc.id,
              ) && (
                <button
                  className="secondary"
                  disabled={
                    !canOperate || busy || !canIssue || doc.basis_changed
                  }
                  onClick={() => open({ kind: "publish", doc })}
                >
                  Publish to carrier portal
                </button>
              )}
            {(workspace.portal_offers || [])
              .filter((v) => v.rate_confirmation_id === doc.id)
              .map((v) => (
                <small key={v.id}>
                  Published to carrier portal{" "}
                  {new Date(v.published_at).toLocaleString()} · {v.published_by}
                </small>
              ))}
          </div>
          <details className="rate-confirmation-evidence">
            <summary>Issued snapshot and history</summary>
            <p className="rate-terms">{doc.snapshot.terms}</p>
            <p>Reason: {doc.reason}</p>
            <small>Document SHA-256: {doc.sha256}</small>
            {doc.supersedes_id && <small>Replaces {doc.supersedes_id}</small>}
            {!doc.handoffs.length && <p>No external handoff recorded.</p>}
            {doc.handoffs.map((h) => (
              <div className="rate-handoff" key={h.id}>
                <strong>
                  {h.retraction
                    ? "Retracted handoff evidence"
                    : h.coordination_required && !h.resolution
                      ? "Coordination required"
                      : "External handoff recorded"}
                </strong>
                <p>
                  {h.channel} · {h.carrier_contact} · {h.reference}
                </p>
                <p>Sent {new Date(h.sent_at).toLocaleString()}</p>
                <p>{h.note}</p>
                <small>
                  Recorded by {h.recorded_by} ·{" "}
                  {new Date(h.recorded_at).toLocaleString()}
                </small>
                {h.document_id && (
                  <a
                    href={href(
                      `/api/freight/documents/${h.document_id}/content`,
                    )}
                  >
                    Download original handoff evidence
                  </a>
                )}
                {h.retraction && (
                  <p>
                    Correction: {h.retraction.note} · {h.retraction.recorded_by}
                  </p>
                )}
                {h.resolution && (
                  <p>
                    Carrier coordination: {h.resolution.carrier_contact} ·{" "}
                    {h.resolution.reference} · {h.resolution.note} ·{" "}
                    {h.resolution.recorded_by}
                  </p>
                )}
                <div className="rate-confirmation-actions">
                  {h.coordination_required &&
                    !h.resolution &&
                    !h.retraction && (
                      <button
                        className="secondary"
                        disabled={!canOperate || busy}
                        onClick={() =>
                          open({ kind: "resolve", doc, handoff: h })
                        }
                      >
                        Resolve handoff coordination
                      </button>
                    )}
                  {!h.retraction && (
                    <button
                      className="secondary"
                      disabled={!canOperate || busy}
                      onClick={() => open({ kind: "retract", doc, handoff: h })}
                    >
                      Correct erroneous handoff
                    </button>
                  )}
                </div>
              </div>
            ))}
          </details>
          {doc.handoffs.some(
            (h) => h.coordination_required && !h.resolution && !h.retraction,
          ) && (
            <p className="release-hold" role="status">
              A historical or changed document was handed off. Resolve carrier
              coordination before new transport commitments.
            </p>
          )}
        </article>
      ))}
      {modal && (
        <Modal
          title={
            modal.kind === "publish"
              ? "Publish carrier portal offer"
              : modal.kind === "issue"
                ? "Issue carrier rate confirmation"
                : modal.kind === "handoff"
                  ? "Record actual carrier handoff"
                  : modal.kind === "resolve"
                    ? "Resolve carrier handoff coordination"
                    : "Correct erroneous handoff evidence"
          }
          subtitle={load.reference}
          close={() => !busy && setModal(null)}
        >
          <form onSubmit={submit}>
            <div className="form-body">
              {error && (
                <div className="form-error" role="alert">
                  {error}
                </div>
              )}
              {modal.kind === "publish" ? (
                <p>
                  Publish this exact carrier confirmation to the assigned
                  carrier's configured portal users. This counts as
                  communicating the offer and prevents unsent withdrawal, even
                  with zero EDI attempts. Carrier acceptance and EDI transport
                  remain separate. No email or message is sent.
                </p>
              ) : modal.kind === "issue" ? (
                <>
                  <p>
                    Uses the current carrier buy agreement and approved
                    additions. Customer sell price, margin, bank details, and
                    private procurement evidence are excluded. Configure your
                    issuing legal identity in Freight audit.
                  </p>
                  <Field title="Actual carrier terms">
                    <textarea
                      name="terms"
                      required
                      maxLength={4000}
                      rows={6}
                      defaultValue={modal.doc?.snapshot.terms || ""}
                    />
                  </Field>
                </>
              ) : modal.kind === "handoff" ? (
                <>
                  <p>
                    Record an actual external communication and the exact
                    document you sent. This does not send a message or accept
                    the load for the carrier. Historical or changed documents
                    require coordination review.
                  </p>
                  <Field title="Actual sent time (UTC)">
                    <input
                      type="datetime-local"
                      step="0.001"
                      name="sent_at"
                      required
                      defaultValue={new Date().toISOString().slice(0, -1)}
                    />
                  </Field>
                  <Field title="Handoff channel">
                    <select name="channel">
                      <option value="email">Email</option>
                      <option value="portal">Carrier portal</option>
                      <option value="other">Other external channel</option>
                    </select>
                  </Field>
                  <Field
                    title="Actual carrier contact"
                    name="carrier_contact"
                    maxLength={200}
                  />
                  <Field
                    title="External handoff reference"
                    name="reference"
                    maxLength={200}
                  />
                  <Field title="Handoff evidence file (optional)">
                    <select name="document_id">
                      <option value="">Evidence held externally</option>
                      {(workspace.documents || [])
                        .filter(
                          (v) =>
                            v.load_id === load.id && v.status !== "rejected",
                        )
                        .map((v) => (
                          <option key={v.id} value={v.id}>
                            {v.filename}
                          </option>
                        ))}
                    </select>
                  </Field>
                </>
              ) : modal.kind === "resolve" ? (
                <>
                  <p>
                    Contact the carrier and resolve which terms and tender
                    apply. Record actual findings. This decision retains current
                    ownership and does not establish release, acceptance,
                    custody, or a charge.
                  </p>
                  <Field
                    title="Actual carrier contact"
                    name="carrier_contact"
                    maxLength={200}
                  />
                  <Field
                    title="Carrier coordination reference"
                    name="reference"
                    maxLength={200}
                  />
                </>
              ) : (
                <p>
                  Retract only an erroneous handoff entry. The original evidence
                  remains. A real sent offer requires actual carrier release;
                  correcting this record does not recall a message.
                </p>
              )}
              {modal.kind !== "publish" && (
                <Field
                  title={
                    modal.kind === "issue"
                      ? "Issuance or revision reason"
                      : "Actual findings"
                  }
                >
                  <textarea name="note" required maxLength={200} rows={3} />
                </Field>
              )}
            </div>
            <div className="modal-footer">
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => setModal(null)}
              >
                Cancel
              </button>
              <button type="submit" className="primary" disabled={busy}>
                {busy
                  ? "Saving…"
                  : modal.kind === "publish"
                    ? "Publish carrier offer"
                    : modal.kind === "issue"
                      ? "Issue rate confirmation"
                      : modal.kind === "handoff"
                        ? "Record handoff evidence"
                        : modal.kind === "resolve"
                          ? "Record carrier coordination"
                          : "Retract erroneous evidence"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
