const phases = ["arrival", "service", "departure"];
const sourceName = {
  gate: "Gate observation",
  stop_service: "Stop confirmation",
  carrier: "Carrier report",
};
const phase = (f) =>
  ["pickup", "delivery"].includes(f.kind) ? "service" : f.kind;
const instant = (value) =>
  new Date(value).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
export function ExecutionTimeline({ timeline }) {
  if (!timeline?.record_count) return null;
  const label = (f) => {
    const stop = timeline.stops.find((s) => s.id === f.stop_id);
    return `Stop ${stop?.sequence || "?"} ${f.kind}`;
  };
  return (
    <section
      className="execution-timeline"
      aria-label="Shipment execution timeline"
    >
      <details open={!timeline.consistent || undefined}>
        <summary>
          Execution timeline · {timeline.record_count} recorded observations
        </summary>
        <p className="section-note">
          Actual gate, service, and carrier observations in route order. Missing
          times remain unconfirmed.
        </p>
        {!timeline.consistent && (
          <div className="form-error" role="alert">
            <strong>
              {timeline.evidence_valid === false
                ? "Movement correction evidence needs review"
                : "Execution times need review"}
            </strong>
            <p>
              Correct the original conflicting evidence. Financial approval and
              final delivery updates remain on hold.
            </p>
            <ul>
              {timeline.conflicts.map((c) => (
                <li key={`${c.before.id}-${c.after.id}`}>
                  {label(c.before)} ({instant(c.before.observed_at)}) is after{" "}
                  {label(c.after)} ({instant(c.after.observed_at)}).
                </li>
              ))}
            </ul>
          </div>
        )}
        <ol className="execution-stop-list">
          {timeline.stops.map((stop) => (
            <li key={stop.id}>
              <strong>
                Stop {stop.sequence} · {stop.kind} · {stop.facility}
              </strong>
              <dl className="execution-phases">
                {phases.map((kind) => (
                  <div key={kind}>
                    <dt>
                      {kind === "service"
                        ? stop.kind === "pickup"
                          ? "Pickup service"
                          : "Delivery service"
                        : kind === "arrival"
                          ? "Gate arrival"
                          : "Gate departure"}
                    </dt>
                    <dd>
                      {stop.facts.filter((f) => phase(f) === kind).length ? (
                        stop.facts
                          .filter((f) => phase(f) === kind)
                          .map((f) => (
                            <div key={f.id} className="execution-fact">
                              <time
                                dateTime={f.observed_at}
                                title={f.observed_at}
                              >
                                {instant(f.observed_at)}
                              </time>
                              <small>
                                {sourceName[f.source] || "Recorded observation"}
                              </small>
                              {(f.corrected || f.correction) && (
                                <small>
                                  Corrected actual time · originally{" "}
                                  {instant(f.original_observed_at)}
                                </small>
                              )}
                              {f.reference && (
                                <small>
                                  {f.reference} · {f.recorded_by}
                                </small>
                              )}
                            </div>
                          ))
                      ) : (
                        <span className="muted">Unrecorded</span>
                      )}
                    </dd>
                  </div>
                ))}
              </dl>
            </li>
          ))}
        </ol>
      </details>
    </section>
  );
}
