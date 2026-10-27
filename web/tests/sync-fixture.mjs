// Test-owned canonical order/tracking contract. No external commerce connection.
const tenant = "00000000-0000-4000-8000-000000000901";
let order = null;
let unavailable = false;
const tracking = [];
async function body(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  return JSON.parse(Buffer.concat(chunks).toString());
}
export async function syncFixture(request, response) {
  const pathname = new URL(request.url, "http://localhost").pathname;
  if (pathname.startsWith("/__sync_fixture/")) {
    if (request.headers["x-fixture-key"] !== "freight-sync-fixture-only") {
      response.statusCode = 403;
      response.end('{"error":"Fixture access denied"}');
      return true;
    }
    if (request.method === "PUT" && pathname === "/__sync_fixture/order") {
      order = await body(request);
      tracking.length = 0;
      unavailable = false;
      response.end('{"ok":true}');
      return true;
    }
    if (request.method === "PATCH" && pathname === "/__sync_fixture/order") {
      order = { ...order, ...(await body(request)) };
      response.end('{"ok":true}');
      return true;
    }
    if (
      request.method === "PUT" &&
      pathname === "/__sync_fixture/availability"
    ) {
      unavailable = (await body(request)).unavailable === true;
      response.end('{"ok":true}');
      return true;
    }
    if (request.method === "GET" && pathname === "/__sync_fixture/tracking") {
      response.end(JSON.stringify({ tracking, order }));
      return true;
    }
  }
  if (!pathname.startsWith(`/v1/tenants/${tenant}/orders/`)) return false;
  if (request.headers["x-stateset-api-key"] !== "freight-source-sync-fixture") {
    response.statusCode = 403;
    response.end('{"error":"Wrong sync credential"}');
    return true;
  }
  if (
    order &&
    request.method === "GET" &&
    pathname === `/v1/tenants/${tenant}/orders/${order.id}`
  ) {
    if (unavailable) {
      response.statusCode = 503;
      response.end('{"error":"Owned source outage"}');
      return true;
    }
    // Actual convert_order_row decodes TrackingInfo and serializes camelCase.
    const canonical = { ...order };
    if (order.tracking)
      canonical.tracking = {
        trackingNumber: order.tracking.trackingNumber ?? null,
        carrier: order.tracking.carrier ?? null,
        trackingUrl: order.tracking.trackingUrl ?? null,
        estimatedDelivery: order.tracking.estimatedDelivery ?? null,
      };
    response.end(JSON.stringify({ data: canonical }));
    return true;
  }
  if (
    order &&
    request.method === "POST" &&
    pathname === `/v1/tenants/${tenant}/orders/${order.id}/tracking`
  ) {
    tracking.push({
      path: pathname,
      idempotency_key: request.headers["idempotency-key"],
      body: await body(request),
    });
    // Match the actual sibling database/orders.rs update_order_tracking semantics.
    order.status = "shipped";
    order.tracking = tracking.at(-1).body.trackingData;
    order.updatedAt = new Date().toISOString();
    response.end('{"data":{"ok":true}}');
    return true;
  }
  response.statusCode = 404;
  response.end('{"error":"No such owned order"}');
  return true;
}
