# Authentication response budgets

The web gateway now bounds the API identity metadata and authentication errors it reads to **65,536 bytes**. This applies to reviewed-identity preflight, save-recovery authorization, and staff/partner sign-in error parsing. Sign-in cancels successful and denied response bodies it does not need, including large partner portal listings, instead of leaving them open until the network timeout.

The shared reader checks declared length and actual streamed bytes before retaining each chunk. Exactly 65,536 bytes are accepted. Streamed checks also cover chunked responses and decompressed gzip bodies whose declared compressed size is small. An unfinished oversized response is cancelled immediately; the existing ten-second fetch deadline still covers smaller stalled responses. JSON parsing happens only after the complete bounded body is read, and invalid UTF-8 is rejected instead of repaired.

An oversized or unreadable identity response produces an uncached 503. It cannot authorize a subsequent save, create a recovery envelope, or revoke the existing cookie. Ordinary bounded 401/403 denials and recognized 503 busy responses retain their existing behavior and retry hints. Malformed successful recovery-authority JSON now produces a service-unavailable response instead of blaming the browser envelope with a 400. Sign-in continues to use the trusted API's success status; it does not decode portal listings to establish a session.

This limit applies only to authentication metadata and parsed authentication errors. It does not limit ordinary workspace responses or original document transfers, and it is not a total process-memory or deployment-capacity bound. A failure never automatically retries a business request. Preserve the original save and its idempotency key and use the existing receipt-recovery flow when its outcome is uncertain.

## Evidence

The corrected baseline fixture runs against the preceding standalone production build: **nine checks pass and 17 fail**. Failures include acceptance of oversized identity metadata, treating oversized denials as session rejection, and leaving unused sign-in bodies open. The rebuilt production application passes all **34 response-budget checks**, including eight additional cancellation and invalid-UTF-8 cases. These tests use real owned HTTP peers, not a mocked fetch implementation, and inspect whether a mutation reached the API and whether unfinished responses were closed.

All **61 Node tests** pass, including the same 34 checks through the route handlers, and all **84 production redirect checks** still pass. These overlapping test executions are reported separately rather than summed into a distinct-test total. The production build and web formatting pass. The standard `scripts/check.sh` now exercises the response-budget cases against the built standalone server as well as through the ordinary Node suite.

All **19 selected production-browser scenarios** pass without skips or flaky results. They cover staff and partner sign-in, organization membership and revocation, original saves interrupted before/after commit, tender recovery without duplicate EDI jobs, encrypted recovery, and session lifecycle. The preceding full 101-scenario browser run remains historical; this change reruns the affected workflows. All 485 protected backend, dependency, and preceding evidence files remain unchanged.

## Reproduction

With Node 22 and installed locked dependencies:

```sh
npm --prefix web run test:unit
npm --prefix web run build
FREIGHT_ACCESS_TEST_PRODUCTION=true node --test web/tests/access-response-budget.test.mjs
```

Wait for the build to finish successfully before starting the production checks. The tests allocate their own loopback ports, use synthetic credentials, and stop their own processes and HTTP peers. Production identity/partner commissioning, sustained deployment testing, off-host restoration, and independent operator outcomes remain open A+ evidence gates.
