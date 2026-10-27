const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function trackingIntent(load, review) {
  if (
    !load ||
    !review ||
    load.source_order?.sourceSystem !== "icommerce" ||
    !["in_transit", "delivered"].includes(load.status) ||
    review.load_id !== load.id ||
    review.expected_load_version !== load.version ||
    !Number.isSafeInteger(load.version) ||
    !uuid.test(review.shipment?.id) ||
    review.plan?.shipment_id !== review.shipment.id ||
    review.plan?.order_id !== load.sync_order_id ||
    review.shipment.order_id !== load.sync_order_id ||
    !Number.isSafeInteger(review.shipment.version) ||
    review.shipment.version < 0 ||
    review.plan.expected_version !== review.shipment.version ||
    !["in_transit", "out_for_delivery", "delivered"].includes(
      review.plan.next_status,
    )
  )
    throw new Error(
      "Shipment or load review changed; preview again before confirming.",
    );
  return {
    path: `loads/${load.id}/commerce-tracking`,
    body: {
      expected_load_version: load.version,
      shipment_id: review.shipment.id,
      expected_shipment_version: review.shipment.version,
    },
  };
}
