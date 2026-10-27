import { useState } from "react";

export function SaveRecovery({ outcome, detail, busy, retry, resolve }) {
  const [note, setNote] = useState("");
  const uncertain = outcome.kind === "unknown";
  const stopped = outcome.kind === "stopped";
  return (
    <section className="save-recovery" aria-label="Save recovery">
      <div role={uncertain ? "alert" : "status"}>
        <h3>
          {uncertain
            ? "Save outcome not confirmed"
            : stopped
              ? "Original save stopped"
              : "Save completed"}
        </h3>
        <p>
          {uncertain
            ? "The response was interrupted. Your original save is kept. Check it before changing or submitting it again."
            : stopped
              ? "The original request was stopped before application. Refresh to see the latest records."
              : "Your save completed, but the workspace has not refreshed yet. Refresh to see the latest records."}
        </p>
        {!uncertain && outcome.message && <p>{outcome.message}</p>}
      </div>
      {outcome.body && (
        <details>
          <summary>Original save details</summary>
          <p>
            Recorded {new Date(outcome.createdAt).toLocaleString()} on this
            browser.
          </p>
          <p style={{ overflowWrap: "anywhere" }}>
            Original target: <code>{outcome.path}</code>
          </p>
          <pre
            aria-label="Original save values"
            style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}
          >
            {JSON.stringify(outcome.body, null, 2)}
          </pre>
        </details>
      )}
      {detail && <p>{detail}</p>}
      <button
        className="secondary"
        type="button"
        disabled={busy}
        onClick={retry}
      >
        {busy
          ? uncertain
            ? "Checking save…"
            : "Refreshing…"
          : uncertain
            ? "Check original save"
            : "Refresh workspace"}
      </button>
      {uncertain && resolve && (
        <details>
          <summary>Stop waiting for this save</summary>
          <p>
            Keep any completed change. Otherwise, stop this request from being
            applied.
          </p>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              resolve(note);
            }}
          >
            <label className="field">
              <span>Resolution note</span>
              <input
                value={note}
                onChange={(event) => setNote(event.target.value)}
                required
                maxLength={200}
                disabled={busy}
              />
            </label>
            <button type="submit" className="secondary" disabled={busy}>
              Resolve original save
            </button>
          </form>
        </details>
      )}
    </section>
  );
}
