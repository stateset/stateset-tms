import { useFreightIdentity } from "./FreightIdentity";
import { useRef, useState } from "react";
import { Modal } from "./Modal";
import {
  amountStep,
  formatMoney,
  moneyInput,
  parseMoney,
} from "../lib/money.mjs";

const today = () => new Date().toISOString().slice(0, 10);
function Field({ title, children, ...props }) {
  return (
    <label className="field">
      <span>{title}</span>
      {children || <input aria-label={title} required {...props} />}
    </label>
  );
}
export function CarrierCreditApplications({ workspace, onChanged, openLoad }) {
  const { fetch, href: identityHref } = useFreightIdentity();
  const [form, setForm] = useState(null),
    [selected, setSelected] = useState({}),
    [busy, setBusy] = useState(false),
    [error, setError] = useState("");
  const pending = useRef(null);
  const entries = workspace.carrier_credit_applications || [];
  const canWrite = workspace.access.capabilities.includes("finance.write");
  const load = (id) => workspace.loads.find((v) => v.id === id);
  // A settlement carries its shipment's currency; credits never cross it.
  const currencyOf = (settlement) => load(settlement?.load_id)?.currency;
  const carrier = (id) =>
    workspace.carriers.find((v) => v.id === id)?.name || id;
  const label = (target) =>
    `${load(target?.load_id)?.reference || target?.load_id} · ${workspace.invoices.find((v) => v.load_id === target?.load_id && v.status === "approved")?.number || target?.id}`;
  const snapshots = form?.settlements || [];
  const source = snapshots.find((v) => v.id === form?.sourceId);
  const sources = snapshots.filter(
    (v) =>
      v.balance_cents < 0 &&
      (!form?.entry || v.carrier_id === form.entry.carrier_id),
  );
  const targets = snapshots.filter(
    (v) =>
      v.balance_cents > 0 &&
      v.carrier_id === source?.carrier_id &&
      currencyOf(v) === currencyOf(source) &&
      v.id !== source?.id,
  );
  const available = workspace.settlements.filter(
    (v) =>
      v.balance_cents < 0 &&
      workspace.settlements.some(
        (t) =>
          t.carrier_id === v.carrier_id &&
          currencyOf(t) === currencyOf(v) &&
          t.balance_cents > 0,
      ),
  );
  const selectedTotal = Object.values(selected).reduce((sum, value) => {
    try {
      return sum + parseMoney(value, currencyOf(source));
    } catch {
      return sum;
    }
  }, 0);
  const affected =
    form?.kind === "reverse"
      ? [
          form.entry.source_settlement_id,
          ...form.entry.allocations.map((v) => v.target_id),
        ]
      : [source?.id, ...Object.keys(selected)];
  const earliest = affected.reduce((date, id) => {
    const candidate = snapshots.find(
      (v) => v.id === id,
    )?.credit_application_earliest_date;
    return candidate && candidate > date ? candidate : date;
  }, "1900-01-01");
  function open(kind, entry = null) {
    setError("");
    pending.current = null;
    const settlements = workspace.settlements.map((v) => ({ ...v }));
    const defaultSource = entry
      ? settlements.find(
          (v) => v.id === entry.source_settlement_id && v.balance_cents < 0,
        ) ||
        settlements.find(
          (v) => v.carrier_id === entry.carrier_id && v.balance_cents < 0,
        )
      : available[0];
    setSelected({});
    setForm({
      kind,
      entry,
      settlements,
      sourceId:
        kind === "reverse"
          ? entry.source_settlement_id
          : defaultSource?.id || "",
    });
  }
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const values = Object.fromEntries(new FormData(event.currentTarget));
      const proof = {
        counterparty_contact: values.counterparty_contact,
        confirmation_reference: values.confirmation_reference,
        note: values.note,
      };
      let path, body;
      if (form.kind === "reverse") {
        const original = form.entry;
        path = `carrier-credit-applications/${original.id}/reverse`;
        body = {
          ...proof,
          expected_version: original.version,
          expected_source_version: source?.version,
          reversed_on: values.reversed_on,
          targets: original.allocations.map((v) => ({
            target_id: v.target_id,
            expected_version: snapshots.find((t) => t.id === v.target_id)
              ?.version,
          })),
        };
      } else {
        if (!source)
          throw new Error("Choose an available carrier credit balance.");
        const allocations = Object.entries(selected).map(
          ([target_id, amount]) => ({
            target_id,
            amount_cents: parseMoney(amount, currencyOf(source)),
            expected_version: targets.find((v) => v.id === target_id)?.version,
          }),
        );
        const amount_cents = allocations.reduce(
          (sum, v) => sum + v.amount_cents,
          0,
        );
        if (!allocations.length || amount_cents > -source.balance_cents)
          throw new Error(
            "Choose allocations within the available carrier credit.",
          );
        for (const allocation of allocations)
          if (
            allocation.amount_cents >
            targets.find((v) => v.id === allocation.target_id)?.balance_cents
          )
            throw new Error(
              "Allocation exceeds the target's outstanding payable.",
            );
        path = "carrier-credit-applications";
        body = {
          ...proof,
          source_settlement_id: source.id,
          expected_source_version: source.version,
          amount_cents,
          applied_on: values.applied_on,
          allocations,
          evidence_reference:
            form.entry?.evidence_reference || values.evidence_reference,
          document_id: values.document_id || null,
          corrects_id: form.entry?.id || null,
        };
      }
      const fingerprint = JSON.stringify([path, body]);
      if (pending.current?.fingerprint !== fingerprint)
        pending.current = { fingerprint, key: crypto.randomUUID() };
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
          result.error?.message || "Carrier credit action failed.",
        );
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
      className="panel finance-secondary"
      aria-label="Carrier credit applications"
    >
      <div className="panel-heading">
        <div>
          <h2>Carrier credit applications</h2>
          <p>
            Record the carrier's confirmation that an existing credit covers
            other prepared invoices.
          </p>
        </div>
        <button
          className="primary"
          disabled={!canWrite || busy || !available.length}
          onClick={() => open("apply")}
        >
          Apply carrier credit
        </button>
      </div>
      <div className="credit-application-list">
        {!entries.length && (
          <p className="empty">
            No carrier credits applied to other invoices. Eligible sources have
            an available credit and another payable for that same carrier.
          </p>
        )}
        {[...entries].reverse().map((entry) => (
          <article className="credit-application-card" key={entry.id}>
            <div className="credit-application-heading">
              <div>
                <strong>{entry.evidence_reference}</strong>
                <small>
                  {carrier(entry.carrier_id)} ·{" "}
                  {formatMoney(entry.amount_cents, entry.currency)} ·{" "}
                  {entry.applied_on}
                </small>
              </div>
              <span
                className={`badge ${entry.reversal ? "reversed" : "posted"}`}
              >
                {entry.reversal ? "Reversed" : "Applied"}
              </span>
            </div>
            <p>
              From{" "}
              <button
                className="text-link"
                onClick={() => openLoad(entry.source_load_id)}
              >
                {label(
                  workspace.settlements.find(
                    (v) => v.id === entry.source_settlement_id,
                  ),
                )}
              </button>
            </p>
            {entry.allocations.map((allocation) => (
              <p key={allocation.target_id}>
                To{" "}
                <button
                  className="text-link"
                  onClick={() => openLoad(allocation.load_id)}
                >
                  {label(
                    workspace.settlements.find(
                      (v) => v.id === allocation.target_id,
                    ),
                  )}
                </button>{" "}
                · {formatMoney(allocation.amount_cents, entry.currency)}
              </p>
            ))}
            <details>
              <summary>Application evidence and history</summary>
              <p>
                {entry.confirmation.counterparty_contact} ·{" "}
                {entry.confirmation.confirmation_reference}
              </p>
              <p>{entry.confirmation.note}</p>
              <p>
                Recorded {new Date(entry.recorded_at).toLocaleString()} by{" "}
                {entry.recorded_by}
              </p>
              {entry.document_id && (
                <p>
                  <a
                    href={identityHref(
                      `/api/freight/documents/${entry.document_id}/content`,
                    )}
                  >
                    Download carrier application evidence
                  </a>{" "}
                  · SHA-256 {entry.document_sha256}
                </p>
              )}
              {entry.reversal && (
                <>
                  <p>
                    Reversed {entry.reversal.reversed_on} by{" "}
                    {entry.reversal.actor}: {entry.reversal.note}
                  </p>
                  <p>
                    {entry.reversal_confirmation?.counterparty_contact} ·{" "}
                    {entry.reversal_confirmation?.confirmation_reference}
                  </p>
                </>
              )}
              {entry.corrects_id && (
                <p>Corrects application {entry.corrects_id}</p>
              )}
              {entry.corrected_by_id && (
                <p>Corrected by {entry.corrected_by_id}</p>
              )}
              <small>Record {entry.id}</small>
            </details>
            <div className="finance-actions">
              {!entry.reversal && (
                <button
                  className="secondary"
                  disabled={!canWrite || busy}
                  onClick={() => open("reverse", entry)}
                >
                  Reverse credit application
                </button>
              )}
              {entry.reversal && !entry.corrected_by_id && (
                <button
                  className="secondary"
                  disabled={
                    !canWrite ||
                    busy ||
                    !workspace.settlements.some(
                      (v) =>
                        v.carrier_id === entry.carrier_id &&
                        v.balance_cents < 0,
                    )
                  }
                  onClick={() => open("apply", entry)}
                >
                  Correct reversed application
                </button>
              )}
            </div>
          </article>
        ))}
      </div>
      {form && (
        <Modal
          title={
            form.kind === "reverse"
              ? "Reverse carrier credit application"
              : form.entry
                ? "Correct carrier credit application"
                : "Apply carrier credit to invoices"
          }
          close={() => {
            if (!busy) setForm(null);
          }}
          wide
        >
          <form className="modal-form" onSubmit={submit}>
            {error && (
              <p className="form-error" role="alert">
                {error}
              </p>
            )}
            {form.kind !== "reverse" ? (
              <>
                <p>
                  The application reduces the credit and the selected payables
                  by the same amount. Original invoices, costs, and bank entries
                  remain retained.
                </p>
                <Field title="Carrier credit source">
                  <select
                    aria-label="Carrier credit source"
                    required
                    value={form.sourceId}
                    onChange={(e) => {
                      setForm({ ...form, sourceId: e.target.value });
                      setSelected({});
                    }}
                  >
                    <option value="" disabled>
                      Select a credit balance
                    </option>
                    {sources.map((v) => (
                      <option key={v.id} value={v.id}>
                        {carrier(v.carrier_id)} · {label(v)} ·{" "}
                        {formatMoney(-v.balance_cents, currencyOf(v))} available
                      </option>
                    ))}
                  </select>
                </Field>
                <p>
                  {formatMoney(selectedTotal, currencyOf(source))} allocated /{" "}
                  {formatMoney(
                    -(source?.balance_cents || 0),
                    currencyOf(source),
                  )}{" "}
                  available
                </p>
                <div className="credit-targets">
                  {targets.map((target) => (
                    <div className="credit-target" key={target.id}>
                      <label>
                        <input
                          type="checkbox"
                          aria-label={`Apply credit to ${load(target.load_id)?.reference}`}
                          checked={Object.hasOwn(selected, target.id)}
                          onChange={(e) => {
                            const next = { ...selected };
                            if (e.target.checked) next[target.id] = "";
                            else delete next[target.id];
                            setSelected(next);
                          }}
                        />{" "}
                        {label(target)} ·{" "}
                        {formatMoney(target.balance_cents, currencyOf(target))}{" "}
                        due
                      </label>
                      {Object.hasOwn(selected, target.id) && (
                        <Field
                          title={`Credit amount for ${load(target.load_id)?.reference}`}
                          type="number"
                          min={amountStep(currencyOf(target))}
                          max={moneyInput(
                            target.balance_cents,
                            currencyOf(target),
                          )}
                          step={amountStep(currencyOf(target))}
                          value={selected[target.id]}
                          onChange={(e) =>
                            setSelected({
                              ...selected,
                              [target.id]: e.target.value,
                            })
                          }
                        />
                      )}
                    </div>
                  ))}
                  {!targets.length && (
                    <p>
                      No outstanding prepared invoices for this carrier. Prepare
                      the applicable payable before applying a credit.
                    </p>
                  )}
                </div>
                <Field
                  title="Carrier credit application reference"
                  name="evidence_reference"
                  maxLength={200}
                  defaultValue={form.entry?.evidence_reference || ""}
                  readOnly={!!form.entry}
                />
                <Field
                  title="Application date"
                  name="applied_on"
                  type="date"
                  min={earliest}
                  max={today()}
                  defaultValue={today()}
                />
                <Field title="Stored application evidence (optional)">
                  <select
                    aria-label="Stored application evidence (optional)"
                    name="document_id"
                    key={form.sourceId}
                    defaultValue=""
                  >
                    <option value="">
                      External carrier reference recorded above
                    </option>
                    {(workspace.documents || [])
                      .filter(
                        (v) =>
                          v.load_id === source?.load_id &&
                          v.status !== "rejected",
                      )
                      .map((v) => (
                        <option key={v.id} value={v.id}>
                          {v.filename} · {v.sha256.slice(0, 12)}
                        </option>
                      ))}
                  </select>
                </Field>
              </>
            ) : (
              <>
                <p>
                  Reverse{" "}
                  {formatMoney(form.entry.amount_cents, form.entry.currency)}{" "}
                  applied under {form.entry.evidence_reference}. This can reopen
                  target payables and the source credit balance. The original
                  confirmation and postings remain in history.
                </p>
                <Field
                  title="Application reversal date"
                  name="reversed_on"
                  type="date"
                  min={earliest}
                  max={today()}
                  defaultValue={today()}
                />
              </>
            )}
            <Field
              title="Carrier application contact"
              name="counterparty_contact"
              maxLength={200}
            />
            <Field
              title="Carrier application confirmation"
              name="confirmation_reference"
              maxLength={200}
            />
            <Field
              title="Carrier application decision note"
              name="note"
              maxLength={200}
            />
            <p>
              Dates must follow the existing payable activity on every affected
              settlement. Record the actual carrier confirmation; this action
              does not initiate a payment or verify a remote carrier account.
            </p>
            <div className="modal-actions">
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => setForm(null)}
              >
                Cancel
              </button>
              <button
                className="primary"
                disabled={
                  busy ||
                  !canWrite ||
                  (form.kind !== "reverse" &&
                    (!source || !Object.keys(selected).length))
                }
              >
                {busy
                  ? "Saving…"
                  : form.kind === "reverse"
                    ? "Confirm application reversal"
                    : "Confirm carrier credit application"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </section>
  );
}
