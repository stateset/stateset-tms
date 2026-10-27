import { trackingIntent } from "./commerce-tracking.mjs";
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const id = (value) =>
  typeof value === "string" && /^[A-Za-z0-9_-]{1,200}$/.test(value);
const text = (value, max) =>
  typeof value === "string" &&
  value.trim() &&
  value.length <= max &&
  !/[\x00-\x1f\x7f-\x9f]/.test(value);
const activeStates = ["pending_handoff", "sending", "retry", "held"];
export const MAX_SHIPMENT_PAGE = Math.floor(4294967295 / 25) + 1;
export function validateShipmentPage(load, page, requestedPage) {
  const invalid = () => {
    throw new Error(
      "Shipment list no longer matches this load or page. Refresh the list.",
    );
  };
  if (
    !load ||
    !page ||
    !id(load.id) ||
    load.source_order?.sourceSystem !== "icommerce" ||
    !Number.isSafeInteger(load.version) ||
    load.version < 0 ||
    page.load_id !== load.id ||
    page.expected_load_version !== load.version ||
    !uuid.test(load.sync_order_id) ||
    page.order_id !== load.sync_order_id ||
    !Number.isSafeInteger(requestedPage) ||
    requestedPage < 1 ||
    requestedPage > MAX_SHIPMENT_PAGE ||
    page.page !== requestedPage ||
    page.page_size !== 25 ||
    typeof page.has_more !== "boolean" ||
    !Array.isArray(page.items) ||
    page.items.length > 25 ||
    (page.has_more && page.items.length !== 25) ||
    !Array.isArray(page.active_handoffs) ||
    page.active_handoffs.some(
      (j) => !id(j?.id) || !activeStates.includes(j.status),
    )
  )
    invalid();
  const seen = new Set();
  for (const row of page.items) {
    const s = row?.shipment;
    if (
      !s ||
      !uuid.test(s.id) ||
      seen.has(s.id) ||
      s.order_id !== page.order_id ||
      !Number.isInteger(s.version) ||
      s.version < 0 ||
      s.version > 2147483647 ||
      !text(s.status, 100) ||
      !text(row.shipment_number, 200) ||
      !text(row.reason, 1000) ||
      !["ready_for_review", "up_to_date", "blocked"].includes(row.state)
    )
      invalid();
    seen.add(s.id);
    if (row.state === "ready_for_review") {
      if (page.active_handoffs.length) invalid();
      trackingIntent(load, {
        load_id: page.load_id,
        expected_load_version: page.expected_load_version,
        shipment: s,
        plan: row.plan,
      });
    }
  }
  return page;
}
export function candidateShipmentId(load, page, shipmentId) {
  validateShipmentPage(load, page, page.page);
  const row = page.items.find((r) => r.shipment.id === shipmentId);
  if (!row || row.state !== "ready_for_review" || page.active_handoffs.length)
    throw new Error(
      "This native shipment cannot start another step. Refresh the list or review Integration handoffs.",
    );
  // Return identity only: the selection must fetch a fresh individual preview.
  return row.shipment.id;
}
