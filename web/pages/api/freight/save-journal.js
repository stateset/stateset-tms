import { tokenFor, validOrigin } from "../../../lib/session.mjs";
import {
  clerkGatewayIdentity,
  publicAuthConfig,
} from "../../../lib/auth-config.mjs";
import { boundedBody } from "../../../lib/gateway.mjs";
import { readAccessJson } from "../../../lib/access-response.mjs";
import {
  sealJournalIntent,
  openJournalIntent,
} from "../../../lib/save-journal-crypto.mjs";
export const config = { api: { bodyParser: false, responseLimit: "2mb" } };
export default async function handler(req, res) {
  res.setHeader("Cache-Control", "no-store");
  res.setHeader("X-Content-Type-Options", "nosniff");
  if (req.method !== "POST") return res.status(405).end();
  if (!validOrigin(req))
    return res
      .status(403)
      .json({ error: { message: "Request origin is not allowed." } });
  const staff = publicAuthConfig().mode === "clerk",
    identity = staff ? clerkGatewayIdentity(req) : null;
  const token = staff ? identity?.token : tokenFor(req);
  if (!token)
    return res
      .status(401)
      .json({ error: { message: "Sign in to your freight workspace." } });
  try {
    const upstream = await fetch(
      `${process.env.FREIGHT_API_URL || "http://127.0.0.1:8090"}/v1/access`,
      {
        headers: {
          Authorization: `Bearer ${token}`,
          ...(staff
            ? {
                "X-Freight-Identity-Mode": "clerk",
                "X-Freight-Organization-Id": identity.organization,
              }
            : {}),
        },
        // API responses must never redirect credentials or original payloads.
        redirect: "error",
        signal: AbortSignal.timeout(10000),
      },
    );
    if (!upstream.ok) {
      if (upstream.headers.get("retry-after"))
        res.setHeader("Retry-After", upstream.headers.get("retry-after"));
      return res.status(upstream.status).json(
        await readAccessJson(upstream, {
          error: { message: "Workspace access could not be verified." },
        }),
      );
    }
    const access = await readAccessJson(upstream);
    if (!access.access?.capabilities?.includes("workspace.read"))
      return res.status(403).json({
        error: {
          message: "Staff workspace access is required for save recovery.",
        },
      });
    const input = JSON.parse(
      (await boundedBody(req, 2 * 1024 * 1024)).toString("utf8"),
    );
    if (
      !input ||
      !["seal", "open"].includes(input.action) ||
      Object.keys(input).sort().join() !==
        (input.action === "seal"
          ? "action,intent,owner"
          : "action,owner,sealed")
    )
      return res
        .status(400)
        .json({ error: { message: "Invalid save recovery envelope." } });
    let session = "";
    if (staff) {
      // The API has verified this exact signed token and its live session.
      session = JSON.parse(
        Buffer.from(token.split(".")[1], "base64url").toString("utf8"),
      ).sid;
      if (
        typeof session !== "string" ||
        !/^sess_[A-Za-z0-9_-]{1,128}$/.test(session)
      )
        throw new Error("Invalid verified session");
    }
    const owner = JSON.stringify([
      staff ? "clerk" : "credential",
      staff ? identity.organization : "",
      access.tenant_id,
      access.access.subject,
      session,
    ]);
    if (input.owner !== owner)
      return res.status(409).json({
        error: {
          code: "workspace_session_changed",
          message:
            "This workspace session has changed. Sign in again before recovery.",
        },
      });
    return res.json({
      data:
        input.action === "seal"
          ? sealJournalIntent(owner, input.intent)
          : openJournalIntent(owner, input.sealed),
    });
  } catch (error) {
    const status = error.status || (error instanceof SyntaxError ? 400 : 503);
    return res.status(status).json({
      error: {
        ...(error.code ? { code: error.code } : {}),
        message: error.status
          ? error.message
          : status === 400
            ? "Invalid save recovery envelope."
            : "Save recovery service is unavailable. Retain the original and retry.",
      },
    });
  }
}
