import { readAccessJson } from "./access-response.mjs";

// The gateway compares a reviewed browser identity with an independently
// verified upstream identity, then sends the mutation with that same captured
// token/organization. The expected identity never grants access.
export async function verifyExpectedAccess(
  value,
  url,
  headers,
  fetcher = fetch,
) {
  if (value === undefined) return null;
  let expected;
  try {
    if (typeof value !== "string" || value.length > 2000) throw new Error();
    expected = JSON.parse(decodeURIComponent(value));
    if (
      typeof expected.tenant_id !== "string" ||
      typeof expected.subject !== "string" ||
      Object.keys(expected).sort().join() !== "subject,tenant_id"
    )
      throw new Error();
  } catch {
    return {
      status: 400,
      body: { error: { message: "Invalid reviewed workspace identity." } },
    };
  }
  const response = await fetcher(`${url}/v1/access`, {
    headers,
    redirect: "error",
    signal: AbortSignal.timeout(10000),
  });
  if (!response.ok) {
    return {
      status: response.status,
      retryAfter: response.headers.get("retry-after"),
      body: await readAccessJson(response, {
        error: { message: "Workspace access could not be verified." },
      }),
    };
  }
  const actual = await readAccessJson(response);
  if (
    actual.tenant_id !== expected.tenant_id ||
    actual.access?.subject !== expected.subject
  )
    return {
      status: 409,
      body: {
        error: {
          code: "workspace_session_changed",
          message:
            "This workspace session has changed. Sign in again before saving.",
        },
      },
    };
  return null;
}
