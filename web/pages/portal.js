import Head from "next/head";
import dynamic from "next/dynamic";
import { useCallback, useEffect, useRef, useState } from "react";
import { publicAuthConfig } from "../lib/auth-config.mjs";
import { currencyCode, formatMoney } from "../lib/money.mjs";
import { useFreightIdentity } from "../components/FreightIdentity";
import { Modal } from "../components/Modal";
import { CustomerQuotes } from "../components/CustomerQuotes";
import { PartnerFiles } from "../components/PartnerFiles";
import { ItineraryExecution } from "../components/ItineraryExecution";
import { ExecutionTimeline } from "../components/ExecutionTimeline";
import { CarrierReleaseRequests } from "../components/CarrierReleaseRequests";
const ClerkWorkspace = dynamic(() => import("../components/ClerkWorkspace"), {
  ssr: false,
});
const money = (c, currency) => formatMoney(c, currency);
const evidenceKind = {
  pickup: "Pickup",
  location: "In-transit location",
  delivery: "Delivery",
  pod: "Proof of delivery",
};
const evidenceOutcome = {
  applied: "Recorded",
  stale_ignored: "Earlier observation retained; current progress unchanged",
  attached_unreviewed: "Submitted for finance review",
};
export async function getServerSideProps({ res }) {
  res.setHeader("Cache-Control", "no-store");
  return { props: { authConfig: publicAuthConfig() } };
}
export default function PartnerPage({ authConfig }) {
  return authConfig?.mode === "clerk" ? (
    <ClerkWorkspace
      publishableKey={authConfig.publishableKey}
      destination="/portal"
      partner
    >
      <Portal />
    </ClerkWorkspace>
  ) : (
    <Portal />
  );
}
function Stop({ stop, title }) {
  return (
    <section className="partner-stop">
      <h3>{title}</h3>
      <strong>{stop.facility}</strong>
      <p>
        {stop.address_lines.join(", ")}
        <br />
        {stop.city}, {stop.state} {stop.postal_code} {stop.country}
      </p>
      <p>
        {stop.date} · {stop.contact} {stop.phone}
      </p>
      {stop.instructions && (
        <p className="partner-terms">{stop.instructions}</p>
      )}
    </section>
  );
}
function Plan({ plan }) {
  return (
    <>
      <p>
        {plan.equipment} · {plan.commodity} · {plan.weight_lbs.toLocaleString()}{" "}
        lb
      </p>
      <div className="partner-stops">
        <Stop stop={plan.origin} title="Pickup" />
        {(plan.intermediate_stops || []).map((s, i) => (
          <Stop key={s.id} stop={s.stop} title={`Stop ${i + 2} · ${s.kind}`} />
        ))}
        <Stop stop={plan.destination} title="Delivery" />
      </div>
    </>
  );
}
function Portal() {
  const identity = useFreightIdentity();
  const [data, setData] = useState(null),
    [loading, setLoading] = useState(true),
    [login, setLogin] = useState(false),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [modal, setModal] = useState(null),
    [message, setMessage] = useState("");
  const pending = useRef(null);
  const refreshSequence = useRef(0),
    explicitlySignedOut = useRef(false);
  const refresh = useCallback(async () => {
    if (explicitlySignedOut.current) return false;
    const generation = ++refreshSequence.current;
    try {
      const r = await identity.fetch("/api/freight/portal");
      const result = await r.json();
      if (!r.ok)
        throw Object.assign(
          new Error(result.error?.message || "Partner portal unavailable"),
          { status: r.status },
        );
      if (generation !== refreshSequence.current) return false;
      setData(result);
      setLogin(false);
      setError("");
      return true;
    } catch (e) {
      if (generation !== refreshSequence.current) return false;
      setError(e.message);
      if ([401, 403].includes(e.status)) {
        setData(null);
        setModal(null);
        setMessage("");
        setLogin(true);
        pending.current = null;
      }
      return false;
    } finally {
      if (generation === refreshSequence.current) setLoading(false);
    }
  }, [identity]);
  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 30000);
    return () => {
      refreshSequence.current++;
      clearInterval(timer);
    };
  }, [refresh]);
  async function signIn(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const token = new FormData(event.currentTarget).get("credential");
      const r = await fetch("/api/portal-session", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ token }),
      });
      const v = await r.json();
      if (!r.ok) throw new Error(v.error?.message || "Sign-in failed");
      explicitlySignedOut.current = false;
      await refresh();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  async function signOut() {
    explicitlySignedOut.current = true;
    refreshSequence.current++;
    setData(null);
    setModal(null);
    setMessage("");
    setLogin(true);
    pending.current = null;
    if (identity.signOut) return identity.signOut();
    await fetch("/api/portal-session", { method: "DELETE" });
    setLogin(true);
    setError("");
  }
  async function respond(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const form = new FormData(event.currentTarget),
      o = modal.offer;
    const body = {
      expected_load_version: o.load_version,
      decision: modal.decision,
      reference: form.get("reference"),
      note: form.get("note"),
      rate_confirmation_id: o.rate_confirmation_id,
      rate_sha256: o.rate_sha256,
    };
    const path = `/api/freight/portal/offers/${o.id}/response`,
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
      const result = await r.json();
      if (!r.ok)
        throw new Error(result.error?.message || "Carrier response failed");
      pending.current = null;
      setModal(null);
      setMessage(
        modal.decision === "accept"
          ? "Carrier acceptance recorded for this exact offer."
          : "Carrier decline recorded; the original offer remains in history.",
      );
      await refresh();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  async function submitEvidence(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const form = new FormData(event.currentTarget),
      o = modal.offer;
    try {
      const isPod = modal.reporting === "pod";
      const fields = {
        expected_load_version: o.load_version,
        reference: form.get("reference"),
        note: form.get("note"),
      };
      let path = `/api/freight/portal/offers/${o.id}/${isPod ? "pod" : "tracking"}`;
      let body, contentType, fingerprint;
      if (isPod) {
        const file = form.get("file");
        if (!file?.size || file.size > 10 * 1024 * 1024)
          throw new Error("Choose a PDF, JPEG, or PNG up to 10 MiB.");
        const bytes = await file.arrayBuffer();
        const hash = Array.from(
          new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)),
          (v) => v.toString(16).padStart(2, "0"),
        ).join("");
        path += `?${new URLSearchParams({ ...fields, filename: file.name })}`;
        body = bytes;
        contentType = file.type;
        fingerprint = JSON.stringify([path, contentType, hash]);
      } else {
        body = JSON.stringify({
          ...fields,
          kind: form.get("kind"),
          location: form.get("location"),
          observed_at: new Date(`${form.get("observed_at")}Z`).toISOString(),
        });
        contentType = "application/json";
        fingerprint = JSON.stringify([path, body]);
      }
      if (pending.current?.fingerprint !== fingerprint)
        pending.current = { fingerprint, key: crypto.randomUUID() };
      const r = await identity.fetch(path, {
        method: "POST",
        headers: {
          "Content-Type": contentType,
          "Idempotency-Key": pending.current.key,
        },
        body,
      });
      const result = await r.json();
      if (!r.ok)
        throw new Error(
          result.error?.message || "Carrier evidence could not be recorded",
        );
      pending.current = null;
      setModal(null);
      setMessage(
        isPod
          ? "POD submitted. Finance must review the original file before approval."
          : result.data?.outcome === "stale_ignored"
            ? "Earlier observation retained in history. Latest shipment progress is unchanged."
            : "Carrier observation recorded against this shipment assignment.",
      );
      await refresh();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  if (loading)
    return (
      <main className="login">
        <p role="status">Loading partner portal…</p>
      </main>
    );
  if (login || !data)
    return (
      <main className="login">
        <section className="login-card">
          <div className="login-brand">STATESET FREIGHT</div>
          <h1>Partner portal</h1>
          <p>
            Access shipments and documents for your approved carrier or customer
            account.
          </p>
          {error && (
            <div className="form-error" role="alert">
              {error}
            </div>
          )}
          {identity.mode === "credential" ? (
            <form onSubmit={signIn}>
              <label className="field">
                <span>Partner credential</span>
                <input
                  name="credential"
                  type="password"
                  required
                  minLength={32}
                  maxLength={512}
                  autoComplete="current-password"
                />
              </label>
              <button className="primary" disabled={busy}>
                Open partner portal
              </button>
            </form>
          ) : (
            <button className="secondary" onClick={refresh}>
              Refresh partner access
            </button>
          )}
          <a href="/">Staff workspace</a>
        </section>
      </main>
    );
  return (
    <>
      <Head>
        <title>Partner portal · StateSet Freight</title>
      </Head>
      <main className="partner-workspace">
        <header className="partner-header">
          <div>
            <span>STATESET FREIGHT · {data.kind.toUpperCase()} PORTAL</span>
            <h1>{data.name}</h1>
            <p>{data.access.subject}</p>
          </div>
          <div className="partner-actions">
            <button className="secondary" onClick={refresh}>
              Refresh shipments
            </button>
            <button className="secondary" onClick={signOut}>
              Sign out
            </button>
          </div>
        </header>
        {data.mode === "demo" && (
          <p className="partner-message">
            Synthetic demo workspace · no live carrier or customer activity
          </p>
        )}
        {error && (
          <div className="form-error" role="alert">
            {error}
          </div>
        )}
        {message && (
          <p role="status" className="partner-message">
            {message}
          </p>
        )}
        {data.kind === "carrier" ? (
          <>
            <CarrierReleaseRequests
              requests={data.release_requests || []}
              identity={identity}
              onChanged={refresh}
            />
            <h2>Carrier offers and shipments</h2>
            {!data.offers.length && (
              <p>No carrier offers have been published to your account.</p>
            )}
            {data.offers
              .slice()
              .reverse()
              .map((o) => (
                <article
                  className="partner-card"
                  key={o.id}
                  aria-label={`Carrier offer ${o.plan.reference} revision ${o.rate_revision}`}
                >
                  <div className="partner-card-heading">
                    <h2>{o.plan.reference}</h2>
                    <span className={`badge ${o.status}`}>
                      {o.is_current ? o.status : "Historical offer"}
                    </span>
                  </div>
                  <p>
                    {o.shipment_reference} · revision {o.rate_revision}
                  </p>
                  <strong className="partner-price">
                    {money(o.amount_cents, o.currency)} inclusive{" "}
                    {currencyCode(o.currency)}
                  </strong>
                  <Plan plan={o.plan} />
                  {o.windows.map((w, i) => (
                    <p key={i}>
                      {w.stop} appointment: {w.start_local} – {w.end_local} (
                      {w.timezone})
                    </p>
                  ))}
                  {o.location && (
                    <p>
                      Latest location: {o.location}
                      {o.last_tracking_at
                        ? ` · ${new Date(o.last_tracking_at).toLocaleString()}`
                        : ""}
                    </p>
                  )}
                  <details>
                    <summary>Carrier terms</summary>
                    <p className="partner-terms">{o.terms}</p>
                  </details>
                  {o.awaiting_updated_confirmation && o.is_current && (
                    <p role="status">
                      Operations is preparing an updated confirmation.
                      Acceptance requires the current published revision.
                    </p>
                  )}
                  {o.is_current &&
                    o.can_decline &&
                    !o.can_accept &&
                    !o.awaiting_updated_confirmation && (
                      <p role="status">
                        Acceptance is on hold for operations review. Contact
                        your transportation team before committing.
                      </p>
                    )}
                  {o.decision && (
                    <p>
                      Recorded {o.decision.decision}: {o.decision.reference} ·{" "}
                      {o.decision.recorded_by}
                    </p>
                  )}
                  {o.is_current &&
                    o.reporting.pod_status !== "missing" &&
                    o.reporting.pod_status && (
                      <p>
                        POD review:{" "}
                        {o.reporting.pod_status.replaceAll("_", " ")}
                      </p>
                    )}
                  <ItineraryExecution offer={o} onChanged={refresh} />
                  <ExecutionTimeline
                    timeline={o.reporting.execution_timeline}
                  />
                  <div className="partner-actions">
                    {!!o.reporting.tracking_kinds.length && (
                      <button
                        className="primary"
                        disabled={busy}
                        onClick={() => {
                          setError("");
                          setModal({ offer: o, reporting: "tracking" });
                        }}
                      >
                        Report shipment progress
                      </button>
                    )}
                    {o.reporting.can_submit_pod && (
                      <button
                        className="primary"
                        disabled={busy}
                        onClick={() => {
                          setError("");
                          setModal({ offer: o, reporting: "pod" });
                        }}
                      >
                        Submit POD
                      </button>
                    )}
                    <a
                      className="secondary text-link"
                      href={identity.href(
                        `/api/freight/portal/rate-confirmations/${o.rate_confirmation_id}/statement`,
                      )}
                    >
                      Download carrier confirmation
                    </a>
                    {o.can_accept && (
                      <button
                        className="primary"
                        disabled={busy}
                        onClick={() => {
                          setError("");
                          setModal({ offer: o, decision: "accept" });
                        }}
                      >
                        Accept carrier offer
                      </button>
                    )}
                    {o.can_decline && (
                      <button
                        className="secondary"
                        disabled={busy}
                        onClick={() => {
                          setError("");
                          setModal({ offer: o, decision: "decline" });
                        }}
                      >
                        Decline carrier offer
                      </button>
                    )}
                  </div>
                  {!!o.reporting.submissions.length && (
                    <details>
                      <summary>
                        Your shipment evidence ({o.reporting.submissions.length}
                        )
                      </summary>
                      {o.reporting.submissions
                        .slice()
                        .reverse()
                        .map((s) => (
                          <section
                            key={s.id}
                            aria-label={`Carrier evidence ${s.reference}`}
                          >
                            <h3>
                              {evidenceKind[s.kind] || s.kind} · {s.reference}
                            </h3>
                            <p>{s.note}</p>
                            <p>
                              {s.observed_at
                                ? `Observed ${new Date(s.observed_at).toLocaleString()} · ${s.location} · `
                                : ""}
                              {evidenceOutcome[s.outcome] ||
                                s.outcome.replaceAll("_", " ")}
                            </p>
                            {s.document && (
                              <p>
                                {s.document.status.replaceAll("_", " ")} ·{" "}
                                <a
                                  className="partner-file-link"
                                  href={identity.href(
                                    `/api/freight/portal/submissions/${s.id}/content`,
                                  )}
                                >
                                  Download your POD: {s.document.filename}
                                </a>
                              </p>
                            )}
                            <p>
                              Recorded{" "}
                              {new Date(s.recorded_at).toLocaleString()} ·{" "}
                              {s.recorded_by}
                            </p>
                          </section>
                        ))}
                    </details>
                  )}
                </article>
              ))}
          </>
        ) : (
          <>
            <CustomerQuotes
              quotes={data.quotes || []}
              identity={identity}
              onChanged={refresh}
            />
            <h2>Your shipments</h2>
            {!data.shipments.length && (
              <p>No shipments for this customer account.</p>
            )}
            {data.shipments.map((s) => (
              <article
                className="partner-card"
                key={s.id}
                aria-label={`Customer shipment ${s.plan.reference}`}
              >
                <div className="partner-card-heading">
                  <h2>{s.plan.reference}</h2>
                  <span className={`badge ${s.status}`}>{s.status}</span>
                </div>
                <Plan plan={s.plan} />
                <ExecutionTimeline timeline={s.execution_timeline} />
                {s.location && <p>Latest location: {s.location}</p>}
                {s.windows.map((w, i) => (
                  <p key={i}>
                    {w.stop} appointment: {w.start_local} – {w.end_local} (
                    {w.timezone})
                  </p>
                ))}
              </article>
            ))}
            <h2>Issued invoices</h2>
            {!data.bills.length && <p>No issued invoices.</p>}
            {data.bills.map((b) => (
              <article
                className="partner-card"
                key={b.id}
                aria-label={`Customer invoice ${b.number}`}
              >
                <h3>
                  {b.number} · {b.status}
                </h3>
                <p>
                  {b.load_reference} · {money(b.amount_cents, b.currency)} · Due{" "}
                  {b.due_date}
                </p>
                <p>
                  {b.balance_cents < 0
                    ? `${money(b.refund_due_cents, b.currency)} customer refund due`
                    : `${money(b.balance_cents, b.currency)} outstanding`}
                </p>
                <a
                  className="secondary text-link"
                  href={identity.href(
                    `/api/freight/portal/bills/${b.id}/statement`,
                  )}
                >
                  Download invoice
                </a>
              </article>
            ))}
          </>
        )}
        <PartnerFiles files={data.files || []} identity={identity} />
      </main>
      {modal && !modal.reporting && (
        <Modal
          title={
            modal.decision === "accept"
              ? "Accept published carrier offer"
              : "Decline published carrier offer"
          }
          subtitle={modal.offer.plan.reference}
          close={() => !busy && setModal(null)}
        >
          <form onSubmit={respond}>
            <div className="form-body">
              {error && (
                <div className="form-error" role="alert">
                  {error}
                </div>
              )}
              <p>
                {money(modal.offer.amount_cents, modal.offer.currency)}{" "}
                inclusive {currencyCode(modal.offer.currency)} · revision{" "}
                {modal.offer.rate_revision} · {modal.offer.shipment_reference}
              </p>
              {modal.decision === "accept" && (
                <label className="partner-ack">
                  <input type="checkbox" required />I reviewed this carrier
                  confirmation and agree to this exact rate and revision.
                </label>
              )}
              <label className="field">
                <span>Your response reference</span>
                <input name="reference" required maxLength={200} />
              </label>
              <label className="field">
                <span>Carrier response note</span>
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
                {busy ? "Recording…" : "Record carrier response"}
              </button>
            </div>
          </form>
        </Modal>
      )}
      {modal?.reporting && (
        <Modal
          title={
            modal.reporting === "pod"
              ? "Submit proof of delivery"
              : "Report shipment progress"
          }
          subtitle={modal.offer.plan.reference}
          close={() => !busy && setModal(null)}
        >
          <form onSubmit={submitEvidence}>
            <div className="form-body">
              {error && (
                <div className="form-error" role="alert">
                  {error}
                </div>
              )}
              {modal.reporting === "pod" ? (
                <>
                  <p>
                    Submit the actual receiver evidence. Finance will inspect
                    the original file; uploading does not approve an invoice or
                    certify a signature.
                  </p>
                  <label className="field">
                    <span>POD file</span>
                    <input
                      name="file"
                      type="file"
                      accept="application/pdf,image/jpeg,image/png"
                      required
                    />
                  </label>
                  <p>
                    PDF, JPEG, or PNG · up to 10 MiB. Your file remains
                    available in your submission history.
                  </p>
                </>
              ) : (
                <>
                  <p>
                    Report actual physical movement for{" "}
                    {modal.offer.shipment_reference}. Delivery status alone does
                    not supply POD.
                  </p>
                  <label className="field">
                    <span>Shipment observation</span>
                    <select name="kind" required>
                      {modal.offer.reporting.tracking_kinds.map((k) => (
                        <option key={k} value={k}>
                          {k === "pickup"
                            ? "Picked up"
                            : k === "delivery"
                              ? "Delivered"
                              : "In-transit location"}
                        </option>
                      ))}
                    </select>
                  </label>
                  <label className="field">
                    <span>Observed time (UTC)</span>
                    <input
                      name="observed_at"
                      type="datetime-local"
                      required
                      defaultValue={new Date().toISOString().slice(0, 16)}
                      max={new Date().toISOString().slice(0, 16)}
                    />
                  </label>
                  <label className="field">
                    <span>Observed location</span>
                    <input name="location" required maxLength={200} />
                  </label>
                </>
              )}
              <label className="field">
                <span>Evidence reference</span>
                <input name="reference" required maxLength={200} />
              </label>
              <label className="field">
                <span>Carrier evidence note</span>
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
                  ? "Recording…"
                  : modal.reporting === "pod"
                    ? "Submit POD for review"
                    : "Record shipment observation"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </>
  );
}
