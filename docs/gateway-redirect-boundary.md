# Gateway redirect boundary

The web server now refuses redirects from the configured Freight API. The rule covers staff and partner credential sign-in, reviewed-identity preflight, encrypted save-recovery authorization, and the shared API proxy for reads, saves, uploads, and downloads. Existing exception handlers return an uncached 503. Sign-in cannot mint a cookie from a redirected response, and a redirected identity preflight cannot authorize the subsequent mutation.

The [Fetch standard](https://fetch.spec.whatwg.org/#concept-request-redirect-mode) defaults requests to following redirects. Each server-side API fetch now explicitly selects `redirect: "error"`. This rejects both same-origin and cross-origin redirects, including method-changing redirects. Configure `FREIGHT_API_URL` to the final API origin/path and correct reverse-proxy routes instead of relying on a redirect to repair them.

This boundary does not authenticate a misconfigured initial API destination, replace HTTPS, or reverse work already received by that API. A failed save response remains an uncertain outcome: retain the original body and idempotency key, inspect its receipt, and use the existing explicit recovery workflow. The gateway does not retry the request automatically or create a replacement key.

## Measured behavior

The production qualification runs actual standalone Next.js processes against owned loopback HTTP peers with synthetic credentials. It covers eight entry paths, five redirect statuses (301, 302, 303, 307, 308), and same-origin and cross-origin destinations: **80 redirect cases**, plus four positive controls.

The original production build failed 72 redirect cases and passed 12 checks. Destination listeners observed 72 redirected requests, including 36 same-origin requests carrying the synthetic credential. **No redirected body bytes were observed.** Eight streamed POST cases already rejected 307/308 responses. The change makes redirect refusal explicit and consistent across all entry paths; it does not claim a demonstrated body leak where none occurred.

The rebuilt production gateway passes all **84 checks**, with zero redirected requests or credentials. Positive controls retain normal staff/partner sign-in, original cookies after temporary failure, exact save bodies and idempotency keys on deliberate retry, original document bytes/digests, and encryption/decryption of the original recovery intent. A separate real-HTTP preflight regression is included in the **27 passing Node tests**.

All **101 production-browser scenarios pass**, with no skipped, failed, or flaky tests, against the rebuilt web application and unchanged Rust API. They cover staff and partner operations, multi-stop execution, financial approvals, EDI recovery ownership, exact original-save recovery, encrypted browser storage, and session isolation. The production build, web formatting, and standard-check script syntax also pass.

## Reproduction

Use Node 22 and installed locked dependencies:

```sh
npm --prefix web run test:unit
npm --prefix web run build
node web/scripts/qualify-gateway-redirects.mjs /tmp/freight-gateway-new-report.json
```

The report path must not exist. The harness chooses loopback ports, starts its own web process and HTTP fixtures, records the production build ID, and stops its services. It does not accept a deployment URL or production credential. The standard `scripts/check.sh` workflow now runs this qualification after building the web application.

These checks establish a local request boundary. Production ingress behavior, live identity/partner commissioning, representative sustained load, off-host recovery, and independent operator outcomes remain required for an A+ operating assessment.
