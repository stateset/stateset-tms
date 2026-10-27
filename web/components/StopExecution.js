import { RecordHistory } from "./RecordHistory";
import { useFreightIdentity } from "./FreightIdentity";
import { useRef, useState } from "react";
import { CalendarClock, Clock, MapPin } from "lucide-react";

const zones = [
  "America/Los_Angeles",
  "America/Denver",
  "America/Chicago",
  "America/New_York",
  "America/Vancouver",
  "America/Toronto",
  "America/Phoenix",
  "UTC",
];
const localNow = (timezone) => {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat("en-CA", {
      timeZone: timezone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      hourCycle: "h23",
    })
      .formatToParts(new Date())
      .map((v) => [v.type, v.value]),
  );
  return `${parts.year}-${parts.month}-${parts.day}T${parts.hour}:${parts.minute}`;
};
const at = (value, timezone) =>
  new Date(value).toLocaleString("en-US", {
    timeZone: timezone,
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "numeric",
    minute: "2-digit",
    timeZoneName: "short",
  });
const title = (value) =>
  value.replaceAll("_", " ").replace(/\b\w/g, (v) => v.toUpperCase());

export function StopExecution({
  load,
  workspace,
  onChanged,
  readHistory,
  historyOwner,
}) {
  const { fetch } = useFreightIdentity();
  const [form, setForm] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const pending = useRef(null);
  const canOperate = workspace.access.capabilities.includes("operations.write");
  const [historyOpen, setHistoryOpen] = useState(false);
  const execution = load.stop_execution || {};
  const route = [
    { id: "pickup", stop: load.origin, kind: "pickup" },
    ...(load.intermediate_stops || []),
    { id: "delivery", stop: load.destination, kind: "delivery" },
  ];
  const label = (id) =>
    id === "pickup" || id === "delivery"
      ? title(id)
      : `Stop ${route.findIndex((s) => s.id === id) + 1} · ${route.find((s) => s.id === id)?.kind || "historical"}`;
  function open(kind, stop, record = null) {
    setError("");
    const timezone =
      record?.timezone ||
      execution[stop]?.appointment?.timezone ||
      Intl.DateTimeFormat().resolvedOptions().timeZone ||
      "UTC";
    const plan = route.find((s) => s.id === stop)?.stop || load.destination;
    setForm({
      kind,
      stop,
      record,
      version: load.version,
      defaults: {
        timezone,
        start_local: record?.start_local || `${plan.date}T09:00`,
        end_local: record?.end_local || `${plan.date}T10:00`,
        local_time: record?.local_time || localNow(timezone),
        disambiguation: record?.disambiguation || "reject",
      },
    });
  }
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const values = Object.fromEntries(new FormData(event.currentTarget));
    const body = { ...values, expected_load_version: form.version };
    let path;
    if (form.kind === "appointment") {
      path = `loads/${load.id}/appointments`;
      body.stop = form.stop;
    } else if (form.kind === "cancel")
      path = `appointments/${form.record.id}/cancel`;
    else if (form.kind === "correct")
      path = `stop-events/${form.record.id}/correct`;
    else {
      path = `loads/${load.id}/stop-events`;
      body.stop = form.stop;
      body.kind = form.kind;
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
        throw new Error(result.error?.message || "Stop update failed");
      await onChanged();
      pending.current = null;
      setForm(null);
    } catch (problem) {
      setError(problem.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section
      className="stop-execution"
      aria-label="Stop appointments and gate times"
    >
      <h3>
        <CalendarClock size={17} /> Stop appointments and gate times
      </h3>
      <p className="section-note">
        Record facility-confirmed windows and actual gate evidence. Gate times
        do not confirm pickup, delivery, or billable detention.
      </p>
      <div className="stop-cards">
        {route.map(({ id: stop, stop: plan }) => {
          const m = execution[stop] || {},
            a = m.appointment;
          const completed =
              load.itinerary?.stops.find((s) => s.id === stop)?.completed ??
              (load.status === "delivered" ||
                (stop === "pickup" && load.status === "in_transit")),
            closed = load.status === "cancelled";
          const canArrive =
            stop === "pickup"
              ? ["accepted", "dispatched", "in_transit", "delivered"].includes(
                  load.status,
                )
              : ["in_transit", "delivered"].includes(load.status);
          const canDepart =
            stop === "pickup"
              ? ["in_transit", "delivered"].includes(load.status)
              : stop === "delivery"
                ? load.status === "delivered"
                : completed;
          return (
            <article
              className="stop-card"
              key={stop}
              aria-label={`${label(stop)} stop`}
            >
              <h4>
                <MapPin size={15} /> {label(stop)}
              </h4>
              <strong>{plan.facility}</strong>
              <address>
                {(plan.address_lines || []).map((line, i) => (
                  <span key={i}>{line}</span>
                ))}
                <span>
                  {plan.city}, {plan.state} {plan.postal_code} ·{" "}
                  {plan.country || "US"}
                </span>
              </address>
              {!plan.address_lines?.length && (
                <p className="stop-warning">
                  Add a street address and postal code in the load plan before
                  tendering.
                </p>
              )}
              {(plan.contact || plan.phone) && (
                <p>
                  {plan.contact}
                  {plan.contact && plan.phone ? " · " : ""}
                  {plan.phone}
                </p>
              )}
              {plan.instructions && (
                <p className="stop-instructions">{plan.instructions}</p>
              )}
              <div className="stop-window">
                <span className="stop-label">CONFIRMED WINDOW</span>
                {a ? (
                  <>
                    <strong>
                      {at(a.starts_at, a.timezone)} —{" "}
                      {at(a.ends_at, a.timezone)}
                    </strong>
                    <small>
                      {a.timezone} · {a.confirmation_reference}
                    </small>
                    <small>Confirmed with {a.contact}</small>
                  </>
                ) : (
                  <p>No appointment recorded</p>
                )}
                {m.overdue && (
                  <span className="badge exception">
                    Window elapsed · arrival unrecorded
                  </span>
                )}
                {m.arrival && (
                  <span
                    className={`badge ${m.arrival_status === "late" ? "exception" : "accepted"}`}
                  >
                    {title(m.arrival_status)}
                    {m.late_minutes > 0
                      ? ` · ${m.late_minutes} min after window`
                      : ""}
                  </span>
                )}
              </div>
              <dl className="gate-times">
                {["arrival", "departure"].map((kind) => (
                  <div key={kind}>
                    <dt>{title(kind)}</dt>
                    <dd>
                      {m[kind] ? (
                        <>
                          <span>
                            {at(m[kind].occurred_at, m[kind].timezone)}
                          </span>
                          <small>{m[kind].reference}</small>
                          {canOperate && (
                            <button
                              className="text-link"
                              disabled={busy}
                              onClick={() => open("correct", stop, m[kind])}
                            >
                              Correct {stop} {kind}
                            </button>
                          )}
                        </>
                      ) : (
                        "Not recorded"
                      )}
                    </dd>
                  </div>
                ))}
              </dl>
              {m.dwell_minutes != null && (
                <p className="dwell">
                  <Clock size={14} /> {m.dwell_minutes} min{" "}
                  {m.dwell_complete
                    ? "recorded dwell"
                    : "elapsed since arrival"}
                </p>
              )}
              {m.arrival && !m.departure && !m.dwell_open && (
                <p className="section-note">
                  Departure time missing; completed dwell is unknown.
                </p>
              )}
              {canOperate && (
                <div className="stop-actions">
                  {!completed && !closed && !m.arrival && (
                    <>
                      <button
                        className="secondary"
                        disabled={busy}
                        onClick={() => open("appointment", stop, a)}
                      >
                        {a ? `Reschedule ${stop}` : `Book ${stop} appointment`}
                      </button>
                    </>
                  )}
                  {a && !completed && !m.arrival && (
                    <button
                      className="text-link"
                      disabled={busy}
                      onClick={() => open("cancel", stop, a)}
                    >
                      Cancel {stop} appointment
                    </button>
                  )}
                  {canArrive && !m.arrival && (
                    <button
                      className="secondary"
                      disabled={busy}
                      onClick={() => open("arrival", stop)}
                    >
                      Record {stop} arrival
                    </button>
                  )}
                  {canDepart && m.arrival && !m.departure && (
                    <button
                      className="secondary"
                      disabled={busy}
                      onClick={() => open("departure", stop)}
                    >
                      Record {stop} departure
                    </button>
                  )}
                </div>
              )}
            </article>
          );
        })}
      </div>
      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
      {form && (
        <form
          key={`${form.kind}-${form.stop}-${form.record?.id || "new"}`}
          className="stop-form"
          aria-label="Stop evidence form"
          onSubmit={submit}
        >
          <h4>
            {form.kind === "appointment"
              ? `Confirm ${label(form.stop)} appointment`
              : form.kind === "cancel"
                ? `Cancel ${label(form.stop)} appointment`
                : `${form.kind === "correct" ? "Correct" : "Record"} ${label(form.stop)} ${form.record?.kind || form.kind}`}
          </h4>
          {form.kind === "cancel" ? (
            <label className="field">
              <span>Cancellation reason and external coordination</span>
              <textarea
                name="reason"
                aria-label="Appointment cancellation reason"
                required
                maxLength={200}
                autoFocus
              />
            </label>
          ) : (
            <>
              <div className="form-grid">
                {form.kind === "appointment" ? (
                  <>
                    <label className="field">
                      <span>Window start (facility local time)</span>
                      <input
                        type="datetime-local"
                        name="start_local"
                        aria-label="Appointment window start"
                        defaultValue={form.defaults.start_local}
                        required
                        autoFocus
                      />
                    </label>
                    <label className="field">
                      <span>Window end (facility local time)</span>
                      <input
                        type="datetime-local"
                        name="end_local"
                        aria-label="Appointment window end"
                        defaultValue={form.defaults.end_local}
                        required
                      />
                    </label>
                  </>
                ) : (
                  <label className="field">
                    <span>Actual gate time (local)</span>
                    <input
                      type="datetime-local"
                      name="local_time"
                      aria-label="Actual gate local time"
                      defaultValue={form.defaults.local_time}
                      required
                      autoFocus
                    />
                  </label>
                )}
                <label className="field">
                  <span>Facility timezone</span>
                  <input
                    name="timezone"
                    aria-label="Facility timezone"
                    list="freight-stop-timezones"
                    defaultValue={form.defaults.timezone}
                    maxLength={100}
                    required
                  />
                  <datalist id="freight-stop-timezones">
                    {zones.map((zone) => (
                      <option key={zone} value={zone} />
                    ))}
                  </datalist>
                </label>
                <label className="field">
                  <span>If a daylight saving time repeats</span>
                  <select
                    name="disambiguation"
                    aria-label="Repeated local time choice"
                    defaultValue={form.defaults.disambiguation}
                  >
                    <option value="reject">Require clarification</option>
                    <option value="earlier">Use the earlier occurrence</option>
                    <option value="later">Use the later occurrence</option>
                  </select>
                </label>
              </div>
              {form.kind === "appointment" && (
                <label className="field">
                  <span>Facility contact</span>
                  <input
                    name="contact"
                    aria-label="Appointment facility contact"
                    defaultValue={form.record?.contact || ""}
                    required
                    maxLength={200}
                  />
                </label>
              )}
              <label className="field">
                <span>
                  {form.kind === "appointment"
                    ? "Confirmation reference"
                    : "Gate evidence reference"}
                </span>
                <input
                  name={
                    form.kind === "appointment"
                      ? "confirmation_reference"
                      : "reference"
                  }
                  aria-label="Stop confirmation reference"
                  required
                  maxLength={200}
                />
              </label>
              <label className="field">
                <span>
                  {form.kind === "correct"
                    ? "Correction reason and supporting evidence"
                    : "Evidence note"}
                </span>
                <textarea
                  name="note"
                  aria-label="Stop evidence note"
                  required
                  maxLength={200}
                />
              </label>
            </>
          )}
          <p className="section-note">
            {form.kind === "appointment" || form.kind === "cancel"
              ? "Confirm the change with the facility and carrier through your operating channel before recording it here."
              : "Record an observed time backed by a gate ledger, driver report, or other actual evidence. Corrections retain the original record."}
          </p>
          <div className="form-actions">
            <button
              type="button"
              className="secondary"
              disabled={busy}
              onClick={() => {
                setForm(null);
                setError("");
              }}
            >
              Close stop form
            </button>
            <button className="primary" disabled={busy}>
              {busy
                ? "Saving…"
                : form.kind === "cancel"
                  ? "Record appointment cancellation"
                  : "Save stop evidence"}
            </button>
          </div>
        </form>
      )}
      <details
        className="stop-history"
        onToggle={(event) => setHistoryOpen(event.currentTarget.open)}
      >
        <summary>Appointment and gate evidence history</summary>
        {historyOpen && (
          <>
            <RecordHistory
              kind="appointments"
              loadId={load.id}
              owner={historyOwner}
              read={readHistory}
              heading="Appointment history"
              onRefresh={onChanged}
            />
            <RecordHistory
              kind="gates"
              loadId={load.id}
              owner={historyOwner}
              read={readHistory}
              heading="Gate observation history"
              onRefresh={onChanged}
            />
          </>
        )}
      </details>
    </section>
  );
}
