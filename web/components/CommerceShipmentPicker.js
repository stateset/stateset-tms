import { useEffect, useRef, useState } from "react";
import { useFreightIdentity } from "./FreightIdentity";
import {
  candidateShipmentId,
  validateShipmentPage,
  MAX_SHIPMENT_PAGE,
} from "../lib/commerce-shipments.mjs";

export function CommerceShipmentPicker({
  load,
  disabled,
  onChoose,
  onOpenHandoffs,
}) {
  const identity = useFreightIdentity();
  const generation = useRef(0);
  const [page, setPage] = useState(null);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState("");
  useEffect(
    () => () => {
      generation.current++;
    },
    [],
  );
  const blocked = disabled || reading;
  async function read(requestedPage) {
    const epoch = ++generation.current;
    setReading(true);
    setError("");
    try {
      const response = await identity.fetch(
        `/api/freight/loads/${encodeURIComponent(load.id)}/commerce-shipments?page=${requestedPage}`,
      );
      const result = await response.json();
      if (!response.ok)
        throw new Error(
          result.error?.message || "Native shipments unavailable",
        );
      if (epoch !== generation.current) return;
      setPage(validateShipmentPage(load, result.data, requestedPage));
    } catch (problem) {
      if (epoch === generation.current) setError(problem.message);
    } finally {
      if (epoch === generation.current) setReading(false);
    }
  }
  return (
    <section aria-label="Native shipments for this order" aria-busy={reading}>
      <button
        type="button"
        className="secondary"
        disabled={blocked}
        onClick={() => read(1)}
      >
        {reading
          ? "Finding native shipments…"
          : page
            ? "Refresh shipment list"
            : "Find order shipments"}
      </button>
      {error && (
        <p role="alert" className="form-error">
          {error}
        </p>
      )}
      {page && (
        <>
          <p>
            Order {page.order_id} · Page {page.page}. Choose a shipment to
            review its manifest and next step.
          </p>
          {page.active_handoffs.length > 0 && (
            <article className="source-review">
              <strong>Existing native handoff needs attention</strong>
              {page.active_handoffs.map((job) => (
                <p key={job.id}>
                  {job.id} · {job.status.replaceAll("_", " ")}
                </p>
              ))}
              {onOpenHandoffs && (
                <button
                  type="button"
                  className="secondary"
                  onClick={onOpenHandoffs}
                >
                  Open integration handoffs
                </button>
              )}
            </article>
          )}
          {!page.items.length && (
            <p>
              No native shipments on this page. Packing and carrier handoff are
              managed in iCommerce.
            </p>
          )}
          {page.items.map((row) => (
            <article className="source-review" key={row.shipment.id}>
              <strong>{row.shipment_number}</strong>
              <p style={{ overflowWrap: "anywhere" }}>{row.shipment.id}</p>
              <p>
                {row.shipment.status.replaceAll("_", " ")} · version{" "}
                {row.shipment.version}
              </p>
              <p>{row.reason}</p>
              {row.state === "ready_for_review" && (
                <button
                  type="button"
                  className="secondary"
                  disabled={blocked}
                  onClick={() => {
                    try {
                      onChoose(
                        candidateShipmentId(load, page, row.shipment.id),
                      );
                    } catch (problem) {
                      setError(problem.message);
                    }
                  }}
                >
                  Preview this shipment step
                </button>
              )}
            </article>
          ))}
          <nav aria-label="Native shipment pages">
            <button
              type="button"
              className="secondary"
              disabled={blocked || page.page <= 1}
              onClick={() => read(page.page - 1)}
            >
              Previous shipments
            </button>{" "}
            <button
              type="button"
              className="secondary"
              disabled={
                blocked || !page.has_more || page.page >= MAX_SHIPMENT_PAGE
              }
              onClick={() => read(page.page + 1)}
            >
              Next shipments
            </button>
          </nav>
          <small>
            Pages reflect live native records. Holds and shipment versions are
            checked again when you confirm.
          </small>
        </>
      )}
    </section>
  );
}
