export const MAX_DOCUMENT_BYTES = 10 * 1024 * 1024;
export async function boundedBody(request, limit) {
  const declared = request.headers["content-length"];
  if (declared && (!/^\d+$/.test(declared) || Number(declared) > limit)) {
    throw Object.assign(new Error("Request body exceeds the allowed size."), {
      status: 413,
    });
  }
  let size = 0;
  const chunks = [];
  for await (const chunk of request) {
    size += chunk.length;
    if (size > limit)
      throw Object.assign(new Error("Request body exceeds the allowed size."), {
        status: 413,
      });
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}
