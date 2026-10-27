import { useCallback, useEffect, useRef, useState } from "react";
import { RefreshCw, Search, ShieldAlert, UserCheck } from "lucide-react";
import { useFreightIdentity } from "./FreightIdentity";
import { Modal } from "./Modal";
import { ExecutionTimeline } from "./ExecutionTimeline";

const instant = (v) => (v ? new Date(v).toLocaleString() : "Not scheduled");
const future = (minutes) =>
  new Date(Date.now() + minutes * 60000).toISOString().slice(0, 16);
const title = (s) =>
  s.replaceAll("_", " ").replace(/\b\w/g, (c) => c.toUpperCase());
function SourceFacts({ signal, workspace }) {
  if (signal.kind === "execution_conflict")
    return <ExecutionTimeline timeline={signal.source} />;
  const hidden = new Set([
    "event_id",
    "tender_id",
    "review_id",
    "outbox_id",
    "inbox_id",
    "digest",
  ]);
  const load = workspace.loads.find((l) => l.id === signal.load_id);
  const stopName = (id) =>
    id === "pickup"
      ? load?.origin.facility
      : id === "delivery"
        ? load?.destination.facility
        : load?.intermediate_stops.find((s) => s.id === id)?.stop.facility ||
          id;
  const labels = {
    missing_stop_ids: "Unconfirmed facilities",
    last_tracking_at: "Last recorded tracking time",
    tracking_stale_minutes: "Tracking follow-up interval (minutes)",
    ends_at: "Confirmed window ends",
    expires_at: "Carrier response deadline",
    review_state: "Delivery evidence review",
    reason: "Reported condition",
    stop_id: "Facility",
    document_id: "Original document",
    reference: "Original evidence reference",
    error: "Last processing error",
    status: "Current handoff state",
    destination: "Handoff destination",
    total_attempts: "Retained attempts",
  };
  const values = Object.entries(signal.source).filter(
    ([key, v]) =>
      !hidden.has(key) &&
      v !== null &&
      (typeof v !== "object" ||
        (Array.isArray(v) && v.every((x) => typeof x === "string"))),
  );
  return (
    <dl className="operations-source-facts">
      {values.map(([key, value]) => (
        <div key={key}>
          <dt>{labels[key] || title(key)}</dt>
          <dd>
            {key === "missing_stop_ids"
              ? value.map(stopName).join(", ")
              : key === "stop_id"
                ? stopName(value)
                : key === "document_id" && signal.kind !== "inbound_failure"
                  ? workspace.documents.find((d) => d.id === value)?.filename ||
                    "Original delivery evidence"
                  : Array.isArray(value)
                    ? value.join(", ")
                    : ["last_tracking_at", "ends_at", "expires_at"].includes(
                          key,
                        )
                      ? instant(value)
                      : String(value)}
          </dd>
        </div>
      ))}
    </dl>
  );
}

export function OperationsWorkspace({
  workspace,
  onChanged,
  openLoad,
  openIntegrations,
}) {
  const identity = useFreightIdentity(),
    sequence = useRef(0),
    pending = useRef(null),
    paging = useRef(false),
    lastWorkspace = useRef(workspace);
  const [data, setData] = useState(null),
    [view, setView] = useState("actionable"),
    [search, setSearch] = useState("");
  const [loading, setLoading] = useState(false),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false),
    [form, setForm] = useState(null),
    [formError, setFormError] = useState("");
  paging.current = (data?.issues.length || 0) > 25;
  const capabilities = workspace.access.capabilities,
    canWrite = capabilities.includes("operations.write"),
    canConfigure = capabilities.includes("operations.configure"),
    subject = workspace.access.subject;
  const readPage = useCallback(
    async (cursor = null) => {
      const generation = ++sequence.current;
      setLoading(true);
      setError("");
      try {
        const query = new URLSearchParams({ view, search, limit: "25" });
        if (cursor) query.set("cursor", cursor);
        const r = await identity.fetch(`/api/freight/operations?${query}`),
          v = await r.json();
        if (!r.ok)
          throw new Error(v.error?.message || "Operations queue unavailable");
        if (generation === sequence.current)
          setData((previous) => ({
            ...v,
            issues: cursor
              ? [...(previous?.issues || []), ...v.issues]
              : v.issues,
          }));
      } catch (e) {
        if (generation === sequence.current) setError(e.message);
      } finally {
        if (generation === sequence.current) setLoading(false);
      }
    },
    [identity, view, search],
  );
  useEffect(() => {
    readPage();
    const timer = setInterval(() => {
      if (!paging.current) readPage();
    }, 30000);
    return () => {
      sequence.current++;
      clearInterval(timer);
    };
  }, [readPage]);
  useEffect(() => {
    if (lastWorkspace.current !== workspace) {
      lastWorkspace.current = workspace;
      if (!paging.current) readPage();
    }
  }, [workspace, readPage]);
  async function write(path, body) {
    const fingerprint = JSON.stringify([path, body]);
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
      v = await r.json();
    if (!r.ok) throw new Error(v.error?.message || "Operations action failed");
    pending.current = null;
    return v.data;
  }
  async function refresh() {
    setBusy(true);
    setError("");
    try {
      if (canWrite) await write("operations/reconcile", {});
      await readPage();
      await onChanged();
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  async function open(row, action) {
    setBusy(true);
    setError("");
    try {
      const r = await identity.fetch(
          `/api/freight/operations/issues/${row.id}`,
        ),
        v = await r.json();
      if (!r.ok) throw new Error(v.error?.message || "Issue unavailable");
      setFormError("");
      setForm({ row: v, action });
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  async function submit(e) {
    e.preventDefault();
    setBusy(true);
    setFormError("");
    try {
      const v = Object.fromEntries(new FormData(e.currentTarget));
      if (form.action === "policy")
        await write("operations/policy", {
          expected_version: form.policy.version,
          tracking_stale_minutes: Number(v.interval),
          note: v.note,
        });
      else
        await write(`operations/issues/${form.row.id}/actions`, {
          expected_version: form.row.case?.version || 0,
          expected_source_active: form.row.source_active,
          source_sha256: form.row.source_sha256,
          action: form.action,
          note: v.note,
          follow_up_at: v.follow_up_at
            ? new Date(v.follow_up_at + "Z").toISOString()
            : null,
          defer_until: v.defer_until
            ? new Date(v.defer_until + "Z").toISOString()
            : null,
        });
      setForm(null);
      await readPage();
      await onChanged();
    } catch (e) {
      setFormError(e.message);
    } finally {
      setBusy(false);
    }
  }
  const metrics = data?.metrics || workspace.operations?.metrics || {};
  const labels = {
    claim: "Take ownership",
    release: "Release ownership",
    force_release: "Release another owner",
    note: "Add follow-up note",
    snooze: "Defer follow-up",
    resume: "Resume follow-up",
    history: "Issue history",
    policy: "Tracking follow-up policy",
  };
  return (
    <section
      className="operations-workspace"
      aria-label="Operations control tower"
    >
      {paging.current && (
        <p className="section-note">
          Automatic refresh is paused while you review multiple pages. Refresh
          issues to restart from the current queue.
        </p>
      )}
      <div className="operations-intro">
        <div>
          <h2>Work that needs a decision</h2>
          <p>
            Current shipment facts become owned work. Clear the underlying
            problem to resolve an issue; notes and ownership preserve its
            history.
          </p>
        </div>
        <div className="toolbar-actions">
          {canConfigure && (
            <button
              className="secondary"
              onClick={() => {
                setFormError("");
                setForm({
                  action: "policy",
                  policy: data?.policy || workspace.operations.policy,
                });
              }}
            >
              Tracking policy
            </button>
          )}
          <button
            className="secondary"
            disabled={busy || loading}
            onClick={refresh}
          >
            <RefreshCw size={15} />
            Refresh issues
          </button>
        </div>
      </div>
      <div className="operations-metrics">
        {[
          ["Active issues", metrics.active],
          ["Critical holds", metrics.critical],
          ["Unassigned", metrics.unowned],
          ["Follow-ups overdue", metrics.follow_ups_overdue],
          ["Deferred", metrics.deferred],
        ].map(([name, value]) => (
          <div className="panel" key={name}>
            <span>{name}</span>
            <strong>{value ?? "—"}</strong>
          </div>
        ))}
      </div>
      {metrics.detected_cycles > 0 && (
        <p className="section-note">
          First response:{" "}
          {metrics.median_first_response_seconds == null
            ? metrics.acknowledged_cycles > 0
              ? "timing unavailable"
              : "no claims recorded"
            : `${metrics.median_first_response_seconds} seconds median`}{" "}
          · {metrics.acknowledged_cycles} of {metrics.detected_cycles} detected
          cycles claimed. Timing starts when detection is durably recorded.
          {metrics.response_time_unavailable_cycles > 0 &&
            ` ${metrics.response_time_unavailable_cycles} response timings unavailable; no duration is inferred from clock-skewed observations.`}
        </p>
      )}
      {data?.evidence_valid === false && (
        <div className="form-error" role="alert">
          <strong>Operations evidence needs recovery.</strong> Retained case
          history or policy cannot be verified. Source facts remain visible;
          recover original records before changing ownership.
        </div>
      )}
      {error && (
        <div className="form-error" role="alert">
          {error}
        </div>
      )}
      <div className="panel operations-list">
        <div className="table-toolbar">
          <div className="search">
            <Search size={16} />
            <input
              aria-label="Search operations issues"
              placeholder="Search shipment or issue…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <label className="operations-view">
            Queue view
            <select
              aria-label="Operations queue view"
              value={view}
              onChange={(e) => setView(e.target.value)}
            >
              {[
                ["actionable", "Needs action"],
                ["active", "All active"],
                ["mine", "Assigned to me"],
                ["unowned", "Unassigned"],
                ["deferred", "Deferred"],
                ["resolved", "Source cleared"],
                ["all", "All history"],
              ].map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
          </label>
        </div>
        {!data && loading && (
          <p className="empty-state" role="status">
            Loading current operations…
          </p>
        )}
        {data?.issues.length === 0 && (
          <div className="empty-state">
            <UserCheck size={25} />
            <h3>No issues in this view</h3>
            <p>Change the view or search to inspect other work.</p>
          </div>
        )}
        {data?.issues.map((row) => {
          // Opening an action fetches fresh issue detail. A background list
          // refresh must not disable the button midway through a pointer click.
          const c = row.case,
            owns = row.owner === subject,
            closed = !row.source_active,
            enabled = canWrite && row.evidence_valid && !busy,
            priority = ["Critical", "High", "Normal"][row.signal.priority];
          return (
            <article
              className="operations-issue"
              aria-label={`${row.signal.title} · ${row.load_reference || "Integration"}`}
              key={row.id}
            >
              <div className="operations-issue-heading">
                <div>
                  <span
                    className={`operations-priority p${row.signal.priority}`}
                  >
                    <ShieldAlert size={13} />
                    {priority}
                  </span>
                  <h3>{row.signal.title}</h3>
                  <p>
                    {row.load_reference || "Integration recovery"} ·{" "}
                    {!row.evidence_valid
                      ? "Ownership history needs recovery"
                      : closed
                        ? "Source condition cleared"
                        : row.deferred
                          ? `Deferred until ${instant(c.deferred_until)}`
                          : row.owner
                            ? "In progress"
                            : "Awaiting an owner"}
                  </p>
                </div>
                {!row.evidence_valid ? (
                  <span className="operations-priority p0">
                    Unverified history
                  </span>
                ) : (
                  closed && (
                    <span className="operations-cleared">Source cleared</span>
                  )
                )}
              </div>
              <p>{row.signal.summary}</p>
              <p className="operations-next">{row.signal.next_action}</p>
              <div className="operations-ownership">
                <span>
                  Owner{" "}
                  <strong>
                    {!row.evidence_valid
                      ? "Unverified"
                      : row.owner || "Unassigned"}
                  </strong>
                </span>
                <span className={row.follow_up_overdue ? "form-error" : ""}>
                  Next follow-up{" "}
                  <strong>
                    {!row.evidence_valid
                      ? "Unverified"
                      : instant(
                          !closed && c?.status !== "resolved"
                            ? c?.follow_up_at
                            : null,
                        )}
                  </strong>
                </span>
              </div>
              <div className="operations-actions">
                {row.signal.load_id ? (
                  <button
                    className="secondary"
                    onClick={() => openLoad(row.signal.load_id)}
                  >
                    Review shipment
                  </button>
                ) : (
                  <button className="secondary" onClick={openIntegrations}>
                    Review integration
                  </button>
                )}
                {!closed && !row.owner && (
                  <button
                    className="primary"
                    disabled={!enabled}
                    onClick={() => open(row, "claim")}
                  >
                    Take ownership
                  </button>
                )}
                {!closed && owns && (
                  <>
                    <button
                      className="secondary"
                      disabled={!enabled}
                      onClick={() => open(row, "note")}
                    >
                      Update follow-up
                    </button>
                    <button
                      className="secondary"
                      disabled={
                        !enabled || (!row.deferred && row.signal.priority === 0)
                      }
                      onClick={() =>
                        open(row, row.deferred ? "resume" : "snooze")
                      }
                    >
                      {row.deferred ? "Resume follow-up" : "Defer follow-up"}
                    </button>
                    <button
                      className="secondary"
                      disabled={!enabled}
                      onClick={() => open(row, "release")}
                    >
                      Release ownership
                    </button>
                  </>
                )}
                {(!owns || closed) && (
                  <button
                    className="secondary"
                    disabled={!enabled}
                    onClick={() => open(row, "note")}
                  >
                    Add note
                  </button>
                )}
                {!closed && row.owner && !owns && canConfigure && (
                  <button
                    className="secondary"
                    disabled={!enabled}
                    onClick={() => open(row, "force_release")}
                  >
                    Release another owner
                  </button>
                )}
                <button
                  className="text-link"
                  disabled={busy}
                  onClick={() => open(row, "history")}
                >
                  View issue history
                </button>
              </div>
            </article>
          );
        })}
        <div className="table-footer">
          {data
            ? `${data.issues.length} of ${data.total} matching issues`
            : "Current source conditions"}
          {data?.next_cursor && (
            <button
              className="secondary"
              disabled={loading || busy}
              onClick={() => readPage(data.next_cursor)}
            >
              Load more issues
            </button>
          )}
        </div>
      </div>
      {form && (
        <Modal
          title={labels[form.action]}
          subtitle={
            form.row
              ? `${form.row.load_reference || "Integration recovery"} · ${form.row.signal.title}`
              : "Workspace policy"
          }
          close={() => !busy && setForm(null)}
          wide={form.action === "history"}
        >
          {form.action === "history" ? (
            <div className="operations-history">
              {!form.row.evidence_valid && (
                <div className="form-error" role="alert">
                  These historical records cannot be verified. Recover the
                  original evidence before relying on ownership or timing.
                </div>
              )}
              <SourceFacts signal={form.row.signal} workspace={workspace} />
              {!form.row.case && (
                <p>
                  {form.row.evidence_valid
                    ? "No triage actions have been recorded yet."
                    : "Retained history is unavailable until its original records recover."}
                </p>
              )}
              {form.row.case?.history.map((h, i) => (
                <article key={i}>
                  <strong>
                    {title(h.action)} · cycle {h.cycle}
                  </strong>
                  <small>
                    {h.actor} · {instant(h.at)}
                  </small>
                  <p>{h.note}</p>
                  {h.source && (
                    <details>
                      <summary>Captured source evidence</summary>
                      <SourceFacts signal={h.source} workspace={workspace} />
                    </details>
                  )}
                </article>
              ))}
            </div>
          ) : (
            <form className="modal-form" onSubmit={submit}>
              {formError && (
                <div className="form-error" role="alert">
                  {formError}
                </div>
              )}
              {form.action === "policy" ? (
                <>
                  <p>
                    Choose how long an in-transit shipment may go without an
                    actual carrier update before it enters the work queue. This
                    changes follow-up detection, not shipment truth.
                  </p>
                  <label className="field">
                    <span>Tracking freshness interval (minutes)</span>
                    <input
                      name="interval"
                      type="number"
                      min="15"
                      max="10080"
                      defaultValue={form.policy.tracking_stale_minutes}
                      required
                    />
                  </label>
                </>
              ) : (
                <>
                  <p>{form.row.signal.next_action}</p>
                  <SourceFacts signal={form.row.signal} workspace={workspace} />
                  <p className="section-note">
                    {form.row.owner ? `Current owner: ${form.row.owner}. ` : ""}
                    Ownership and notes do not change carrier custody, delivery
                    evidence, source authorization, or payments.
                  </p>
                </>
              )}
              {(form.action === "claim" ||
                (form.action === "note" &&
                  form.row.owner === subject &&
                  form.row.source_active)) && (
                <label className="field">
                  <span>Next follow-up (UTC)</span>
                  <input
                    name="follow_up_at"
                    type="datetime-local"
                    step="0.001"
                    defaultValue={
                      form.action === "claim"
                        ? future(60)
                        : form.row.case?.follow_up_at?.replace(/Z$/, "")
                    }
                    required={form.action === "claim"}
                  />
                </label>
              )}
              {form.action === "snooze" && (
                <label className="field">
                  <span>Defer until (UTC)</span>
                  <input
                    name="defer_until"
                    type="datetime-local"
                    required
                    defaultValue={future(15)}
                  />
                </label>
              )}
              <label className="field">
                <span>
                  {form.action === "policy"
                    ? "Policy change reason"
                    : "Follow-up findings and next step"}
                </span>
                <textarea name="note" required maxLength={200} rows={3} />
              </label>
              {form.action !== "policy" && (
                <label className="partner-ack">
                  <input type="checkbox" required />I reviewed this issue’s
                  current source evidence and the action I am recording.
                </label>
              )}
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
                  {busy
                    ? "Saving…"
                    : form.action === "policy"
                      ? "Save tracking policy"
                      : "Record follow-up action"}
                </button>
              </div>
            </form>
          )}
        </Modal>
      )}
    </section>
  );
}
