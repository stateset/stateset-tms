import { useFreightIdentity } from "./FreightIdentity";
import { useRef, useState } from "react";
import { AlertTriangle, ShieldCheck, Truck } from "lucide-react";
import { ReleasePortalStaff } from "./ReleasePortalStaff";

const title = (value) =>
  value.replaceAll("_", " ").replace(/\b\w/g, (v) => v.toUpperCase());
export function TenderHistory({ load, workspace, onChanged }) {
  const { fetch } = useFreightIdentity();
  const [form, setForm] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const pending = useRef(null),
    canOperate = workspace.access.capabilities.includes("operations.write");
  const tenders = (workspace.tenders || []).filter(
      (v) => v.load_id === load.id,
    ),
    releases = (workspace.releases || []).filter((v) => v.load_id === load.id),
    incidents = (workspace.tender_incidents || []).filter(
      (v) => v.load_id === load.id,
    );
  const active = tenders.find((v) => v.id === load.tender_id),
    release = releases.find(
      (v) => v.status === "pending" && v.tender_id === load.tender_id,
    );
  const canRequest =
    load.carrier_id &&
    ["tendered", "accepted", "dispatched"].includes(load.status) &&
    !release &&
    !load.tender_control?.can_withdraw;
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
        throw new Error(
          result.error?.message || "Carrier coordination action failed",
        );
      pending.current = null;
      await onChanged();
      setForm(null);
    } catch (problem) {
      setError(problem.message);
    } finally {
      setBusy(false);
    }
  }
  function open(value) {
    setError("");
    setForm(value);
  }
  return (
    <section
      className="tender-history"
      aria-label="Tender history and carrier release"
    >
      <h3>
        <Truck size={17} /> Carrier tenders and release
      </h3>
      {active && (
        <div className="active-tender">
          <div>
            <span>Current carrier shipment ID</span>
            <strong>{active.shipment_reference}</strong>
            <small>
              {active.carrier.name} · attempt {active.sequence}
              {active.legacy ? " · legacy identity retained" : ""}
            </small>
          </div>
          <span className={`badge ${active.status}`}>
            {title(active.status)}
          </span>
        </div>
      )}
      {release && (
        <div className="release-hold" role="status">
          <AlertTriangle size={17} />
          <div>
            <strong>
              Carrier release pending · dispatch and reassignment on hold
            </strong>
            <p>{release.reason}</p>
            <small>
              Requested by {release.requested_by} ·{" "}
              {new Date(release.requested_at).toLocaleString()} ·{" "}
              {release.disposition === "cancel"
                ? "Cancel shipment after release"
                : "Return to planning after release"}
            </small>
          </div>
        </div>
      )}
      <ReleasePortalStaff
        load={load}
        workspace={workspace}
        onChanged={onChanged}
      />
      {incidents
        .filter((v) => v.status === "open")
        .map((incident) => (
          <article className="tender-incident" key={incident.id}>
            <strong>
              <AlertTriangle size={15} /> Carrier custody or invoice conflict
            </strong>
            <p>
              Event {incident.event.event_id} belongs to tender{" "}
              {tenders.find((v) => v.id === incident.tender_id)
                ?.shipment_reference || incident.tender_id}
              . Dispatch, new tender transmission, and financial approval
              require reconciliation.
            </p>
            <details>
              <summary>Original carrier event</summary>
              <pre>{JSON.stringify(incident.event, null, 2)}</pre>
            </details>
            <button
              className="secondary"
              disabled={!canOperate || busy}
              onClick={() => open({ kind: "incident", incident })}
            >
              Resolve carrier conflict
            </button>
          </article>
        ))}
      <div className="tender-actions">
        {load.tender_control?.can_withdraw && (
          <button
            className="secondary"
            disabled={!canOperate || busy}
            onClick={() => open({ kind: "withdraw" })}
          >
            Withdraw unsent tender
          </button>
        )}
        {canRequest && (
          <button
            className="secondary"
            disabled={!canOperate || busy}
            onClick={() => open({ kind: "request" })}
          >
            Request carrier release
          </button>
        )}
        {release && (
          <>
            <button
              className="secondary"
              disabled={!canOperate || busy}
              onClick={() => open({ kind: "confirm", release })}
            >
              Confirm carrier released
            </button>
            <button
              className="secondary"
              disabled={!canOperate || busy}
              onClick={() => open({ kind: "retain", release })}
            >
              Confirm carrier retained
            </button>
          </>
        )}
      </div>
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {form && (
        <form
          className="carrier-release-form"
          aria-label="Carrier coordination"
          onSubmit={async (event) => {
            event.preventDefault();
            const values = Object.fromEntries(
              new FormData(event.currentTarget),
            );
            if (form.kind === "withdraw")
              return mutate(`loads/${load.id}/commands`, {
                action: "withdraw_tender",
                expected_version: load.version,
              });
            const path =
              form.kind === "request"
                ? `loads/${load.id}/release-requests`
                : form.kind === "incident"
                  ? `tender-incidents/${form.incident.id}/resolve`
                  : `release-requests/${form.release.id}/${form.kind}`;
            await mutate(path, {
              ...values,
              expected_load_version: load.version,
            });
          }}
        >
          <h4>
            {
              {
                request: "Request external carrier release",
                confirm: "Record carrier release confirmation",
                retain: "Record carrier commitment retained",
                withdraw: "Withdraw proven-unsent offer",
                incident: "Record external conflict resolution",
              }[form.kind]
            }
          </h4>
          {form.kind === "withdraw" ? (
            <p>
              This offer has never entered a gateway handoff. Withdraw it and
              return this load to planning.
            </p>
          ) : form.kind === "request" ? (
            <>
              <p>
                Record the coordination task, then contact the carrier through
                your established channel. This does not transmit an electronic
                cancellation.
              </p>
              <label className="field">
                <span>After carrier release</span>
                <select aria-label="After carrier release" name="disposition">
                  <option value="replan">
                    Return to planning for re-tendering
                  </option>
                  <option value="cancel">Cancel the shipment</option>
                </select>
              </label>
              <label className="field">
                <span>Release reason</span>
                <textarea
                  aria-label="Release reason"
                  name="reason"
                  required
                  maxLength={200}
                />
              </label>
            </>
          ) : (
            <>
              <p>
                {form.kind === "incident"
                  ? "Record the external coordination that establishes the current assignment is correct and resolves this evidence conflict. The shipment assignment is retained."
                  : form.kind === "confirm"
                    ? "Confirm the carrier released this exact tender and will not pick it up. The recorded disposition will take effect."
                    : "Confirm the carrier remains committed to this tender. This closes the release hold without changing shipment ownership."}
              </p>
              <label className="field">
                <span>Carrier contact</span>
                <input
                  aria-label="Carrier contact"
                  name="carrier_contact"
                  required
                  maxLength={200}
                />
              </label>
              <label className="field">
                <span>Carrier confirmation reference</span>
                <input
                  aria-label="Carrier confirmation reference"
                  name="confirmation_reference"
                  required
                  maxLength={200}
                />
              </label>
              <label className="field">
                <span>Carrier coordination note</span>
                <textarea
                  aria-label="Carrier coordination note"
                  name="note"
                  required
                  maxLength={200}
                />
              </label>
            </>
          )}
          <div className="tender-actions">
            <button
              className="secondary"
              type="button"
              disabled={busy}
              onClick={() => setForm(null)}
            >
              Cancel coordination entry
            </button>
            <button className="primary" disabled={busy || !canOperate}>
              {busy
                ? "Saving…"
                : {
                    withdraw: "Confirm withdrawal",
                    request: "Create release request",
                    confirm: "Record release confirmation",
                    retain: "Record retained confirmation",
                    incident: "Record conflict resolution",
                  }[form.kind]}
            </button>
          </div>
        </form>
      )}
      <div className="tender-attempt-list">
        {[...tenders].reverse().map((tender) => (
          <details key={tender.id}>
            <summary>
              Attempt {tender.sequence} · {tender.carrier.name} ·{" "}
              {title(tender.status)}
            </summary>
            <p className="tender-reference">
              Carrier shipment ID: <strong>{tender.shipment_reference}</strong>
            </p>
            <small>
              {tender.created_by} ·{" "}
              {new Date(tender.created_at).toLocaleString()}
              {tender.legacy
                ? " · legacy snapshot adopted; original handoff time not inferred"
                : ""}
            </small>
            {workspace.outbox
              .filter((v) => v.tender_id === tender.id)
              .map((job) => (
                <p key={job.id}>
                  {title(job.status)} · {job.total_attempts || job.attempts}{" "}
                  total attempts · transmission{" "}
                  {title(job.transmission || "unknown")}
                </p>
              ))}
            {releases
              .filter((v) => v.tender_id === tender.id)
              .map((entry) => (
                <div className="release-evidence" key={entry.id}>
                  <strong>Release {title(entry.status)}</strong>
                  <p>{entry.reason}</p>
                  {entry.confirmation && (
                    <p>
                      <ShieldCheck size={12} />{" "}
                      {entry.confirmation.carrier_contact} ·{" "}
                      {entry.confirmation.reference}
                      <br />
                      {entry.confirmation.note}
                      <br />
                      <small>
                        {entry.confirmation.recorded_by} ·{" "}
                        {new Date(
                          entry.confirmation.recorded_at,
                        ).toLocaleString()}
                      </small>
                    </p>
                  )}
                </div>
              ))}
          </details>
        ))}
        {incidents
          .filter((v) => v.status === "resolved")
          .map((incident) => (
            <details key={incident.id}>
              <summary>
                Resolved carrier conflict · {incident.event.event_id}
              </summary>
              <p>
                {incident.resolution?.carrier_contact} ·{" "}
                {incident.resolution?.reference}
              </p>
              <p>{incident.resolution?.note}</p>
            </details>
          ))}
        {!tenders.length && load.carrier_id && (
          <p className="muted">
            Existing carrier assignment uses the original load reference. Its
            transmission history is uncertain until reconciled.
          </p>
        )}
      </div>
    </section>
  );
}
