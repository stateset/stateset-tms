// Only identity metadata and authentication errors use this budget. Shipment
// workspaces, partner portal listings, and original documents have other sizes.
export const MAX_ACCESS_RESPONSE_BYTES = 64 * 1024;

export async function discardAccessResponse(response) {
  await response.body?.cancel();
}

export async function readAccessJson(response, malformedFallback) {
  const limit = MAX_ACCESS_RESPONSE_BYTES;
  const declared = response.headers.get("content-length");
  if (
    declared !== null &&
    (!/^\d+$/.test(declared) || Number(declared) > limit)
  ) {
    await discardAccessResponse(response);
    throw new Error("API access response exceeds the allowed size.");
  }
  const reader = response.body?.getReader();
  const chunks = [];
  let size = 0;
  if (reader) {
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > limit)
          throw new Error("API access response exceeds the allowed size.");
        chunks.push(value);
      }
    } catch (error) {
      await reader.cancel().catch(() => {});
      throw error;
    } finally {
      reader.releaseLock();
    }
  }
  // Fetch exposes decompressed bytes: a small compressed Content-Length cannot
  // bypass the streamed limit. Decode only after the complete bounded read.
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(
      Buffer.concat(chunks, size),
    );
  } catch {
    throw new Error("API access response is not valid UTF-8.");
  }
  try {
    return JSON.parse(text);
  } catch {
    if (malformedFallback !== undefined) return malformedFallback;
    // Keep upstream corruption separate from a malformed browser request.
    throw new Error("API access response is not valid JSON.");
  }
}
