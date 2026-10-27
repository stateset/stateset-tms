import { useFreightIdentity } from "./FreightIdentity";
import { useRef, useState } from "react";
import { Download, FileCheck2, Upload, ShieldCheck } from "lucide-react";
import { DocumentSharing } from "./DocumentSharing";

const title = (value) =>
  value.replaceAll("_", " ").replace(/\b\w/g, (c) => c.toUpperCase());
const status = (document) =>
  document.reviews.at(-1)?.decision || "pending_review";

export function ShipmentDocuments({
  load,
  documents = [],
  capabilities = [],
  onChanged,
  locked,
  sharingContext = {},
}) {
  const { fetch, href: identityHref } = useFreightIdentity();
  const [file, setFile] = useState(null),
    [kind, setKind] = useState("pod");
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [review, setReview] = useState(null);
  const pending = useRef(null),
    fileInput = useRef(null);
  const canUpload = capabilities.includes("documents.write"),
    canReview = capabilities.includes("finance.write"),
    canOperate = capabilities.includes("operations.write");
  async function mutate(path, body, contentType, fingerprint) {
    setError("");
    setBusy(true);
    if (pending.current?.fingerprint !== fingerprint)
      pending.current = { fingerprint, key: crypto.randomUUID() };
    try {
      const response = await fetch(`/api/freight/${path}`, {
        method: "POST",
        headers: {
          "Content-Type": contentType,
          "Idempotency-Key": pending.current.key,
        },
        body,
      });
      const result = await response.json();
      if (!response.ok)
        throw new Error(result.error?.message || "Document action failed");
      pending.current = null;
      await onChanged();
      return true;
    } catch (problem) {
      setError(problem.message);
      return false;
    } finally {
      setBusy(false);
    }
  }
  async function upload(event) {
    event.preventDefault();
    if (!file) return;
    if (file.size > 10 * 1024 * 1024 || !file.size) {
      setError("Choose a file between 1 byte and 10 MiB.");
      return;
    }
    setBusy(true);
    try {
      const bytes = await file.arrayBuffer();
      const digest = [
        ...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)),
      ]
        .map((v) => v.toString(16).padStart(2, "0"))
        .join("");
      const path = `loads/${load.id}/documents?${new URLSearchParams({ kind, filename: file.name })}`;
      const type =
        file.type ||
        (file.name.toLowerCase().endsWith(".pdf")
          ? "application/pdf"
          : "application/octet-stream");
      if (
        await mutate(path, bytes, type, JSON.stringify([path, type, digest]))
      ) {
        setFile(null);
        if (fileInput.current) fileInput.current.value = "";
      }
    } catch (problem) {
      setError(problem.message || "Unable to read the selected file.");
    } finally {
      setBusy(false);
    }
  }
  const command = async (document) => {
    const path = `loads/${load.id}/commands`,
      body = JSON.stringify({
        action: "attach_pod",
        document_id: document.id,
        expected_version: load.version,
      });
    await mutate(path, body, "application/json", path + body);
  };
  return (
    <section className="shipment-documents" aria-label="Shipment documents">
      <div className="panel-heading">
        <div>
          <h3>
            <FileCheck2 size={17} /> Shipment documents
          </h3>
          <p>
            Original files remain unchanged. Finance reviews delivery evidence
            before invoice approval.
          </p>
        </div>
      </div>
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {canUpload && (
        <form className="document-upload" onSubmit={upload}>
          <label className="field">
            <span>Document type</span>
            <select
              aria-label="Document type"
              disabled={busy}
              value={kind}
              onChange={(event) => setKind(event.target.value)}
            >
              {[
                "pod",
                "bill_of_lading",
                "rate_confirmation",
                "carrier_invoice",
                "other",
              ].map((value) => (
                <option key={value} value={value}>
                  {value === "pod" ? "Proof of delivery" : title(value)}
                </option>
              ))}
            </select>
          </label>
          <label className="field">
            <span>PDF, JPEG, or PNG · up to 10 MiB</span>
            <input
              ref={fileInput}
              aria-label="Shipment document file"
              type="file"
              accept="application/pdf,image/jpeg,image/png"
              disabled={busy}
              onChange={(event) => setFile(event.target.files[0] || null)}
              required
            />
          </label>
          <button className="secondary" disabled={busy || !file}>
            <Upload size={14} />
            {busy ? "Saving…" : "Upload document"}
          </button>
        </form>
      )}
      {load.pod_reference && !load.pod_document_id && (
        <div className="external-evidence">
          <strong>External proof of delivery</strong>
          <span
            role="status"
            className={`badge ${load.pod_review?.decision || "pending_review"}`}
          >
            {title(load.pod_review?.decision || "pending_review")}
          </span>
          <p>{load.pod_reference}</p>
          {load.pod_review && (
            <p>
              {load.pod_review.note} · {load.pod_review.reviewed_by}
            </p>
          )}
          <button
            className="secondary"
            disabled={!canReview || busy}
            onClick={() => setReview({ external: true, version: load.version })}
          >
            <ShieldCheck size={14} />
            Review external POD
          </button>
        </div>
      )}
      <div className="document-list">
        {documents.map((document) => (
          <article className="document-card" key={document.id}>
            <div className="document-heading">
              <div>
                <strong>{document.filename}</strong>
                <small>
                  {document.kind === "pod"
                    ? "Proof of delivery"
                    : title(document.kind)}{" "}
                  · {Math.ceil(document.size_bytes / 1024)} KiB ·{" "}
                  {document.uploaded_by}
                </small>
              </div>
              <span role="status" className={`badge ${status(document)}`}>
                {title(status(document))}
              </span>
            </div>
            <small className="document-checksum">
              SHA-256 <code>{document.sha256}</code>
            </small>
            <div className="toolbar-actions">
              <a
                className="secondary document-download"
                href={identityHref(
                  `/api/freight/documents/${document.id}/content`,
                )}
              >
                <Download size={14} />
                Download {document.filename}
              </a>
              {document.kind === "pod" && (
                <button
                  className="secondary"
                  disabled={
                    busy ||
                    !canOperate ||
                    load.status !== "delivered" ||
                    locked ||
                    status(document) === "rejected" ||
                    load.pod_document_id === document.id
                  }
                  onClick={() => command(document)}
                >
                  {load.pod_document_id === document.id
                    ? "Attached as POD"
                    : "Use as POD"}
                </button>
              )}
              <button
                className="secondary"
                disabled={busy || !canReview}
                onClick={() =>
                  setReview({
                    id: document.id,
                    version: document.version,
                    filename: document.filename,
                  })
                }
              >
                Review document
              </button>
            </div>
            <DocumentSharing
              load={load}
              document={document}
              capabilities={capabilities}
              onChanged={onChanged}
              {...sharingContext}
            />
            {document.reviews.length > 0 && (
              <details>
                <summary>Review history ({document.reviews.length})</summary>
                {document.reviews.map((entry, index) => (
                  <p key={index}>
                    <strong>{title(entry.decision)}</strong> ·{" "}
                    {entry.reviewed_by} ·{" "}
                    {new Date(entry.reviewed_at).toLocaleString()}
                    <br />
                    {entry.note}
                  </p>
                ))}
              </details>
            )}
          </article>
        ))}
        {!documents.length && (
          <p className="muted">No stored files attached to this shipment.</p>
        )}
      </div>
      {review && (
        <form
          className="document-review"
          aria-label="Delivery evidence review"
          onSubmit={async (event) => {
            event.preventDefault();
            const values = Object.fromEntries(
              new FormData(event.currentTarget),
            );
            const path = review.external
              ? `loads/${load.id}/pod/review`
              : `documents/${review.id}/review`;
            const body = JSON.stringify({
              ...values,
              expected_version: review.version,
            });
            if (await mutate(path, body, "application/json", path + body))
              setReview(null);
          }}
        >
          <h4>
            Review{" "}
            {review.external ? "external delivery evidence" : review.filename}
          </h4>
          <p>
            Inspect the original evidence, confirm it belongs to this shipment,
            and record your decision. A file upload alone does not verify a
            delivery or signature.
          </p>
          <label className="field">
            <span>Decision</span>
            <select
              autoFocus
              name="decision"
              aria-label="Evidence review decision"
            >
              <option value="accepted">Accept evidence</option>
              <option value="rejected">Reject evidence</option>
            </select>
          </label>
          <label className="field">
            <span>Review note</span>
            <textarea
              name="note"
              aria-label="Evidence review note"
              required
              maxLength={200}
              placeholder="What did you verify, or why is the evidence insufficient?"
            />
          </label>
          <div className="toolbar-actions">
            <button
              className="secondary"
              type="button"
              disabled={busy}
              onClick={() => setReview(null)}
            >
              Cancel review
            </button>
            <button className="primary" disabled={busy || !canReview}>
              Record review
            </button>
          </div>
        </form>
      )}
    </section>
  );
}
