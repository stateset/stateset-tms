// These helpers scope each operation/download to the organization that rendered
// it. The API verifies that scope against the signed identity, never trusting it
// as a tenant grant. A stale screen cannot silently post into a new organization.
export function organizationHref(path, organization) {
  if (!organization) return path;
  const url = new URL(path, "http://freight.invalid");
  if (
    url.origin !== "http://freight.invalid" ||
    !url.pathname.startsWith("/api/freight/")
  )
    throw new Error("Invalid freight download path");
  url.searchParams.set("organization_id", organization);
  return `${url.pathname}${url.search}`;
}
export function organizationFetch(
  organization,
  getToken,
  fetcher = globalThis.fetch,
) {
  return async (path, options = {}) => {
    if (typeof path !== "string" || !path.startsWith("/api/freight/"))
      throw new Error("Invalid freight API path");
    const token = await getToken({ organizationId: organization });
    if (!token)
      return new Response(
        JSON.stringify({
          error: { message: "A current staff session is required." },
        }),
        { status: 401, headers: { "Content-Type": "application/json" } },
      );
    const headers = new Headers(options.headers);
    headers.set("Authorization", `Bearer ${token}`);
    headers.set("X-Freight-Organization-Id", organization);
    return fetcher(path, { ...options, headers });
  };
}
