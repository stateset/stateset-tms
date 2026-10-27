# Process probes and HTTP monitoring

Freight exposes separate process liveness, dependency readiness and protected Prometheus metrics. These signals support commissioning; they do not establish a production service-level objective or replace the actual-service recovery and sustained-load qualifications.

The later [first-failure qualification](integration-failure-detection.md) runs all 355 current Rust tests, including 71 PostgreSQL tests with none skipped, and repeats the 15 process checks against its new API binary. It also adds current-source control-tower browser and pinned-version EDI/Sync recovery evidence. The original observability reports above retain their historical source and scope.

## Probes

| Endpoint      | Meaning                                                                | Dependencies                                    |
| ------------- | ---------------------------------------------------------------------- | ----------------------------------------------- |
| `GET /live`   | This HTTP process can respond.                                         | No database, identity provider or partner call. |
| `GET /ready`  | The process is accepting work and storage reports the expected format. | One storage check with a one-second deadline.   |
| `GET /health` | Compatibility alias for readiness.                                     | Same check and deadline as `/ready`.            |

The endpoints are unauthenticated and return generic service/status JSON with `Cache-Control: no-store`. Readiness returns 503 for an unavailable database, incompatible storage format, check timeout or requested shutdown. Memory/demo storage can be ready; readiness does not certify durable storage or production configuration. A healthy database does not prove that Clerk, EDI or Sync is reachable.

Database I/O, closed pools and explicit PostgreSQL connection/shutdown conditions emit `freight_database_unavailable` with `reason=connection_unavailable`; database budgets keep their existing `freight_database_budget_exhausted` reasons. These warnings include no SQL, credentials or original payloads. Protocol/decode/integrity failures retain ordinary storage-error handling. An unavailable response does not prove an interrupted mutation rolled back; retain the original key/body and resolve its receipt before another submission.

During shutdown, readiness and new business/webhook requests return 503 with `Retry-After: 1`; admitted work keeps its existing bounded drain behavior. Liveness and authenticated metrics remain available on connections that can still be served. The HTTP listener itself stops accepting connections during graceful shutdown, so a scrape is not guaranteed after SIGTERM.

Use readiness for traffic admission and liveness for process restart decisions. The following container fragment follows the [Kubernetes probe model](https://kubernetes.io/docs/tasks/configure-pod-container/configure-liveness-readiness-startup-probes/); adjust startup allowance to measured migrations and environment startup time:

```yaml
# Under the API container:
startupProbe:
  httpGet: { path: /live, port: 8090 }
  periodSeconds: 5
  timeoutSeconds: 2
  failureThreshold: 60
livenessProbe:
  httpGet: { path: /live, port: 8090 }
  periodSeconds: 10
  timeoutSeconds: 2
  failureThreshold: 3
readinessProbe:
  httpGet: { path: /ready, port: 8090 }
  periodSeconds: 5
  timeoutSeconds: 2
  failureThreshold: 2
```

Set the Pod's `terminationGracePeriodSeconds` above `FREIGHT_SHUTDOWN_TIMEOUT_MS` plus any pre-stop hook duration. Existing Compose allows 310 seconds. The probe fragment is an example, not an applied or qualified Kubernetes deployment.

## Protected metrics

Set `FREIGHT_METRICS_TOKEN` to an independently generated secret of 32–512 visible ASCII characters on the API. Missing/empty configuration disables collection and returns 404 from `/metrics`. Invalid nonempty configuration or reuse of a configured business API credential fails service startup without echoing its contents. Compose forwards this variable only to the API.

Scrape `GET /metrics` with exactly one `Authorization: Bearer …` header. Missing, duplicate or incorrect authorization returns 401. Query parameters and staff/partner credentials do not substitute for the dedicated token. Keep this credential distinct from every business API key: metrics summarize the whole process across tenants. They must not be exposed through the tenant-facing web proxy. Use a private monitoring network or HTTPS ingress; the API listener itself does not terminate TLS. All metrics responses have `Cache-Control: no-store`.

Example [Prometheus scrape configuration](https://prometheus.io/docs/prometheus/latest/configuration/configuration/), using a mounted token file rather than a credential in source:

```yaml
scrape_configs:
  - job_name: stateset-freight
    scrape_interval: 15s
    scrape_timeout: 5s
    metrics_path: /metrics
    authorization:
      type: Bearer
      credentials_file: /run/secrets/freight-metrics-token
    static_configs:
      - targets: [api:8090] # Private Compose network example; discover each replica separately.
```

The target must be reachable from Prometheus; no Prometheus service is started by this change. Rotate by deploying a new API token and updating the scraper's secret together. Collection and counters start fresh for each process. Scrape individual replicas rather than a load-balanced service address to preserve counter continuity.

| Metric                                           | Type      | Interpretation                                                                |
| ------------------------------------------------ | --------- | ----------------------------------------------------------------------------- |
| `freight_process_uptime_seconds`                 | Gauge     | Time since monitor initialization.                                            |
| `freight_process_draining`                       | Gauge     | 1 after shutdown is requested, otherwise 0.                                   |
| `freight_http_in_flight`                         | Gauge     | Instrumented requests still awaiting response headers.                        |
| `freight_http_requests_total`                    | Counter   | Requests ending in response headers or cancellation.                          |
| `freight_http_response_headers_duration_seconds` | Histogram | Time until headers or cancellation; includes authentication and handler work. |

## Storage and admission gauges

An authorized scrape also reports the current local admission and SQLx pool state. Collecting these gauges issues no SQL, acquires no pool connection and never waits for an admission registry. The dedicated token protects process-wide values across tenants; no tenant, shipment, database URL or schema is a label.

| Metric                                         | Meaning                                                                        |
| ---------------------------------------------- | ------------------------------------------------------------------------------ |
| `freight_admission_pending{lane}`              | Waiting requests for `snapshot`, `catalog` and configured `workspace` lanes.   |
| `freight_admission_pending_limit{lane}`        | Global waiting limit for that lane.                                            |
| `freight_admission_tenant_pending_limit{lane}` | Per-tenant waiting limit, without tenant labels.                               |
| `freight_admission_available_permits{lane}`    | Unleased permits for the lane.                                                 |
| `freight_admission_closed{lane}`               | Whether the local semaphore is closed.                                         |
| `freight_admission_snapshot_available{lane}`   | Whether the nonblocking registry observation succeeded; also includes `write`. |
| `freight_workspace_admission_configured`       | Whether the optional workspace processing limit is configured.                 |
| `freight_write_admission_requests`             | Write reservations, including both active owners and waiters.                  |
| `freight_write_admission_limit`                | Global active-plus-waiting write limit.                                        |
| `freight_write_admission_tenant_limit`         | Per-tenant active-plus-waiting write limit.                                    |
| `freight_database_pool_configured`             | Whether this Store uses PostgreSQL.                                            |
| `freight_database_pool_connections`            | SQLx pool connection count.                                                    |
| `freight_database_pool_idle`                   | SQLx idle connection count.                                                    |
| `freight_database_pool_limit`                  | Configured maximum pool connections.                                           |
| `freight_database_pool_closed`                 | Whether the local pool has been closed.                                        |

All 15 added families are gauges. If a registry is busy or poisoned, its snapshot availability is zero and its request-count sample is omitted. An unavailable observation must not be converted to zero queued work. An unconfigured workspace lane or memory-backed Store omits its corresponding numerical lane/pool samples and reports its configured gauge as zero. Limits follow the process configuration; they are not automatically production sizing recommendations.

Samples are independent rather than an atomic snapshot. Available permits measure ownership, including work retained after caller cancellation, rather than CPU use or retained bytes. Write reservations remain counted until actual completion or rollback. Pool values describe the local SQLx object: `closed=0` does not establish that the remote database is healthy. Readiness remains the dependency check.

## HTTP series and interpretation

Request series have only `route`, `method`, and `outcome` labels. Routes are registered templates (such as `/v1/documents/{id}/content`); unknown paths collapse to `unmatched`, nonstandard methods to `OTHER`. Outcomes are status classes `1xx`–`5xx`, `other`, or `cancelled`. No credentials, tenant IDs, shipment IDs, actor IDs, query strings or payloads become metric labels. HTTP trace spans also use route templates and bounded method labels. Application-specific logs retain their existing behavior.

All four diagnostic routes are excluded from request counts and latency histograms so probes and scrapes do not make business traffic appear healthier. Authentication denials, unknown paths and drain rejections on other routes are included. Cancellation means the handler future was dropped before response headers; it does not prove whether a business mutation committed. Body streaming and downstream network delivery are not measured. A header-level 2xx is not proof that the client received the complete response.

Example diagnostic queries, with thresholds to be set from the agreed workload and operating targets:

```promql
# Response-header p95 by route for successful responses.
histogram_quantile(0.95,
  sum by (le, route) (
    rate(freight_http_response_headers_duration_seconds_bucket{outcome="2xx"}[5m])
  )
)

# Server failures and cancellations must both remain visible.
sum by (outcome) (
  rate(freight_http_requests_total{outcome=~"5xx|cancelled"}[5m])
)

# Scrape availability (Prometheus generates this metric).
up{job="stateset-freight"}
```

Zero traffic, absent series, a failed scrape, request rejection and actual service failure are distinct states. Do not substitute a zero series for missing monitoring. Metrics do not yet measure partner queue age, successful external delivery, browser task completion, body-transfer failures or end-to-end operator outcomes.

## Reproduce the checks

```bash
cargo test --offline --locked --lib observability
cargo test --offline --locked --lib lifecycle
cargo run --offline --locked --example observability_fixture \
  > /tmp/freight-metrics.prom 2> /tmp/freight-metrics-trace.log
python3 scripts/check-observability-scrape.py \
  --scrape /tmp/freight-metrics.prom --trace /tmp/freight-metrics-trace.log
cargo build --offline --locked
python3 scripts/qualify-observability.py \
  --binary target/debug/stateset-freight --output /tmp/freight-process-check.json
```

The Rust tests exercise actual in-process Axum routes, dedicated credential separation, disabled export, bounded label cardinality, 32 completions across four runtime threads, cancellation cleanup, cumulative buckets, drain rejection and readiness deadlines. They need neither a network listener nor Docker. The fixture emits the actual scrape after success, denied access and shutdown rejection; its checker requires the qualification-only Python `prometheus-client` package (the exact version is retained in the parser receipt).

The process harness requires permission to bind loopback TCP ports. It runs only the supplied local binary, creates an owned empty dotenv directory, supplies ephemeral synthetic credentials, and inherits no database, identity, integration or proxy configuration. It preserves logs and a receipt, rejects existing output files, and terminates only its own child process. Database health/outage checks and real container/Kubernetes operation require separate qualification in an environment that permits them.

## Alert rules

[`monitoring/freight-alerts.yml`](../monitoring/freight-alerts.yml) is a Prometheus rule file built only on the metric families above and the scrape `up` series. Load it with `rule_files` and route `severity="page"` to the on-call pager and `severity="ticket"` to a working-hours queue. The thresholds are starting points, not qualified production SLOs; tune them after a capacity profile at the target volume. `scripts/test_alert_rules.py` checks that every rule uses only exported series and links to a heading below.

Scrape each replica as job `stateset-freight`, as in the configuration above. `FreightNotReady` also needs a [blackbox exporter](https://github.com/prometheus/blackbox_exporter) HTTP probe of `GET /ready` as job `stateset-freight-ready`; without it that one rule never fires. Readiness needs no credential, so the probe carries none:

```yaml
scrape_configs:
  - job_name: stateset-freight-ready
    metrics_path: /probe
    params: { module: [http_2xx] }
    static_configs:
      - targets: [http://api:8090/ready]
    relabel_configs:
      - { source_labels: [__address__], target_label: __param_target }
      - { source_labels: [__param_target], target_label: instance }
      - { target_label: __address__, replacement: blackbox:9115 }
```

Freight logs database outages as `freight_database_unavailable` and write rejections as `freight_write_admission_rejected`; these are log events, not metrics. Database outages reach the rules as 503s in `FreightServerErrors`, a failed readiness probe and pool gauges.

### Availability

#### FreightScrapeDown

Prometheus cannot read `/metrics`. Check that the process is running, that the scraper reaches the private monitoring address, and that its token file matches `FREIGHT_METRICS_TOKEN`. A 401 in the target's last error means the token rotated on one side only.

#### FreightNotReady

`GET /ready` fails. Compare it with `GET /live`: a live process that is not ready usually has an unavailable database, an incompatible storage format, a slow storage check or a requested shutdown. Check database reachability and recent migrations before restarting.

#### FreightServerErrors

More than 5% of requests return 5xx. Group `freight_http_requests_total{outcome="5xx"}` by `route` to find the failing routes, then search the logs for `freight_database_unavailable` and `freight_database_budget_exhausted`. Treat interrupted writes as unresolved until their original receipts are checked.

#### FreightSlowResponses

The 95th-percentile time to response headers is above 2.5 seconds. Check `FreightDatabasePoolExhausted` and the admission alerts first, then break the histogram down by `route`. Document rendering and large exports are expected to be slower than ordinary reads.

#### FreightDrainStuck

Shutdown was requested 10 minutes ago and the process is still draining. Admitted work is outliving `FREIGHT_SHUTDOWN_TIMEOUT_MS`, or the orchestrator's grace period is longer than intended. Check for held database locks before forcing termination.

#### FreightRestartLoop

The process restarted at least three times in 30 minutes. Read the startup logs: invalid or reused credentials, an unreachable database and an incompatible storage format all stop startup.

### Admission

#### FreightAdmissionBacklog

A read or work lane's waiting queue is above 80% of its global limit. Check whether one route or tenant dominates the traffic, whether the database pool is saturated, and whether the configured limits match the deployment's capacity profile.

#### FreightAdmissionQueueFull

A lane's waiting queue is at its limit, so that lane is rejecting new requests. Handle it as `FreightAdmissionBacklog`, with users already affected.

#### FreightWriteAdmissionSaturated

Active and waiting writes are above 90% of the global write limit. Look for long-running writes or row-lock contention in PostgreSQL. Rejected writes log `freight_write_admission_rejected` with `queue_full` or `queue_timeout`.

#### FreightAdmissionLaneClosed

A lane's local semaphore closed outside shutdown. The lane admits nothing until the process restarts. Capture logs, then restart the replica.

#### FreightAdmissionUnobservable

Scrapes have not obtained the admission registry for 10 minutes, so the lane's queue gauges are missing and backlog alerts cannot fire for it. A persistently busy or poisoned registry needs a log review and a replica restart.

### Database

#### FreightDatabasePoolExhausted

Every configured PostgreSQL connection is leased and none is idle. Check PostgreSQL for long transactions and lock waits, and compare the pool limit with the database's connection budget before raising it.

#### FreightDatabasePoolClosed

The local PostgreSQL pool closed outside shutdown, so storage requests fail. Capture logs, then restart the replica. Readiness also fails while the pool is closed.
