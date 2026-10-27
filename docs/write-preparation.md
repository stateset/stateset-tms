# Write preparation investigation

The CPU and latency comparisons below retain their exact preceding source fingerprints, including the `4118f80d` normalization release. The later [shared workspace admission](workspace-admission.md) release qualifies 600 application scenarios, a finite mixed memory profile and 71 separate TLS/SCRAM recovery checks; it does not rerun these matched CPU comparisons or relabel the original-baseline gate.

The matched current-source `7c39048f` → `4118f80d` profile uses actual optimized APIs and PostgreSQL 16.15, baseline/candidate/candidate/baseline order, ten timed saves per variant and size, and phase logging off. The separately declared 15% CPU target is met at 1,000 shipments:

| Fixture         |              Median save CPU | Median save wall time | API process peak memory |
| --------------- | ---------------------------: | --------------------: | ----------------------: |
| 100 shipments   |              0.060 → 0.060 s |    104.23 → 104.13 ms |       48.78 → 49.45 MiB |
| 1,000 shipments | 0.830 → 0.670 s (**−19.3%**) |  1,106.63 → 912.93 ms |     374.35 → 332.10 MiB |

At 1,000 shipments, complete/deferred workspace reads change from 951.15/831.37 to 857.63/714.43 ms, catalog from 2.12 to 2.44 ms, history from 5.53 to 5.46 ms and replay from 9.31 to 4.10 ms. At 100 shipments, complete/deferred reads worsen from 73.85/61.98 to 85.41/66.58 ms, catalog from 1.61 to 2.11 ms and history from 3.89 to 4.92 ms; replay changes from 5.04 to 4.51 ms. Every individual sample, first read, original snapshot and unfavorable result is preserved. These finite shared-host observations have no resource quotas or sustained arrivals and establish no production capacity or latency/memory bound. The earlier `166789e` baseline is not reevaluated or relabelled.

All eight native runs preserve compared business responses, save versions/locations, exact original-response retries and unchanged physical replay tuples. Complete codec tests, 24 native compatibility pairs, 44 configurations, 34 frame controls and five invalid-source controls pass. The full image drill retains original finance, approvals, receipts, queues, files, wire identities and signed saves through real cluster/EDI-volume loss. Test setup failures remain separately recorded; included component/recovery cases are not added as independent totals.

## Preceding unpromoted trials

Each comparison uses actual optimized HTTP APIs and PostgreSQL 16.15, with baseline/candidate/candidate/baseline ordering at 100 and 1,000 shipments. Phase logging is off. The first two trials retain six timed saves per variant and size; the third retains ten. API CPU includes process user and system time. These synthetic fixtures run on a shared host without resource quotas or sustained arrivals. Compare each candidate with its own paired baseline; the absolute timings across trials are not a ranking.

| Candidate                                                | 1,000-shipment save CPU, baseline → candidate | CPU reduction |   Save wall time | Result       |
| -------------------------------------------------------- | --------------------------------------------: | ------------: | ---------------: | ------------ |
| Borrowed record ID reader                                |                               0.740 → 0.770 s |         −4.1% |   971 → 1,013 ms | Not promoted |
| Borrowed canonical object keys                           |                               0.825 → 0.710 s |         13.9% |   1,093 → 975 ms | Not promoted |
| Borrowed keys, root-map reuse and earlier buffer release |                               0.785 → 0.745 s |          5.1% | 1,042 → 1,038 ms | Not promoted |

The third candidate also increases median observed API process peak memory from **382.45 to 431.55 MiB**. At 100 shipments its save CPU is unchanged, save wall time rises from 127.45 to 129.95 ms, and replay rises from 4.89 to 5.51 ms. The second candidate improves the measured large-workspace reads and replay, but smaller full/deferred reads and process peak memory worsen. Every trial retains all read, replay, memory and individual timing samples, including unfavorable observations. These findings do not identify every cause of shared-host variance or establish a memory bound.

All eight native runs in each trial pass business-response comparisons, save version/location checks, exact original-response retries and unchanged physical tuples on replay. The save harness separately guards four unrelated collections and retains complete before/after snapshots. All owned profiling schemas are removed. The third candidate separately passes 323 active Rust tests, 91 actual PostgreSQL tests and strict workspace/all-target Clippy; its remaining application, image and recovery qualification was not rerun because it was not accepted. Those tests do not turn a failed performance target into a passed release.

A separate logging-enabled diagnostic attributes median preparation time of 236.91 ms: encoding 44.41 ms, retained-JSON reconciliation 114.32 ms, packing 46.84 ms and delta construction 27.28 ms. Logging overhead remains included. These stages cover only part of the actual save; frame validation, typed decoding, projection, original parsing and prior-block digests also cost CPU. The diagnostic is not a matched speed comparison.

The evidence preserves six complete raw artifact archives, their file hashes, exact candidate core/test/example snapshots, corrected setup failures, compiler/build bindings and the assessments. The borrowed-key codec also retains all 26 component tests and exact compatibility over 24 native pairs, 44 configurations, 34 frame controls and five invalid-source controls. No normalization, duplicate-key, numeric, partition, receipt, transaction or format guard was relaxed to reach a timing result.

These comparisons use the current `7c39048f` baseline. They do not reevaluate the earlier `166789e` baseline or relabel its unmet 15% gate. Production capacity, resource limits and operating objectives still require a defined deployment workload and separate qualification.
