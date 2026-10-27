export function PartnerFiles({ files, identity }) {
  if (!files.length) return null;
  return (
    <section aria-label="Shared shipment files">
      <h2>Shared shipment files</h2>
      {files
        .slice()
        .reverse()
        .map((f) => (
          <article
            key={f.id}
            className="partner-card"
            aria-label={`Shared file ${f.filename} for ${f.load_reference}`}
          >
            <div className="partner-card-heading">
              <h3>{f.filename}</h3>
              <span
                className={`badge ${f.available ? "accepted" : "cancelled"}`}
              >
                {f.status === "revoked"
                  ? "Access revoked"
                  : f.available
                    ? "Available"
                    : "Access on hold"}
              </span>
            </div>
            <p>
              {f.load_reference}
              {f.shipment_reference ? ` · ${f.shipment_reference}` : ""} ·{" "}
              {f.kind.replaceAll("_", " ")} · {Math.ceil(f.size_bytes / 1024)}{" "}
              KiB
            </p>
            <p className="partner-terms">{f.message}</p>
            <p>
              Shared {new Date(f.shared_at).toLocaleString()} · File review:{" "}
              {f.review_status.replaceAll("_", " ")}
            </p>
            {f.available && (
              <a
                className="secondary text-link"
                href={identity.href(
                  `/api/freight/portal/files/${f.id}/content`,
                )}
              >
                Download shared file
              </a>
            )}
            {!f.available && (
              <p>Contact your transportation team about file access.</p>
            )}
            <details>
              <summary>File integrity</summary>
              <p className="partner-terms">SHA-256 {f.sha256}</p>
            </details>
          </article>
        ))}
    </section>
  );
}
