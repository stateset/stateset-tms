import { useEffect, useRef, useState } from "react";

const title = (v) => (v || "").replaceAll("_", " ").replaceAll(".", " ");
const time = (v, timezone) =>
  new Date(v).toLocaleString(
    undefined,
    timezone ? { timeZone: timezone, timeZoneName: "short" } : undefined,
  );
function OriginalRecord({ record }) {
  const [open, setOpen] = useState(false);
  return (
    <details onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>Original event record</summary>
      {open && <pre>{JSON.stringify(record, null, 2)}</pre>}
    </details>
  );
}

// Pages retain a source window. A refreshed owner/filter can never adopt a
// response begun for an older owner, even before effect cleanup runs.
export function RecordHistory({
  kind,
  loadId,
  owner,
  read,
  heading,
  limit = 25,
  onRefresh,
}) {
  const Heading = kind === "events" && !loadId ? "h2" : "h4";
  const key = JSON.stringify([owner, kind, loadId || "", limit]);
  const live = useRef(key),
    sequence = useRef(0);
  live.current = key;
  const [state, setState] = useState({ key, items: [], loading: true });
  const current = state.key === key ? state : { items: [], loading: true };
  async function fetchPage(cursor = null, replace = false) {
    const ticket = ++sequence.current,
      scope = key;
    setState((old) => ({
      ...(old.key === scope ? old : { items: [] }),
      key: scope,
      loading: true,
      error: "",
      retryRefresh: false,
    }));
    try {
      const query = new URLSearchParams({ kind, limit: String(limit) });
      if (loadId) query.set("load_id", loadId);
      if (cursor) query.set("cursor", cursor);
      const page = await read(`history?${query}`, owner);
      if (live.current !== scope || sequence.current !== ticket) return;
      if (
        page.kind !== kind ||
        (page.load_id || "") !== (loadId || "") ||
        !Array.isArray(page.items) ||
        page.items.length > limit ||
        !Number.isSafeInteger(page.total) ||
        page.total < 0 ||
        !page.items.every(
          (item, index) =>
            Number.isSafeInteger(item.position) &&
            item.position > 0 &&
            typeof item.record?.id === "string" &&
            (!loadId ||
              item.record[kind === "events" ? "entity_id" : "load_id"] ===
                loadId) &&
            (index === 0 || item.position < page.items[index - 1].position),
        )
      )
        throw new Error(
          "History response could not be verified. Refresh history.",
        );
      setState((old) => ({
        key: scope,
        items:
          replace || old.key !== scope
            ? page.items
            : [...old.items, ...page.items],
        next: page.next_cursor,
        total: page.total,
        newer: page.has_new_records,
        loading: false,
        error: "",
      }));
    } catch (error) {
      if (
        live.current !== scope ||
        sequence.current !== ticket ||
        error.sessionChanged
      )
        return;
      setState((old) => ({
        ...old,
        key: scope,
        loading: false,
        error: error.message,
        changed: error.status === 409,
        retryCursor: cursor,
        retryReplace: replace,
      }));
    }
  }
  async function refresh() {
    const ticket = ++sequence.current,
      scope = key;
    setState((old) => ({ ...old, loading: true, error: "" }));
    try {
      const ready = onRefresh ? await onRefresh() : true;
      if (live.current !== scope || sequence.current !== ticket) return;
      if (ready === false)
        throw new Error("Shipment could not be refreshed. Retry history.");
      await fetchPage(null, true);
    } catch (error) {
      if (live.current !== scope || sequence.current !== ticket) return;
      setState((old) => ({
        ...old,
        loading: false,
        error: error.message,
        retryCursor: null,
        retryReplace: true,
        retryRefresh: true,
      }));
    }
  }
  useEffect(() => {
    live.current = key;
    fetchPage(null, true);
    return () => {
      sequence.current++;
      live.current = null;
    };
    // Owner/filter and the guarded request function define this read lifetime.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, read]);
  return (
    <section className="record-history" aria-label={heading}>
      <div className="panel-heading">
        <div>
          <Heading>{heading}</Heading>
          <p className="section-note">
            Newest retained records first. Refresh to include later changes.
          </p>
        </div>
        <button
          className="secondary"
          disabled={current.loading}
          onClick={refresh}
        >
          Refresh history
        </button>
      </div>
      {current.total != null && (
        <p className="section-note">
          {current.items.length} of {current.total} records in this view
        </p>
      )}
      {current.newer && (
        <p role="status">
          New records are available. Refresh history to include them.
        </p>
      )}
      {current.error && (
        <div role="alert" className="form-error">
          {current.error}{" "}
          {(!current.changed || current.retryRefresh) && (
            <button
              className="text-link"
              disabled={current.loading}
              onClick={() =>
                current.retryRefresh
                  ? refresh()
                  : fetchPage(current.retryCursor, current.retryReplace)
              }
            >
              Retry history
            </button>
          )}
        </div>
      )}
      {current.loading && <p role="status">Loading history…</p>}
      {!current.loading && !current.error && current.items.length === 0 && (
        <p>No records in this view.</p>
      )}
      <div className={kind === "events" ? "timeline" : "stop-history"}>
        {current.items.map(({ position, record: v }) =>
          kind === "events" ? (
            <article className="timeline-item" key={position}>
              <span className="timeline-dot" />
              <div>
                <strong>{v.message}</strong>
                <small>
                  {title(v.kind)} · {time(v.occurred_at)} · {v.actor}
                </small>
                <OriginalRecord record={v} />
              </div>
            </article>
          ) : (
            <article key={position}>
              <strong>
                {kind === "appointments" ? "Appointment" : "Gate"} ·{" "}
                {title(v.stop)} · {title(v.kind || v.status)}
              </strong>
              <p>
                {kind === "appointments"
                  ? `${time(v.starts_at, v.timezone)} — ${time(v.ends_at, v.timezone)}`
                  : time(v.occurred_at, v.timezone)}{" "}
                · {v.timezone}
              </p>
              <p>
                {v.confirmation_reference || v.reference} · {v.note}
              </p>
              {v.contact && <p>Facility contact: {v.contact}</p>}
              <small>
                Recorded by {v.recorded_by} · {time(v.recorded_at)}
              </small>
              {v.supersedes_id && <small>Corrects {v.supersedes_id}</small>}
              {v.replaces_id && <small>Replaces {v.replaces_id}</small>}
              {v.cancellation && (
                <p>
                  Cancelled by {v.cancellation.recorded_by}:{" "}
                  {v.cancellation.reason}
                </p>
              )}
            </article>
          ),
        )}
      </div>
      {current.next && (
        <button
          className="secondary"
          disabled={current.loading || current.changed}
          onClick={() => fetchPage(current.next)}
        >
          Load older records
        </button>
      )}
    </section>
  );
}
