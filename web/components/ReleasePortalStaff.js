import { useRef, useState } from "react";
import { Modal } from "./Modal";
import { useFreightIdentity } from "./FreightIdentity";
const utc = (v) =>
  new Intl.DateTimeFormat("en-US", {
    timeZone: "UTC",
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(v)) + " UTC";
export function ReleasePortalStaff({ load, workspace, onChanged }) {
  const identity = useFreightIdentity(),
    pending = useRef(null);
  const [modal, setModal] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const releases = workspace.releases.filter((r) => r.load_id === load.id),
    publications = (workspace.release_publications || []).filter(
      (p) => p.load_id === load.id,
    ),
    responses = (workspace.release_responses || []).filter(
      (r) => r.load_id === load.id,
    ),
    canWrite = workspace.access.capabilities.includes("operations.write");
  const current = releases.find(
      (r) => r.status === "pending" && r.tender_id === load.tender_id,
    ),
    carrier = workspace.carriers.find((c) => c.id === load.carrier_id);
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const f = new FormData(event.currentTarget);
    let path, body;
    if (modal.response) {
      path = `release-responses/${modal.response.id}/apply`;
      body = { expected_load_version: modal.version };
    } else {
      path = `release-requests/${modal.release.id}/portal-publications`;
      body = {
        expected_load_version: modal.version,
        expected_previous_id: modal.previous_id || null,
        message: f.get("message"),
        respond_by: new Date(`${f.get("respond_by")}Z`).toISOString(),
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
        throw new Error(v.error?.message || "Carrier release action failed");
      pending.current = null;
      setModal(null);
      await onChanged();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  if (!releases.length) return null;
  return (
    <section
      className="release-portal-staff"
      aria-label="Authenticated carrier release"
    >
      {current && (
        <button
          className="secondary"
          disabled={!canWrite || busy}
          onClick={() => {
            setError("");
            setModal({
              release: current,
              version: load.version,
              previous_id:
                publications.filter((p) => p.release_id === current.id).at(-1)
                  ?.id || null,
              carrierName: carrier?.name,
            });
          }}
        >
          {publications.some((p) => p.release_id === current.id)
            ? "Publish revised release request"
            : "Publish release request to carrier portal"}
        </button>
      )}
      {!!publications.length && (
        <details>
          <summary>
            Published carrier release requests ({publications.length})
          </summary>
          {publications
            .slice()
            .reverse()
            .map((p) => (
              <section key={p.id}>
                <p>
                  {p.snapshot.carrier_name} · {p.snapshot.shipment_reference} ·{" "}
                  {p.snapshot.message}
                </p>
                <p>
                  Response/application deadline: {utc(p.snapshot.respond_by)} ·
                  published by {p.published_by}
                </p>
                <a
                  className="text-link"
                  href={identity.href(
                    `/api/freight/release-publications/${p.id}/statement`,
                  )}
                >
                  Download published release request
                </a>
              </section>
            ))}
        </details>
      )}
      {responses.map((r) => {
        const p = publications.find((p) => p.id === r.publication_id),
          latest = publications
            .filter((p) => p.release_id === r.release_id)
            .at(-1);
        const canApply =
          r.decision === "release" &&
          !r.application &&
          current?.id === r.release_id &&
          latest?.id === r.publication_id &&
          new Date(p.snapshot.respond_by) > new Date();
        return (
          <section
            className="release-response-proof"
            key={r.id}
            aria-label={`Carrier release evidence ${r.reference}`}
          >
            <h4>Authenticated carrier {r.decision}</h4>
            <p>
              {r.contact} · {r.reference} · {r.recorded_by}
            </p>
            <p>{r.note}</p>
            <p>
              Recorded {utc(r.recorded_at)} · Initial result:{" "}
              {r.outcome.replaceAll("_", " ")}
              {r.initial_hold
                ? ` · ${r.initial_hold.replaceAll("_", " ")}`
                : ""}
            </p>
            {r.application && (
              <p>
                Applied {utc(r.application.applied_at)} ·{" "}
                {r.application.applied_by}
              </p>
            )}
            {canApply && (
              <button
                className="primary"
                disabled={!canWrite || busy}
                onClick={() => {
                  setError("");
                  setModal({
                    response: r,
                    version: load.version,
                    carrierName: carrier?.name,
                  });
                }}
              >
                Apply recorded carrier release
              </button>
            )}
          </section>
        );
      })}
      {modal && (
        <Modal
          title={
            modal.response
              ? "Apply authenticated carrier release"
              : "Publish carrier release request"
          }
          subtitle={modal.carrierName || load.reference}
          close={() => !busy && setModal(null)}
        >
          <form onSubmit={submit}>
            <div className="form-body">
              {error && (
                <div className="form-error" role="alert">
                  {error}
                </div>
              )}
              {modal.response ? (
                <>
                  <p>
                    Apply the original carrier's no-custody acknowledgement from{" "}
                    {utc(modal.response.recorded_at)}:{" "}
                    {modal.response.reference}. The server rechecks the current
                    request, deadline, movement, financial records and handoff
                    lease.
                  </p>
                  <label className="partner-ack">
                    <input type="checkbox" required />I inspected the original
                    carrier evidence and reconciled current holds.
                  </label>
                </>
              ) : (
                <>
                  <p>
                    Publish the current pending request to {modal.carrierName}.
                    Confirmed release will{" "}
                    {modal.release.disposition === "cancel"
                      ? "cancel this shipment"
                      : "return this shipment to planning"}
                    . Earlier publications and responses remain in history. The
                    private operations reason is excluded.
                  </p>
                  <label className="field">
                    <span>Message for carrier</span>
                    <textarea
                      name="message"
                      required
                      maxLength={200}
                      rows={3}
                    />
                  </label>
                  <label className="field">
                    <span>Release response/application deadline (UTC)</span>
                    <input
                      name="respond_by"
                      type="datetime-local"
                      required
                      defaultValue={new Date(Date.now() + 86400000)
                        .toISOString()
                        .slice(0, 16)}
                    />
                  </label>
                  <label className="partner-ack">
                    <input type="checkbox" required />I reviewed the exact
                    carrier, assignment, disposition and deadline for this
                    publication.
                  </label>
                </>
              )}
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
                  : modal.response
                    ? "Apply carrier acknowledgement"
                    : "Publish carrier release request"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
