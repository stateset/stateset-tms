export function publicAuthConfig(env = process.env) {
  const mode = env.FREIGHT_WEB_AUTH_MODE || "credential";
  if (!["credential", "clerk"].includes(mode))
    throw new Error("Invalid FREIGHT_WEB_AUTH_MODE");
  if (mode === "credential") return { mode };
  const publishableKey = env.FREIGHT_CLERK_PUBLISHABLE_KEY || "";
  if (!/^pk_(live|test)_[A-Za-z0-9_-]+$/.test(publishableKey))
    throw new Error("Clerk web sign-in requires FREIGHT_CLERK_PUBLISHABLE_KEY");
  const domain = Buffer.from(
    publishableKey.replace(/^pk_(live|test)_/, ""),
    "base64url",
  ).toString("utf8");
  if (!/^[a-z0-9.-]+\$$/.test(domain) || !env.FREIGHT_WEB_ORIGIN)
    throw new Error(
      "Clerk web sign-in requires a valid publishable key and fixed FREIGHT_WEB_ORIGIN",
    );
  const origin = new URL(env.FREIGHT_WEB_ORIGIN);
  if (
    origin.origin !== env.FREIGHT_WEB_ORIGIN ||
    (origin.protocol !== "https:" &&
      !["localhost", "127.0.0.1"].includes(origin.hostname))
  )
    throw new Error(
      "Clerk web origin must use HTTPS (loopback development excepted)",
    );
  if (env.FREIGHT_DEMO === "true")
    throw new Error(
      "Clerk web sign-in cannot use automatic web demo credentials",
    );
  return { mode, publishableKey };
}

export function clerkGatewayIdentity(req) {
  const authorization = req.headers.authorization;
  if (
    authorization &&
    (typeof authorization !== "string" || !authorization.startsWith("Bearer "))
  )
    return null;
  const cookie = (req.headers.cookie || "")
    .split(";")
    .map((v) => v.trim())
    .filter((v) => v.startsWith("__session="));
  const bearer =
    typeof authorization === "string" && authorization.startsWith("Bearer ")
      ? authorization.slice(7)
      : null;
  // Header tokens are used by SDK-authenticated requests; ordinary downloads use
  // Clerk's short-lived cookie. Neither path accepts a static workspace cookie.
  const token =
    bearer ||
    (cookie.length === 1 ? cookie[0].slice("__session=".length) : null);
  const headerOrg = req.headers["x-freight-organization-id"],
    queryOrg = req.query?.organization_id;
  const organization =
    typeof headerOrg === "string"
      ? headerOrg
      : typeof queryOrg === "string"
        ? queryOrg
        : null;
  if (headerOrg && queryOrg && headerOrg !== queryOrg) return null;
  if (
    !token ||
    token.length > 16384 ||
    !/^[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+$/.test(token) ||
    !/^org_[A-Za-z0-9_-]{1,124}$/.test(organization || "")
  )
    return null;
  return { token, organization };
}
