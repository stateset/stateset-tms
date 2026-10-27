import { useEffect, useRef, useState } from "react";
import { useFreightIdentity } from "./FreightIdentity";
import { trackingIntent } from "../lib/commerce-tracking.mjs";
import { CommerceShipmentPicker } from "./CommerceShipmentPicker";

export function CommerceTracking({
  load,
  workspace,
  disabled,
  onQueue,
  onOpenHandoffs,
}) {
  const identity = useFreightIdentity();
  const [shipmentId, setShipmentId] = useState("");
  const [review, setReview] = useState(null);
  const [error, setError] = useState("");
  const [reading, setReading] = useState(false);
  const generation = useRef(0);
  const handoffs = JSON.stringify(
    workspace.outbox
      .filter(
        (job) =>
          job.load_id === load.id && job.destination === "stateset-icommerce",
      )
      .map((job) => [
        job.id,
        job.status,
        job.total_attempts,
        job.lease,
        job.leased_until,
        job.payload?.plan?.shipment_id,
      ]),
  );
  useEffect(() => {
    generation.current++;
    setReview(null);
    setReading(false);
    setError("");
    return () => {
      generation.current++;
    };
  }, [load.id, load.version, handoffs]);
  if (load.source_order?.sourceSystem !== "icommerce") return null;
  const writable = workspace.access.capabilities.includes("operations.write");
  const blocked = disabled || reading || !writable;
  async function preview(event) {
    event.preventDefault();
    await readShipment(shipmentId);
  }
  async function readShipment(selectedId) {
    const epoch = ++generation.current;
    setReading(true);
    setReview(null);
    setError("");
    try {
      const response = await identity.fetch(
        `/api/freight/loads/${encodeURIComponent(load.id)}/commerce-tracking?shipment_id=${encodeURIComponent(selectedId.trim())}`,
      );
      const result = await response.json();
      if (!response.ok)
        throw new Error(result.error?.message || "Shipment review unavailable");
      if (generation.current !== epoch) return;
      if (!result.data.up_to_date) trackingIntent(load, result.data);
      setReview(result.data);
    } catch (problem) {
      if (generation.current === epoch) setError(problem.message);
    } finally {
      if (generation.current === epoch) setReading(false);
    }
  }
  return (
    <section className="source-control" aria-label="iCommerce shipment handoff">
      <h3>iCommerce shipment handoff</h3>
      <p>
        Review the native shipment after warehouse packing and carrier handoff.
        Each confirmation queues one forward status step using its reviewed
        manifest and version.
      </p>
      <CommerceShipmentPicker
        key={`${load.version}-${handoffs}`}
        load={load}
        disabled={blocked}
        onChoose={(id) => {
          setShipmentId(id);
          readShipment(id);
        }}
        onOpenHandoffs={onOpenHandoffs}
      />
      <details>
        <summary>Enter a shipment UUID</summary>
        <form onSubmit={preview}>
          <label>
            Native shipment UUID
            <input
              required
              maxLength={36}
              value={shipmentId}
              disabled={blocked}
              onChange={(event) => {
                generation.current++;
                setShipmentId(event.target.value);
                setReview(null);
              }}
            />
          </label>
          <button
            className="secondary"
            disabled={
              blocked || !["in_transit", "delivered"].includes(load.status)
            }
          >
            Preview shipment step
          </button>
        </form>
      </details>
      {error && (
        <p role="alert" className="form-error">
          {error}
        </p>
      )}
      {review && (
        <article className="source-review">
          <strong>
            {review.up_to_date
              ? "Native shipment already reflects this freight state"
              : `${review.plan.from_status} → ${review.plan.next_status}`}
          </strong>
          <p>
            Shipment {review.shipment.id} · native version{" "}
            {review.shipment.version} · load version{" "}
            {review.expected_load_version}
          </p>
          <ul>
            {review.shipment.items.map((item, index) => (
              <li key={index}>
                {item.sku} · {item.quantity} units · product {item.product_id}
              </li>
            ))}
          </ul>
          {!review.up_to_date && (
            <button
              className="primary"
              disabled={
                blocked || review.expected_load_version !== load.version
              }
              onClick={() => {
                try {
                  const intent = trackingIntent(load, review);
                  setReview(null);
                  onQueue(
                    intent.path,
                    intent.body,
                    "Native shipment step queued for handoff",
                  );
                } catch (problem) {
                  setError(problem.message);
                }
              }}
            >
              Confirm reviewed shipment step
            </button>
          )}
          <p>
            Inspect the integration queue after confirmation. An uncertain
            native response stays held for review of the original handoff.
          </p>
        </article>
      )}
    </section>
  );
}
