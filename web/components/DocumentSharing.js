import { useRef, useState } from "react";
import { Modal } from "./Modal";
import { useFreightIdentity } from "./FreightIdentity";
export function DocumentSharing({
  load,
  document,
  capabilities,
  onChanged,
  shares = [],
  tenders = [],
  customerName = "Customer",
  carrierName = "Carrier",
}) {
  const identity = useFreightIdentity(),
    pending = useRef(null);
  const [modal, setModal] = useState(null),
    [recipient, setRecipient] = useState(""),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const history = shares.filter((s) => s.document_id === document.id),
    attempts = tenders.filter((t) => t.load_id === load.id);
  const canWrite = capabilities.includes("documents.write"),
    finance = capabilities.includes("finance.write"),
    financial = ["carrier_invoice", "rate_confirmation"].includes(
      document.kind,
    );
  const rejected = document.reviews.at(-1)?.decision === "rejected";
  const context = modal?.context;
  const selected = (context?.attempts || attempts).find(
      (t) => recipient === `carrier:${t.id}`,
    ),
    audience = recipient === "customer" ? "customer" : "carrier";
  const target =
    recipient === "customer"
      ? context?.customerName || customerName
      : selected?.carrier.name || context?.carrierName || carrierName;
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const values = new FormData(event.currentTarget);
    let path, body;
    if (modal.revoke) {
      path = `document-shares/${modal.revoke.id}/revoke`;
      body = {
        expected_version: modal.revoke.version,
        note: values.get("note"),
      };
    } else {
      const previous = context.history
        .filter(
          (s) =>
            s.audience === audience &&
            s.counterparty_id ===
              (audience === "customer"
                ? context.customerId
                : selected?.carrier_id || context.carrierId) &&
            s.tender_id ===
              (audience === "customer" ? null : selected?.id || null),
        )
        .at(-1);
      path = `documents/${document.id}/shares`;
      body = {
        expected_load_version: context.loadVersion,
        expected_document_version: context.documentVersion,
        audience,
        carrier_tender_id: audience === "carrier" ? selected?.id || null : null,
        expected_previous_id: previous?.id || null,
        message: values.get("message"),
        review_note: values.get("note"),
        acknowledge_content: values.get("acknowledge") === "on",
      };
    }
    const fingerprint = JSON.stringify([path, body]);
    if (pending.current?.fingerprint !== fingerprint)
      pending.current = { fingerprint, key: crypto.randomUUID() };
    try {
      const r = await identity.fetch(`/api/freight/${path}`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "Idempotency-Key": pending.current.key,
        },
        body: JSON.stringify(body),
      });
      const v = await r.json();
      if (!r.ok)
        throw new Error(v.error?.message || "File access update failed");
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
      className="file-sharing"
      aria-label={`Partner access ${document.filename}`}
    >
      <button
        className="secondary"
        disabled={!canWrite || busy || rejected || (financial && !finance)}
        onClick={() => {
          setError("");
          setRecipient("");
          setModal({
            context: {
              loadVersion: load.version,
              documentVersion: document.version,
              customerId: load.customer_id,
              customerName,
              carrierId: load.carrier_id,
              carrierName,
              legacy: !!load.carrier_id && !load.tender_id,
              attempts,
              history,
            },
          });
        }}
      >
        Share file with partner
      </button>
      {financial && (
        <p>
          Carrier financial files require finance permission and can be shared
          only with a carrier. Customer invoices and quotes use their own portal
          documents.
        </p>
      )}
      {!!history.length && (
        <details>
          <summary>Partner file access ({history.length})</summary>
          {history
            .slice()
            .reverse()
            .map((s) => (
              <section className="file-share-history" key={s.id}>
                <strong>
                  {s.counterparty_name} · {s.audience}
                  {s.shipment_reference ? ` · ${s.shipment_reference}` : ""}
                </strong>
                <p>
                  {s.revocation
                    ? "Access revoked"
                    : rejected
                      ? "Access held by file review"
                      : "Access available"}{" "}
                  · shared {new Date(s.shared_at).toLocaleString()} ·{" "}
                  {s.shared_by}
                </p>
                <p>Recipient message: {s.message}</p>
                <p>Private content review: {s.review_note}</p>
                {s.revocation && (
                  <p>
                    Revoked {new Date(s.revocation.revoked_at).toLocaleString()}{" "}
                    · {s.revocation.revoked_by}: {s.revocation.note}
                  </p>
                )}
                <details>
                  <summary>
                    Download authorizations ({s.download_authorizations.length})
                  </summary>
                  <p>
                    Authorization records do not establish network receipt or
                    erase a partner's retained copies.
                  </p>
                  {s.download_authorizations.map((a) => (
                    <p key={a.id}>
                      {new Date(a.authorized_at).toLocaleString()} ·{" "}
                      {a.authorized_by} · version {a.share_version}
                    </p>
                  ))}
                </details>
                {!s.revocation && canWrite && (
                  <button
                    className="secondary"
                    disabled={busy}
                    onClick={() => {
                      setError("");
                      setModal({ revoke: s });
                    }}
                  >
                    Revoke file access
                  </button>
                )}
              </section>
            ))}
        </details>
      )}
      {modal && (
        <Modal
          title={
            modal.revoke
              ? "Revoke partner file access"
              : "Share original file with partner"
          }
          subtitle={document.filename}
          close={() => !busy && setModal(null)}
        >
          <form onSubmit={submit}>
            <div className="form-body">
              {error && (
                <div className="form-error" role="alert">
                  {error}
                </div>
              )}
              {modal.revoke ? (
                <p>
                  Remove future file access for {modal.revoke.counterparty_name}
                  . This keeps the original document, financial evidence, and
                  authorization history. Previously downloaded copies remain
                  with the recipient.
                </p>
              ) : (
                <>
                  <a
                    className="text-link"
                    href={identity.href(
                      `/api/freight/documents/${document.id}/content`,
                    )}
                  >
                    Inspect original file before sharing
                  </a>
                  <label className="field">
                    <span>File recipient</span>
                    <select
                      required
                      value={recipient}
                      onChange={(e) => setRecipient(e.target.value)}
                    >
                      <option value="">Select the exact recipient</option>
                      {!financial && (
                        <option value="customer">
                          Customer · {context.customerName}
                        </option>
                      )}
                      {context.attempts.map((t) => (
                        <option key={t.id} value={`carrier:${t.id}`}>
                          Carrier · {t.carrier.name} · tender {t.sequence} ·{" "}
                          {t.status} · {t.shipment_reference}
                        </option>
                      ))}
                      {context.legacy && (
                        <option value="carrier:current">
                          Current carrier · {context.carrierName} · legacy
                          assignment
                        </option>
                      )}
                    </select>
                  </label>
                  {recipient && (
                    <p>
                      Share this exact file with {target}.{" "}
                      {audience === "carrier"
                        ? "Carrier file publication counts as communication of the selected tender, even if access is later revoked."
                        : "No other customer's account receives access."}
                    </p>
                  )}
                  <label className="field">
                    <span>Message for recipient</span>
                    <textarea
                      name="message"
                      required
                      maxLength={200}
                      rows={2}
                    />
                  </label>
                  <label className="partner-ack">
                    <input name="acknowledge" type="checkbox" required />I
                    inspected this exact file and filename and confirm they are
                    appropriate for the selected recipient.
                  </label>
                </>
              )}
              <label className="field">
                <span>
                  {modal.revoke
                    ? "Private revocation reason"
                    : "Private content review note"}
                </span>
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
                {busy
                  ? "Saving…"
                  : modal.revoke
                    ? "Revoke partner access"
                    : "Publish file access"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
