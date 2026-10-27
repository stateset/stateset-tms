import { useEffect, useRef, useState } from "react";
import { useFreightIdentity } from "./FreightIdentity";
import { Modal } from "./Modal";
import { externalReviewIntent } from "../lib/commerce-review.mjs";

export function CommerceHandoffReview({ job, load, disabled, onQueue }) {
  const identity = useFreightIdentity();
  const generation = useRef(0);
  const [review, setReview] = useState(null);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState("");
  const [fields, setFields] = useState({
    contact: "",
    reference: "",
    note: "",
  });
  useEffect(() => {
    generation.current++;
    setReview(null);
    setReading(false);
    setError("");
    return () => {
      generation.current++;
    };
  }, [job.id, job.status, job.total_attempts, load?.version]);
  if (job.destination !== "stateset-icommerce" || job.status !== "held")
    return null;
  async function preview() {
    const epoch = ++generation.current;
    setReading(true);
    setError("");
    try {
      const response = await identity.fetch(
        `/api/freight/outbox/${encodeURIComponent(job.id)}/commerce-review`,
      );
      const result = await response.json();
      if (!response.ok)
        throw new Error(result.error?.message || "Native review unavailable");
      if (generation.current !== epoch) return;
      externalReviewIntent(job, load, result.data, {
        contact: "preview",
        reference: "preview",
        note: "preview",
      });
      setReview(result.data);
    } catch (problem) {
      if (generation.current === epoch) setError(problem.message);
    } finally {
      if (generation.current === epoch) setReading(false);
    }
  }
  return (
    <>
      <button
        className="secondary"
        disabled={disabled || reading || !load}
        onClick={preview}
      >
        {reading ? "Reading native shipment…" : "Review external advancement"}
      </button>
      {error && (
        <small role="alert" className="red">
          {error}
        </small>
      )}
      {review && (
        <Modal
          title="Review external native advancement"
          subtitle={load.reference}
          close={() => setReview(null)}
        >
          <form
            onSubmit={(event) => {
              event.preventDefault();
              try {
                const intent = externalReviewIntent(job, load, review, fields);
                setReview(null);
                onQueue(
                  intent.path,
                  intent.body,
                  "External native advancement reviewed",
                );
              } catch (problem) {
                setError(problem.message);
              }
            }}
          >
            <p>{review.explanation}</p>
            <p>
              Shipment {review.shipment.id} · status {review.shipment.status} ·
              original expected version {review.original_plan.expected_version}{" "}
              · observed version {review.shipment.version}
            </p>
            <ul>
              {review.shipment.items.map((item, index) => (
                <li key={index}>
                  {item.sku} · {item.quantity} units · product {item.product_id}
                </li>
              ))}
            </ul>
            {Object.keys(fields).map((field) => (
              <label key={field}>
                {field === "contact"
                  ? "External review contact"
                  : field === "reference"
                    ? "Evidence reference"
                    : "Review note"}
                <input
                  required
                  maxLength={200}
                  value={fields[field]}
                  onChange={(event) =>
                    setFields({ ...fields, [field]: event.target.value })
                  }
                />
              </label>
            ))}
            {error && (
              <p role="alert" className="form-error">
                {error}
              </p>
            )}
            <p>
              This retires the held local handoff. It sends no native PATCH and
              leaves the original PATCH outcome unknown. Queue any next
              lifecycle step separately after a fresh review.
            </p>
            <button className="primary" disabled={disabled || reading}>
              Confirm review and retire local handoff
            </button>
          </form>
        </Modal>
      )}
    </>
  );
}
