import { verifyExpectedAccess } from "../../../lib/expected-access.mjs";
import { tokenFor, validOrigin } from "../../../lib/session.mjs";
import { boundedBody, MAX_DOCUMENT_BYTES } from "../../../lib/gateway.mjs";
import {
  clerkGatewayIdentity,
  publicAuthConfig,
} from "../../../lib/auth-config.mjs";

export const config = { api: { bodyParser: false, responseLimit: "11mb" } };

// Tenant comes from the API-verified identity, never a browser tenant header.
export default async function handler(req, res) {
  res.setHeader("Cache-Control", "no-store");
  const staff = publicAuthConfig().mode === "clerk";
  const identity = staff ? clerkGatewayIdentity(req) : null;
  const token = staff ? identity?.token : tokenFor(req);
  if (!token)
    return res
      .status(401)
      .json({ error: { message: "Sign in to your freight workspace." } });
  const path = (req.query.path || []).map(encodeURIComponent).join("/");
  if (!["GET", "POST"].includes(req.method)) return res.status(405).end();
  if (req.method === "POST" && !validOrigin(req)) {
    return res
      .status(403)
      .json({ error: { message: "Request origin is not allowed." } });
  }
  try {
    if (
      req.method === "POST" ||
      (req.method === "GET" &&
        ((req.query.path?.length === 1 && req.query.path[0] === "history") ||
          (req.query.path?.length === 3 &&
            req.query.path[0] === "invoices" &&
            req.query.path[2] === "approval-evidence")))
    ) {
      const failure = await verifyExpectedAccess(
        req.headers["x-freight-expected-access"],
        process.env.FREIGHT_API_URL || "http://127.0.0.1:8090",
        {
          Authorization: `Bearer ${token}`,
          ...(staff
            ? {
                "X-Freight-Identity-Mode": "clerk",
                "X-Freight-Organization-Id": identity.organization,
              }
            : {}),
        },
      );
      if (failure) {
        if (failure.retryAfter)
          res.setHeader("Retry-After", failure.retryAfter);
        return res.status(failure.status).json(failure.body);
      }
    }
    const search = new URLSearchParams();
    if (
      req.method === "GET" &&
      req.query.path?.length === 1 &&
      ["workspace", "history"].includes(req.query.path[0])
    ) {
      const names =
        req.query.path[0] === "workspace"
          ? ["history"]
          : ["kind", "load_id", "limit", "cursor"];
      for (const name of names) {
        if (Array.isArray(req.query[name]))
          return res.status(400).json({
            error: { message: "History filters require one value each." },
          });
        if (typeof req.query[name] === "string")
          search.set(name, req.query[name]);
      }
    }
    if (
      req.method === "GET" &&
      req.query.path?.length === 3 &&
      req.query.path[0] === "loads" &&
      req.query.path[2] === "commerce-tracking"
    ) {
      if (typeof req.query.shipment_id !== "string")
        return res.status(400).json({
          error: { message: "One native shipment UUID is required." },
        });
      search.set("shipment_id", req.query.shipment_id);
    }
    if (
      req.method === "GET" &&
      req.query.path?.length === 3 &&
      req.query.path[0] === "loads" &&
      req.query.path[2] === "commerce-shipments"
    ) {
      if (
        Object.keys(req.query).some((key) => !["path", "page"].includes(key)) ||
        (req.query.page !== undefined && typeof req.query.page !== "string")
      )
        return res
          .status(400)
          .json({
            error: {
              message: "Native shipment discovery accepts one page only.",
            },
          });
    }
    if (typeof req.query.page === "string") search.set("page", req.query.page);
    if (
      req.method === "GET" &&
      req.query.path?.length === 1 &&
      req.query.path[0] === "operations"
    ) {
      for (const name of ["view", "search", "limit", "cursor"]) {
        if (Array.isArray(req.query[name]))
          return res.status(400).json({
            error: { message: "Queue filters require one value each." },
          });
        if (typeof req.query[name] === "string")
          search.set(name, req.query[name]);
      }
    }
    if (
      req.method === "GET" &&
      req.query.path?.length === 1 &&
      req.query.path[0] === "loads"
    ) {
      for (const name of [
        "limit",
        "cursor",
        "status",
        "customer_id",
        "carrier_id",
        "q",
      ]) {
        if (Array.isArray(req.query[name]))
          return res.status(400).json({
            error: { message: "Load filters require one value each." },
          });
        if (typeof req.query[name] === "string")
          search.set(name, req.query[name]);
      }
    }
    if (
      req.method === "GET" &&
      req.query.path?.length === 3 &&
      ["quotes", "carrier-contracts"].includes(req.query.path[0]) &&
      req.query.path[2] === "statement" &&
      typeof req.query.revision === "string"
    )
      search.set("revision", req.query.revision);
    const staffUpload =
      req.method === "POST" &&
      req.query.path?.length === 3 &&
      req.query.path[0] === "loads" &&
      req.query.path[2] === "documents";
    const carrierPod =
      req.method === "POST" &&
      req.query.path?.length === 4 &&
      req.query.path[0] === "portal" &&
      req.query.path[1] === "offers" &&
      req.query.path[3] === "pod";
    const upload = staffUpload || carrierPod;
    if (upload) {
      for (const name of carrierPod
        ? ["filename", "expected_load_version", "reference", "note"]
        : ["kind", "filename"]) {
        if (typeof req.query[name] === "string")
          search.set(name, req.query[name]);
      }
    }
    const body =
      req.method === "POST"
        ? await boundedBody(req, upload ? MAX_DOCUMENT_BYTES : 1024 * 1024)
        : undefined;
    const upstream = await fetch(
      `${process.env.FREIGHT_API_URL || "http://127.0.0.1:8090"}/v1/${path}${search.size ? `?${search}` : ""}`,
      {
        method: req.method,
        headers: {
          Authorization: `Bearer ${token}`,
          ...(staff
            ? {
                "X-Freight-Identity-Mode": "clerk",
                "X-Freight-Organization-Id": identity.organization,
              }
            : {}),
          "Content-Type": upload
            ? req.headers["content-type"] || "application/octet-stream"
            : "application/json",
          ...(req.method === "GET" &&
          path === "workspace" &&
          req.headers["x-freight-approval-details"]
            ? {
                "X-Freight-Approval-Details":
                  req.headers["x-freight-approval-details"],
              }
            : {}),
          ...(req.headers["idempotency-key"]
            ? { "Idempotency-Key": req.headers["idempotency-key"] }
            : {}),
        },
        ...(body !== undefined ? { body } : {}),
        // API responses must never redirect credentials or original payloads.
        redirect: "error",
        signal: AbortSignal.timeout(15000),
      },
    );
    const content = Buffer.from(await upstream.arrayBuffer());
    res.setHeader("Cache-Control", "no-store");
    for (const header of [
      "content-type",
      "content-disposition",
      "x-content-sha256",
      "x-content-type-options",
      "content-security-policy",
      "retry-after",
    ]) {
      const value = upstream.headers.get(header);
      if (value) res.setHeader(header, value);
    }
    res.status(upstream.status).send(content);
  } catch (error) {
    if (error.status === 413)
      return res.status(413).json({ error: { message: error.message } });
    res.status(503).json({
      error: {
        message: "Freight API unavailable. Start the Rust service and retry.",
      },
    });
  }
}
