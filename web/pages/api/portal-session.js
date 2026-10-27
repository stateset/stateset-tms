import { cookie, seal, validOrigin } from "../../lib/session.mjs";
import { publicAuthConfig } from "../../lib/auth-config.mjs";
import {
  discardAccessResponse,
  readAccessJson,
} from "../../lib/access-response.mjs";

export default async function handler(req, res) {
  res.setHeader("Cache-Control", "no-store");
  if (publicAuthConfig().mode === "clerk")
    return res.status(403).json({
      error: { message: "Use StateSet partner sign-in for this portal." },
    });
  if (!["POST", "DELETE"].includes(req.method)) return res.status(405).end();
  if (!validOrigin(req))
    return res
      .status(403)
      .json({ error: { message: "Request origin is not allowed." } });
  if (req.method === "DELETE")
    return res.setHeader("Set-Cookie", cookie("", true)).json({ ok: true });
  const token = req.body?.token;
  if (typeof token !== "string" || token.length < 32 || token.length > 512)
    return res
      .status(400)
      .json({ error: { message: "Enter a valid partner credential." } });
  try {
    const upstream = await fetch(
      `${process.env.FREIGHT_API_URL || "http://127.0.0.1:8090"}/v1/portal`,
      {
        headers: { Authorization: `Bearer ${token}` },
        // API responses must never redirect credentials or original payloads.
        redirect: "error",
        signal: AbortSignal.timeout(10000),
      },
    );
    if (upstream.ok || upstream.status < 500)
      await discardAccessResponse(upstream);
    if (!upstream.ok && upstream.status >= 500) {
      const failure = await readAccessJson(upstream, null);
      if (
        ["read_capacity_exceeded", "database_temporarily_unavailable"].includes(
          failure?.error?.code,
        )
      ) {
        res.setHeader(
          "Retry-After",
          upstream.headers.get("retry-after") || "1",
        );
        return res.status(503).json({
          error: {
            code: failure.error.code,
            message:
              failure.error.code === "read_capacity_exceeded"
                ? "Workspace is busy. Please retry shortly."
                : "Database is temporarily busy. Retry the original request shortly.",
          },
        });
      }
      return res.status(503).json({
        error: {
          message:
            "Partner portal is temporarily unavailable. Please try again.",
        },
      });
    }
    if (!upstream.ok)
      return res
        .status(upstream.status)
        .json({ error: { message: "Partner credential was not accepted." } });
    res.setHeader("Set-Cookie", cookie(seal(token)));
    return res.json({ ok: true });
  } catch {
    return res.status(503).json({
      error: {
        message:
          "Sign-in unavailable. Check API connectivity and session configuration.",
      },
    });
  }
}
