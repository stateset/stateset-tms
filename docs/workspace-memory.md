# Workspace memory attribution and ownership trial

The current [derived-index admission release](derived-index-admission.md), source `6b520248`, passes 610 application scenarios, 71 separate image recovery checks and its declared invoice-rich profile. It combines direct response serialization, a separate catalog queue and coordination of cold index builders with writes. This page retains both OOM observations and the unpromoted five-array ownership trial; its instrumentation and timings retain their original scope.

## What was observed

The [process sampler](../scripts/profile-api-workspace-memory.py) reads only the owned API container's PID1 executable identity, memory status, resident mappings and task IDs. It retains each raw source and checks required counters rather than treating missing data as zero. Existing debug logs identify read/write phases and Rust worker IDs without logging request contents or credentials. Five negative-control methods cover identity, framing, missing/ambiguous counters, virtual-versus-resident memory, and task churn.

Both profiles retain the exact financial fixtures, fixed arrival times, one-job work/read budget, two Tokio threads, two database connections and API/PostgreSQL limits of 512/768 MiB with two CPUs each and no swap or allocator tuning. Instrumentation adds procfs reads and debug logging. The baseline also overlaps the experimental native build on the shared host. These runs are **not a controlled latency or CPU comparison** with each other or the original uninstrumented failure.

| Observation                                               | Qualified source with instrumentation | Isolated ownership candidate |
| --------------------------------------------------------- | ------------------------------------: | ---------------------------: |
| API terminal result                                       |                         OOM, exit 137 |                OOM, exit 137 |
| Valid raw process snapshots                               |                                    91 |                           96 |
| Minimum resident memory in the sampled warm-idle interval |                             83.70 MiB |                    74.48 MiB |
| Highest sampled process RSS                               |                            423.26 MiB |                   498.04 MiB |
| Highest sampled kernel cgroup peak                        |                            438.70 MiB |                   501.59 MiB |
| Observed process thread counts                            |                                   3–5 |                          3–5 |
| Complete accepted scheduled large reads                   |                                     5 |                            7 |
| Small catalog busy responses before the kill              |                                     3 |                            2 |
| Committed observations / proven unapplied observation     |                                 1 / 1 |                        2 / 1 |

The kernel/Docker terminal OOM evidence establishes failure even when the last available sample is below the 512 MiB limit. The two APIs are first observed exited about 11.90 and 14.01 seconds into their scheduled traffic, respectively. The raw samples show resident anonymous memory falling during idle periods and rising during processing. That narrows the investigation toward working peaks and ownership across jobs; it does not prove absence of leaks or identify live allocator bytes, allocation stacks, individual arena ownership or causal shares.

Large anonymous writable mappings dominate the high-RSS snapshots. Existing phase logs show worker reuse and replacement, including different workers after the 35-second idle. Rust `ThreadId` values are not Linux task IDs; the report does not invent a per-thread heap association from those two namespaces.

## What the trial changes

The response path in this historical trial materializes a typed workspace, constructs a full `serde_json::Value` response, encodes it into a byte vector, drops the projected tree, and returns the encoded bytes to HTTP. Prepared load, invoice, bill, settlement and quote arrays pass through `json!`, which serializes their values by reference and recursively clones them into the response object.

The candidate uses a new native executable on the pinned preceding runtime image. It is experimental packaging, not a build of the unchanged production Dockerfile. It has no complete new application/browser/recovery qualification and changes no dependency, codec, schema, deployment or default. Its core source is `80e18d26`, with all 372 frozen inputs and the exact patch retained. It remains separate from the workspace's qualified application.

## Next engineering gate

The subsequent [direct-serialization candidate](direct-workspace-json.md) avoids the passthrough JSON trees and survives the original financial workload, while still failing the declared catalog-availability requirement. Its mixed result is preserved; at that phase independent bounded catalog admission was the next gate. Neither historical candidate was promoted as a complete release.

Removing those five copies is insufficient. The complete response still builds large JSON trees for retained histories and finance while the typed workspace is live. The processing lease also ends before the encoded HTTP buffer's complete lifetime; returned workspaces and other stores/replicas remain outside that boundary. These are confirmed code boundaries, not isolated causal diagnoses of the measured OOM.