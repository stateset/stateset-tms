import { useRef, useState } from "react";
import { Modal } from "./Modal";
import { useFreightIdentity } from "./FreightIdentity";
import { IntermediateStopsEditor } from "./IntermediateStopsEditor";
import { ContractSourcing } from "./ContractSourcing";
import {
  currencyCode,
  formatMoney,
  moneyInput,
  parseMoney,
} from "../lib/money.mjs";

const date = () => new Date().toISOString().slice(0, 10);
const utc = (value) =>
  new Intl.DateTimeFormat("en-US", {
    timeZone: "UTC",
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value)) + " UTC";
const planOf = (load) =>
  Object.fromEntries(
    [
      "customer_id",
      "mode",
      "equipment",
      "commodity",
      "weight_lbs",
      "miles",
      "intermediate_stops",
      "origin",
      "destination",
      "currency",
    ].map((key) => [key, load[key]]),
  );
const expiry = (hours = 24) =>
  new Date(Date.now() + hours * 3600000).toISOString().slice(0, 16);
function Field({ label, children, ...props }) {
  return (
    <label className="field">
      <span>{label}</span>
      {children || <input required aria-label={label} {...props} />}
    </label>
  );
}
export function QuoteWorkspace({
  workspace,
  onChanged,
  openLoad,
  initialQuoteId = null,
}) {
  const { fetch, href } = useFreightIdentity();
  const [selected, setSelected] = useState(initialQuoteId),
    [form, setForm] = useState(null),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  const [sourceChoice, setSourceChoice] = useState(null);
  const pending = useRef(null);
  const quotes = workspace.quotes || [],
    quote = quotes.find((v) => v.id === selected),
    revision = quote?.revisions.at(-1),
    // Every amount on a quote, including carrier offers, is in its plan currency.
    quoteCurrency = currencyCode(revision?.plan?.currency),
    quoteMoney = (v) => formatMoney(v, quoteCurrency),
    currentOffer =
      quote?.procurements?.at(-1)?.offer ||
      revision?.offers.find((o) => o.id === revision.selected_offer_id),
    canWrite = workspace.access.capabilities.includes("operations.write");
  const open = (kind, q = quote, offer = null) => {
    setError("");
    const source = q?.revisions.at(-1)?.source_load;
    const liveLoad = workspace.loads.find(
      (l) => l.id === (source?.id || q?.booked_load_id),
    );
    setSourceChoice(
      kind === "amend" || q?.revisions.at(-1)?.amendment_of
        ? null
        : source
          ? liveLoad || source
          : null,
    );
    setForm({
      kind,
      quote: q,
      offer,
      version: q?.version,
      parentVersion: workspace.quotes.find(
        (v) => v.id === q?.revisions.at(-1)?.amendment_of?.quote_id,
      )?.version,
      sourceCheck: workspace.source_checks?.find(
        (v) => v.load_id === liveLoad?.id,
      ),
      sourceVersion: source
        ? workspace.loads.find((l) => l.id === source.id)?.version
        : null,
      loadVersion: workspace.loads.find((v) => v.id === q?.booked_load_id)
        ?.version,
    });
  };
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const v = Object.fromEntries(new FormData(event.currentTarget));
      let path = `quotes/${form.quote?.id}`,
        body = { expected_version: form.version };
      if (["create", "revise", "amend"].includes(form.kind)) {
        const currency = currencyLocked
          ? currencyCode(plan?.currency)
          : currencyCode(v.currency);
        const stop = (prefix) => ({
          ...(prefix === "pickup" ? plan?.origin : plan?.destination),
          facility: v[`${prefix}_facility`],
          city: v[`${prefix}_city`],
          state: v[`${prefix}_state`],
          date: v[`${prefix}_date`],
          address_lines: (v[`${prefix}_street`] || "")
            .split("\n")
            .map((line) => line.trim())
            .filter(Boolean),
          contact: v[`${prefix}_contact`] || "",
          phone: v[`${prefix}_phone`] || "",
          instructions: v[`${prefix}_instructions`] || "",
          postal_code: v[`${prefix}_postal`],
          country: v[`${prefix}_country`],
        });
        body = {
          ...body,
          plan: {
            customer_id: v.customer_id || plan?.customer_id,
            mode: v.mode,
            equipment: v.equipment,
            commodity: v.commodity,
            weight_lbs: Number(v.weight_lbs),
            miles: Number(v.miles),
            intermediate_stops: JSON.parse(v.intermediate_stops || "[]"),
            origin: stop("pickup"),
            destination: stop("delivery"),
            currency,
          },
          sell_cents: parseMoney(v.sell, currency),
          valid_until: new Date(v.valid_until + "Z").toISOString(),
          terms: v.terms,
        };
        if (form.kind === "create") {
          path = "quotes";
          delete body.expected_version;
          body.reference = v.reference;
        } else if (form.kind === "amend") {
          path += "/amendments";
          body.reference = v.reference;
          body.reason = v.reason;
          body.expected_load_version = form.loadVersion;
        } else {
          path += "/revise";
          body.reason = v.reason;
          if (data?.amendment_of) {
            body.expected_parent_version = form.parentVersion;
            body.expected_load_version = form.sourceVersion;
          }
        }
      } else if (["offer", "reprocure"].includes(form.kind)) {
        path += form.kind === "offer" ? "/offers" : "/reprocure";
        body = {
          ...body,
          carrier_id: v.carrier_id,
          amount_cents: parseMoney(v.amount, data.plan.currency),
          valid_until: new Date(v.valid_until + "Z").toISOString(),
          contact: v.contact,
          reference: v.reference,
          note: v.note,
        };
        if (form.kind === "reprocure") {
          body.expected_load_version = form.loadVersion;
          body.reason = v.reason;
        }
      } else if (form.kind === "withdraw-offer") {
        path += "/offers/withdraw";
        body = {
          ...body,
          offer_id: form.offer.id,
          contact: v.contact,
          reference: v.reference,
          note: v.note,
        };
      } else if (form.kind === "issue") {
        path += "/issue";
        body.offer_id = v.offer_id;
      } else if (form.kind === "publish") {
        path += "/portal-publications";
        const previous = workspace.customer_quote_publications
          ?.filter((p) => p.quote_id === form.quote.id)
          .at(-1);
        if (previous) body.expected_previous_id = previous.id;
      } else if (form.kind === "book") {
        path += "/book";
      } else if (form.kind === "reconfirm") {
        path += "/reconfirm";
        body = {
          ...body,
          expected_load_version: form.loadVersion,
          valid_until: new Date(v.valid_until + "Z").toISOString(),
          contact: v.contact,
          reference: v.reference,
          note: v.note,
        };
      } else {
        path += "/decision";
        body = {
          ...body,
          decision: form.kind,
          contact: v.contact,
          reference: v.reference,
          note: v.note,
        };
      }
      if (["create", "revise", "amend"].includes(form.kind) && sourceChoice) {
        body.plan = planOf(sourceChoice);
        body.expected_load_version = sourceChoice.version;
        if (form.kind === "create") body.planned_load_id = sourceChoice.id;
      }
      if (form.kind === "book" && form.quote.revisions.at(-1).source_load)
        body.expected_load_version = form.sourceVersion;
      if (
        form.kind === "accept" &&
        data?.amendment_of &&
        data.source_load?.sync_order_id
      ) {
        body.source_digest = form.sourceCheck?.digest || "";
        body.authorize_transport = v.authorize_transport === "on";
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
        throw new Error(result.error?.message || "Quote update failed");
      await onChanged();
      setSelected(form.kind === "publish" ? form.quote.id : result.data.id);
      pending.current = null;
      setForm(null);
    } catch (problem) {
      setError(problem.message);
      if (problem.message.includes("Sync source")) await onChanged();
    } finally {
      setBusy(false);
    }
  }
  const data = form?.quote?.revisions.at(-1),
    plan = sourceChoice || data?.plan,
    isAmendment = form?.kind === "amend" || !!data?.amendment_of,
    // Source shipments and booked amendments fix the currency server-side.
    currencyLocked = !!sourceChoice || isAmendment,
    formCurrency = currencyCode(plan?.currency);
  return (
    <>
      <section className="panel" aria-label="Quote workspace">
        <div className="panel-heading">
          <div>
            <h2>Customer quotes and carrier offers</h2>
            <p>
              Retain actual rate evidence, confirm the customer agreement, then
              book one planned shipment.
            </p>
          </div>
          {canWrite && (
            <button className="primary" onClick={() => open("create", null)}>
              Create quote
            </button>
          )}
        </div>
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>Quote / customer</th>
                <th>Lane</th>
                <th>Sell / validity</th>
                <th>Status</th>
                <th>Action</th>
              </tr>
            </thead>
            <tbody>
              {quotes.map((q) => {
                const r = q.revisions.at(-1);
                return (
                  <tr key={q.id}>
                    <td>
                      <strong>{q.reference}</strong>
                      <small>
                        {r.customer.name} · revision {r.revision}
                      </small>
                    </td>
                    <td>
                      {r.plan.origin.city}, {r.plan.origin.state} →{" "}
                      {r.plan.destination.city}, {r.plan.destination.state}
                    </td>
                    <td>
                      {formatMoney(r.sell_cents, r.plan.currency)}
                      <small>{utc(r.valid_until)}</small>
                    </td>
                    <td>
                      <span className={`badge ${q.effective_status}`}>
                        {q.effective_status}
                      </span>
                    </td>
                    <td>
                      <button
                        className="secondary"
                        onClick={() => setSelected(q.id)}
                      >
                        Open quote {q.reference}
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {!quotes.length && (
            <p className="empty">
              No quotes recorded. Create a lane and customer price, then record
              the carrier's actual offer.
            </p>
          )}
        </div>
      </section>
      {quote && (
        <section
          className="panel quote-detail"
          aria-label={`Quote details ${quote.reference}`}
        >
          <div className="panel-heading">
            <div>
              <h2>{quote.reference}</h2>
              <p>
                Revision {revision.revision} · {quote.effective_status} ·{" "}
                {quoteCurrency} inclusive shipment rates
              </p>
            </div>
            <a
              className="text-link"
              href={href(`/api/freight/quotes/${quote.id}/statement`)}
              download
            >
              Download customer quote
            </a>
          </div>
          <div className="commercial-totals">
            <div>
              <span>Customer quote</span>
              <strong>{quoteMoney(revision.sell_cents)}</strong>
              <small>
                Acceptance and booking deadline: {utc(revision.valid_until)}
              </small>
            </div>
            <div>
              <span>Current procured carrier cost</span>
              <strong>
                {currentOffer
                  ? quoteMoney(currentOffer.amount_cents)
                  : "Select an actual offer"}
              </strong>
              <small>Carrier acceptance follows tendering.</small>
            </div>
          </div>
          <p className="section-note">{revision.terms}</p>
          {workspace.customer_quote_publications?.some(
            (p) => p.quote_id === quote.id && p.revision === revision.revision,
          ) && (
            <p className="section-note">
              This revision is published to the customer's portal. Its exact
              document and native decisions remain in the partner history.
            </p>
          )}
          {revision.source_load && (
            <p className="section-note">
              Existing shipment {revision.source_load.reference}
              {revision.source_load.order_number
                ? ` · Sync order ${revision.source_load.order_number}`
                : ""}
              . Booking retains this shipment and its source order snapshot.
              {openLoad && (
                <button
                  className="text-link"
                  onClick={() => openLoad(revision.source_load.id)}
                >
                  Open source shipment
                </button>
              )}
            </p>
          )}
          {revision.amendment_of && (
            <div className="amendment-context">
              <strong>
                Customer amendment of {revision.amendment_of.quote_reference}
              </strong>
              <p>
                {quote.superseded_by
                  ? "This amendment is historical; follow the replacement agreement for current terms."
                  : quote.status === "booked"
                    ? "This amendment is the active customer agreement. The original plan, prices, and evidence remain in the preceding agreement."
                    : "The current shipment agreement stays effective until this amendment is activated after safe carrier coordination."}
              </p>
              <button
                className="text-link"
                onClick={() => setSelected(revision.amendment_of.quote_id)}
              >
                View original agreement
              </button>
            </div>
          )}
          {quote.superseded_by && (
            <div className="amendment-context">
              <strong>Replaced by {quote.superseded_by.quote_reference}</strong>
              <p>
                Original plan, prices, customer decisions, and carrier evidence
                are retained.
              </p>
              <button
                className="text-link"
                onClick={() => setSelected(quote.superseded_by.quote_id)}
              >
                View replacement agreement
              </button>
            </div>
          )}
          {quotes.some(
            (q) => q.revisions.at(-1)?.amendment_of?.quote_id === quote.id,
          ) && (
            <div className="amendment-links">
              {quotes
                .filter(
                  (q) =>
                    q.revisions.at(-1)?.amendment_of?.quote_id === quote.id,
                )
                .map((q) => (
                  <button
                    key={q.id}
                    className="text-link"
                    onClick={() => setSelected(q.id)}
                  >
                    View amendment {q.reference} · {q.effective_status}
                  </button>
                ))}
            </div>
          )}
          {revision.source_consent && (
            <details className="amendment-context">
              <summary>
                Commerce source authorized with this customer acceptance
              </summary>
              <p>
                {revision.source_consent.authorize_transport
                  ? "Customer explicitly authorized transport despite source status."
                  : "Customer acceptance is tied to this canonical source snapshot."}
              </p>
              <pre className="amendment-source-evidence">
                {JSON.stringify(revision.source_consent.snapshot, null, 2)}
              </pre>
            </details>
          )}
          <div className="charge-actions">
            {canWrite && (
              <>
                {["draft", "issued", "declined", "withdrawn"].includes(
                  quote.status,
                ) && (
                  <button className="secondary" onClick={() => open("revise")}>
                    Revise quote
                  </button>
                )}
                {quote.status === "draft" && (
                  <>
                    <button className="secondary" onClick={() => open("offer")}>
                      Record carrier offer
                    </button>
                    <button
                      className="primary"
                      disabled={!revision.offers.some((o) => !o.withdrawn)}
                      onClick={() => open("issue")}
                    >
                      Issue customer quote
                    </button>
                  </>
                )}
                {quote.status === "issued" && (
                  <>
                    {!workspace.customer_quote_publications?.some(
                      (p) =>
                        p.quote_id === quote.id &&
                        p.revision === revision.revision,
                    ) && (
                      <button
                        className="secondary"
                        onClick={() => open("publish")}
                      >
                        Publish to customer portal
                      </button>
                    )}
                    <button className="primary" onClick={() => open("accept")}>
                      Record customer acceptance
                    </button>
                    <button
                      className="secondary"
                      onClick={() => open("decline")}
                    >
                      Record customer decline
                    </button>
                  </>
                )}
                {["issued", "accepted", "declined"].includes(quote.status) && (
                  <button
                    className="secondary"
                    onClick={() => open("withdraw")}
                  >
                    Record customer withdrawal
                  </button>
                )}
                {quote.status === "accepted" && (
                  <button className="primary" onClick={() => open("book")}>
                    {revision.amendment_of
                      ? "Activate customer amendment"
                      : "Book quoted shipment"}
                  </button>
                )}
                {quote.status === "booked" &&
                  !quote.superseded_by &&
                  !["in_transit", "delivered", "cancelled"].includes(
                    workspace.loads.find((l) => l.id === quote.booked_load_id)
                      ?.status,
                  ) && (
                    <button className="secondary" onClick={() => open("amend")}>
                      Propose customer amendment
                    </button>
                  )}
                {quote.status === "booked" &&
                  workspace.loads.find((l) => l.id === quote.booked_load_id)
                    ?.status === "planned" && (
                    <>
                      <button
                        className="secondary"
                        onClick={() => open("reconfirm")}
                      >
                        Reconfirm carrier rate
                      </button>
                      <button
                        className="secondary"
                        onClick={() => open("reprocure")}
                      >
                        Record replacement procurement
                      </button>
                    </>
                  )}
              </>
            )}
            {quote.booked_load_id && (
              <button
                className="secondary"
                onClick={() => openLoad(quote.booked_load_id)}
              >
                Open booked shipment
              </button>
            )}
          </div>
          <ContractSourcing
            key={quote.id}
            quote={quote}
            workspace={workspace}
            onChanged={onChanged}
          />
          <h3 className="detail-history">Carrier procurement evidence</h3>
          <div className="charge-list">
            {revision.offers.map((offer) => (
              <article key={offer.id}>
                <div className="charge-heading">
                  <strong>
                    {offer.carrier.name} · {quoteMoney(offer.amount_cents)}
                  </strong>
                  <span
                    className={`badge ${offer.withdrawn ? "reversed" : "proposed"}`}
                  >
                    {offer.withdrawn
                      ? "withdrawn"
                      : offer.id === revision.selected_offer_id
                        ? "quote selection"
                        : "offer recorded"}
                  </span>
                </div>
                <p>
                  {offer.evidence.contact} · {offer.evidence.reference}
                  <br />
                  {offer.evidence.note}
                </p>
                <small>
                  Valid until {utc(offer.valid_until)} · recorded by{" "}
                  {offer.evidence.recorded_by}
                </small>
                {offer.withdrawn && (
                  <p>
                    Withdrawn: {offer.withdrawn.contact} ·{" "}
                    {offer.withdrawn.reference} · {offer.withdrawn.note}
                  </p>
                )}
                {offer.rate_source && (
                  <p>
                    Source: carrier agreement revision{" "}
                    {offer.rate_source.revision} ·{" "}
                    <a
                      className="text-link"
                      href={href(
                        `/api/freight/carrier-contracts/${offer.rate_source.agreement_id}/statement?revision=${offer.rate_source.revision}`,
                      )}
                    >
                      Original agreed rate
                    </a>
                  </p>
                )}
                {canWrite && !quote.booked_load_id && !offer.withdrawn && (
                  <button
                    className="text-link"
                    onClick={() => open("withdraw-offer", quote, offer)}
                  >
                    Record offer withdrawal
                  </button>
                )}
              </article>
            ))}
          </div>
          {(quote.procurements || []).map((award, i) => (
            <div className="charge-decision" key={award.offer.id}>
              <strong>
                Procurement {i + 1}: {award.offer.carrier.name} ·{" "}
                {quoteMoney(award.offer.amount_cents)}
              </strong>
              {award.offer.rate_source && (
                <p>
                  Source: carrier agreement revision{" "}
                  {award.offer.rate_source.revision} ·{" "}
                  <a
                    className="text-link"
                    href={href(
                      `/api/freight/carrier-contracts/${award.offer.rate_source.agreement_id}/statement?revision=${award.offer.rate_source.revision}`,
                    )}
                  >
                    Original procurement rate
                  </a>
                </p>
              )}
              <p>{award.reason}</p>
              <small>
                {award.offer.evidence.contact} ·{" "}
                {award.offer.evidence.reference}
                <br />
                {award.recorded_by} · {award.recorded_at}
              </small>
            </div>
          ))}
          {revision.decision_history.map(([kind, e], i) => (
            <p className="section-note" key={i}>
              Customer {kind} · {e.contact} · {e.reference}
              <br />
              {e.note} · {e.recorded_by} · {e.recorded_at}
            </p>
          ))}
          {quote.reconfirmations.map((r, i) => (
            <p className="section-note" key={i}>
              Same-rate carrier reconfirmation · {r.evidence.contact} ·{" "}
              {r.evidence.reference}
              <br />
              Valid until {utc(r.valid_until)} · {r.evidence.note}
            </p>
          ))}
          {quote.revisions.length > 1 && (
            <details>
              <summary>Prior quote revisions</summary>
              {quote.revisions.slice(0, -1).map((r) => (
                <p key={r.revision}>
                  Revision {r.revision} ·{" "}
                  {formatMoney(r.sell_cents, r.plan.currency)} · {r.reason}
                  <br />
                  <a
                    className="text-link"
                    href={href(
                      `/api/freight/quotes/${quote.id}/statement?revision=${r.revision}`,
                    )}
                    download
                  >
                    Download quote revision {r.revision}
                  </a>
                  {r.offers.map((offer) => (
                    <span key={offer.id}>
                      <br />
                      Carrier evidence: {offer.carrier.name} ·{" "}
                      {formatMoney(offer.amount_cents, r.plan.currency)} ·{" "}
                      {offer.evidence.contact} · {offer.evidence.reference} ·{" "}
                      {offer.withdrawn ? "withdrawn" : "recorded"}
                    </span>
                  ))}
                  {r.source_consent && (
                    <details>
                      <summary>
                        Accepted commerce source for revision {r.revision}
                      </summary>
                      <pre className="amendment-source-evidence">
                        {JSON.stringify(r.source_consent.snapshot, null, 2)}
                      </pre>
                    </details>
                  )}
                  {r.decision_history.map(([kind, proof], i) => (
                    <span key={i}>
                      <br />
                      Customer {kind}: {proof.contact} · {proof.reference} ·{" "}
                      {proof.recorded_by}
                    </span>
                  ))}
                </p>
              ))}
            </details>
          )}
        </section>
      )}
      {form && (
        <Modal
          title={
            {
              create: "Create quote",
              amend: "Propose customer amendment",
              revise: "Revise quote",
              offer: "Record carrier offer",
              issue: "Issue customer quote",
              publish: "Publish customer quote",
              accept: "Record customer acceptance",
              decline: "Record customer decline",
              withdraw: "Record customer withdrawal",
              book: data?.amendment_of
                ? "Activate customer amendment"
                : "Book quoted shipment",
              reconfirm: "Reconfirm carrier rate",
              reprocure: "Record replacement procurement",
              "withdraw-offer": "Record offer withdrawal",
            }[form.kind]
          }
          close={() => !busy && setForm(null)}
          wide={["create", "revise", "amend"].includes(form.kind)}
        >
          <form className="quote-form" onSubmit={submit}>
            {["create", "revise", "amend"].includes(form.kind) ? (
              <div className="form-grid" key={sourceChoice?.id || "standalone"}>
                {form.kind === "create" && (
                  <Field label="Quote source shipment">
                    <select
                      aria-label="Quote source shipment"
                      value={sourceChoice?.id || ""}
                      onChange={(e) =>
                        setSourceChoice(
                          workspace.loads.find(
                            (l) => l.id === e.target.value,
                          ) || null,
                        )
                      }
                    >
                      <option value="">
                        Create a new shipment after customer acceptance
                      </option>
                      {workspace.loads
                        .filter(
                          (l) =>
                            l.status === "planned" &&
                            !l.carrier_id &&
                            !l.quote_binding,
                        )
                        .map((l) => (
                          <option key={l.id} value={l.id}>
                            {l.reference}
                            {l.order_number
                              ? ` · Sync order ${l.order_number}`
                              : ""}
                          </option>
                        ))}
                    </select>
                  </Field>
                )}

                {["create", "amend"].includes(form.kind) && (
                  <Field
                    label="Quote reference"
                    name="reference"
                    maxLength={100}
                  />
                )}
                {form.kind !== "create" && (
                  <Field
                    label="Quote revision reason"
                    name="reason"
                    maxLength={200}
                  />
                )}
                <fieldset
                  className="quote-plan-fields"
                  disabled={!!sourceChoice}
                >
                  <legend>
                    {sourceChoice
                      ? `Existing shipment ${sourceChoice.reference} · plan retained`
                      : "Quoted shipment plan"}
                  </legend>
                  <Field label="Quoted customer">
                    <select
                      disabled={isAmendment}
                      aria-label="Quoted customer"
                      name="customer_id"
                      defaultValue={plan?.customer_id}
                      required
                    >
                      {workspace.customers.map((c) => (
                        <option key={c.id} value={c.id}>
                          {c.name}
                        </option>
                      ))}
                    </select>
                  </Field>
                  <Field label="Quoted mode">
                    <select
                      aria-label="Quoted mode"
                      name="mode"
                      defaultValue={plan?.mode || "FTL"}
                    >
                      <option>FTL</option>
                      <option>LTL</option>
                    </select>
                  </Field>
                  <Field label="Quoted equipment">
                    <select
                      aria-label="Quoted equipment"
                      name="equipment"
                      defaultValue={plan?.equipment || "Dry van"}
                    >
                      {["Dry van", "Reefer", "Flatbed"].map((e) => (
                        <option key={e}>{e}</option>
                      ))}
                    </select>
                  </Field>
                  <Field
                    label="Quoted commodity"
                    name="commodity"
                    defaultValue={plan?.commodity || ""}
                    maxLength={200}
                  />
                  <Field
                    label="Quoted weight (lb)"
                    name="weight_lbs"
                    type="number"
                    min="1"
                    max="80000"
                    defaultValue={plan?.weight_lbs || 1000}
                  />
                  <Field
                    label="Quoted distance (miles)"
                    name="miles"
                    type="number"
                    min="1"
                    max="10000"
                    defaultValue={plan?.miles || 100}
                  />
                  {["pickup", "delivery"].map((prefix) => {
                    const s =
                      prefix === "pickup" ? plan?.origin : plan?.destination;
                    const label =
                      prefix === "pickup" ? "Quote pickup" : "Quote delivery";
                    return (
                      <fieldset key={prefix} className="quote-stop">
                        <legend>{label}</legend>
                        <Field
                          label={`${label} facility`}
                          name={`${prefix}_facility`}
                          defaultValue={s?.facility || ""}
                        />
                        <Field label={`${label} street`}>
                          <textarea
                            aria-label={`${label} street`}
                            name={`${prefix}_street`}
                            required
                            maxLength={111}
                            rows={2}
                            defaultValue={s?.address_lines?.join("\n") || ""}
                          />
                        </Field>
                        <Field
                          label={`${label} city`}
                          name={`${prefix}_city`}
                          defaultValue={s?.city || ""}
                        />
                        <Field
                          label={`${label} state`}
                          name={`${prefix}_state`}
                          defaultValue={s?.state || ""}
                        />
                        <Field
                          label={`${label} postal code`}
                          name={`${prefix}_postal`}
                          defaultValue={s?.postal_code || ""}
                        />
                        <Field
                          label={`${label} country`}
                          name={`${prefix}_country`}
                          defaultValue={s?.country || "US"}
                        />
                        <Field
                          label={`${label} contact`}
                          name={`${prefix}_contact`}
                          required={false}
                          maxLength={200}
                          defaultValue={s?.contact || ""}
                        />
                        <Field
                          label={`${label} phone`}
                          name={`${prefix}_phone`}
                          required={false}
                          maxLength={100}
                          defaultValue={s?.phone || ""}
                        />
                        <Field label={`${label} instructions`}>
                          <textarea
                            aria-label={`${label} instructions`}
                            name={`${prefix}_instructions`}
                            maxLength={1000}
                            defaultValue={s?.instructions || ""}
                          />
                        </Field>
                        <Field
                          label={`${label} date`}
                          name={`${prefix}_date`}
                          type="date"
                          defaultValue={s?.date || date()}
                        />
                      </fieldset>
                    );
                  })}
                </fieldset>
                <Field
                  label="Quote currency"
                  name="currency"
                  maxLength={3}
                  pattern="[A-Za-z]{3}"
                  readOnly={currencyLocked}
                  defaultValue={formCurrency}
                />
                <Field
                  label="Customer quote price"
                  name="sell"
                  inputMode="decimal"
                  defaultValue={
                    data ? moneyInput(data.sell_cents, data.plan.currency) : ""
                  }
                />
                {!sourceChoice && (
                  <IntermediateStopsEditor
                    key={form.quote?.id || "new-route"}
                    initial={plan?.intermediate_stops || []}
                  />
                )}
                <Field
                  label="Customer quote deadline (UTC)"
                  name="valid_until"
                  type="datetime-local"
                  defaultValue={data?.valid_until?.slice(0, 16) || expiry()}
                />
                <Field
                  label="Inclusive quote terms"
                  name="terms"
                  maxLength={2000}
                  defaultValue={
                    data?.terms ||
                    "Inclusive rate for the stated shipment. Acceptance and booking must occur before the quote deadline."
                  }
                />
              </div>
            ) : (
              <div className="form-grid">
                {form.kind === "publish" && (
                  <div className="section-note">
                    <p>
                      Publish revision {data.revision} at{" "}
                      {formatMoney(data.sell_cents, data.plan.currency)}{" "}
                      inclusive {currencyCode(data.plan.currency)} to{" "}
                      {data.customer.name}'s configured customer portal. The
                      exact document remains in their history. Publication does
                      not accept or book transport.
                    </p>
                    <label className="partner-ack">
                      <input type="checkbox" required />I reviewed this
                      customer's exact issued quote and am ready to publish it.
                    </label>
                  </div>
                )}
                {["offer", "reprocure"].includes(form.kind) && (
                  <>
                    <Field label="Quoted carrier">
                      <select
                        aria-label="Quoted carrier"
                        name="carrier_id"
                        required
                      >
                        <option value="">Select carrier</option>
                        {workspace.carriers
                          .filter((c) => c.equipment === data.plan.equipment)
                          .map((c) => (
                            <option key={c.id} value={c.id}>
                              {c.name} · {c.scac}
                            </option>
                          ))}
                      </select>
                    </Field>
                    <Field
                      label={`Actual carrier offer (${currencyCode(data.plan.currency)})`}
                      name="amount"
                      inputMode="decimal"
                    />
                  </>
                )}
                {form.kind === "reprocure" && (
                  <Field
                    label="Replacement procurement reason"
                    name="reason"
                    maxLength={200}
                  />
                )}
                {["offer", "reconfirm", "reprocure"].includes(form.kind) && (
                  <Field
                    label="Carrier offer deadline (UTC)"
                    name="valid_until"
                    type="datetime-local"
                    defaultValue={expiry(48)}
                  />
                )}
                {[
                  "offer",
                  "accept",
                  "decline",
                  "withdraw",
                  "withdraw-offer",
                  "reconfirm",
                  "reprocure",
                ].includes(form.kind) && (
                  <>
                    <Field
                      label="Quote counterparty contact"
                      name="contact"
                      maxLength={200}
                    />
                    <Field
                      label="Quote confirmation reference"
                      name="reference"
                      maxLength={200}
                    />
                    <Field
                      label="Quote evidence note"
                      name="note"
                      maxLength={200}
                    />
                  </>
                )}
                {form.kind === "accept" &&
                  data?.amendment_of &&
                  data.source_load?.sync_order_id && (
                    <div className="amendment-source-consent">
                      <h3>Customer consent to commerce source</h3>
                      <p>
                        Acceptance confirms the proposed freight plan against
                        this verified order:{" "}
                        {form.sourceCheck?.snapshot?.orderNumber ||
                          data.source_load.order_number}{" "}
                        · status{" "}
                        {form.sourceCheck?.snapshot?.status || "not checked"}. A
                        later business change requires new acceptance.
                      </p>
                      <details>
                        <summary>Order snapshot being authorized</summary>
                        <pre>
                          {JSON.stringify(
                            form.sourceCheck?.snapshot || {},
                            null,
                            2,
                          )}
                        </pre>
                      </details>
                      <label className="check-field">
                        <input
                          type="checkbox"
                          name="authorize_transport"
                          required={
                            ![
                              "pending",
                              "processing",
                              "shipped",
                              "fulfilled",
                              "delivered",
                            ].includes(
                              String(
                                form.sourceCheck?.snapshot?.status,
                              ).toLowerCase(),
                            )
                          }
                        />
                        Customer explicitly authorizes this amended transport
                        despite the commerce order status
                      </label>
                    </div>
                  )}
                {form.kind === "issue" && (
                  <Field label="Selected carrier offer">
                    <select
                      aria-label="Selected carrier offer"
                      name="offer_id"
                      required
                    >
                      <option value="">Select an actual carrier offer</option>
                      {data.offers
                        .filter((o) => !o.withdrawn)
                        .map((o) => (
                          <option key={o.id} value={o.id}>
                            {o.carrier.name} ·{" "}
                            {formatMoney(o.amount_cents, data.plan.currency)} ·{" "}
                            {o.evidence.reference}
                          </option>
                        ))}
                    </select>
                  </Field>
                )}
                {form.kind === "book" && (
                  <p className="section-note">
                    {data.amendment_of
                      ? `Activate this customer-approved amendment on existing shipment ${data.source_load.reference}. Release or withdraw the current carrier first; reconcile charge agreements, documents, and affected appointments. The old agreement remains historical.`
                      : data.source_load
                        ? `Book the accepted rates on existing shipment ${data.source_load.reference}. Its original order identity, snapshot, and tracking connection remain.`
                        : "Create one planned shipment from this accepted revision and selected carrier cost."}{" "}
                    Tender the shipment separately to obtain carrier acceptance.
                    Booking records no invoice or financial journal.
                  </p>
                )}
                {form.kind === "reconfirm" && (
                  <p className="section-note">
                    Record an actual extension at the same carrier, plan, and
                    price. The original offer and customer acceptance remain. A
                    different carrier cost uses replacement procurement;
                    customer plan or sell changes require customer coordination.
                  </p>
                )}
              </div>
            )}
            {error && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            <div className="form-actions">
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => setForm(null)}
              >
                Cancel
              </button>
              <button type="submit" className="primary" disabled={busy}>
                {busy ? "Saving…" : "Save quote action"}
              </button>
            </div>
          </form>
        </Modal>
      )}
    </>
  );
}
