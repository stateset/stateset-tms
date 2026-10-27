import test from "node:test";
import assert from "node:assert/strict";
import { Readable } from "node:stream";
import { boundedBody } from "../lib/gateway.mjs";
const request = (chunks, headers = {}) =>
  Object.assign(Readable.from(chunks), { headers });

test("gateway preserves exact binary bytes and enforces limits with or without content length", async () => {
  const bytes = Buffer.from([0, 0xff, 0xfe, 0x80, 0x41]);
  assert.deepEqual(
    await boundedBody(request([bytes.subarray(0, 2), bytes.subarray(2)]), 5),
    bytes,
  );
  await assert.rejects(
    boundedBody(request([bytes], { "content-length": "6" }), 5),
    { status: 413 },
  );
  await assert.rejects(boundedBody(request([bytes, bytes]), 5), {
    status: 413,
  });
  await assert.rejects(
    boundedBody(request([], { "content-length": "invalid" }), 5),
    { status: 413 },
  );
});
