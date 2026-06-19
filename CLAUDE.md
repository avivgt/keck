# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Keck

Per-workload power metering for Kubernetes. Measures energy at every level (CPU core to fleet) using hardware signals (RAPL, hwmon, Redfish, DCGM) and eBPF, with bottom-up attribution that weights by frequency and hardware counters instead of flat CPU time ratios.

Deployed on OpenShift via OLM operator. The UI is an OpenShift Dynamic Console Plugin.

## Build Commands

```bash
# Rust crates (requires nightly for eBPF target)
cargo build -p keck-agent        # includes eBPF compilation via aya-build
cargo build -p keck-controller
cargo build -p keck-fleet
cargo test --workspace            # all Rust tests
cargo test -p keck-controller     # single crate
cargo test -p keck-controller -- tests::test_healthz  # single test

# Go operator
cd keck-operator && go build -o bin/manager cmd/main.go
cd keck-operator && go test ./...
cd keck-operator && go test ./controllers/ -run TestKeckClusterReconcile  # single test

# UI (OpenShift console plugin)
cd keck-ui && npm install && npm run build
cd keck-ui && npm run dev          # webpack dev server
cd keck-ui && npm run lint

# Release (builds + pushes operator, bundle, catalog images to quay.io)
./scripts/release.sh 0.1.0

# Operator deployment
cd keck-operator && make install   # CRDs only
cd keck-operator && make deploy    # CRDs + RBAC + manager
cd keck-operator && make run       # run operator locally
```

## Architecture

Four layers, three languages:

**Rust crates** (workspace at root, nightly toolchain, edition 2024):
- `keck-ebpf` -- BPF programs compiled to `bpfel-unknown-none`. Built automatically by `keck-agent/build.rs` via aya-build. Not in the workspace members list (separate target).
- `keck-common` -- shared types with `no_std` support. Feature-gated: `userspace` enables aya Pod impls.
- `keck-agent` -- DaemonSet, one per node. Reads hardware power (Layer 0), drains eBPF maps (Layer 1), runs attribution engine (Layer 2), POSTs reports to controller.
- `keck-controller` -- single Deployment. Receives agent reports via REST, aggregates to cluster/namespace/node views, serves the UI API. Axum-based, tokio async.
- `keck-fleet` -- standalone fleet manager (multi-cluster). gRPC + REST, tonic-based. Not yet deployed.

**Go operator** (`keck-operator/`):
- CRDs: `KeckCluster` (cluster-scoped), `PowerBudget` (namespaced), `PowerProfile` (cluster-scoped).
- Reconciles KeckCluster into DaemonSet (agent) + Deployment (controller) + RBAC + Services.
- OLM bundle at `keck-operator/bundle/`.

**TypeScript UI** (`keck-ui/`):
- OpenShift Dynamic Console Plugin using PatternFly 5 + React 17.
- Tabs: Cluster Overview, Namespaces, Nodes, Pods, Applications.
- Talks to keck-controller REST API. Shared polling via `usePolling` hook.

## Data Flow

```
Agent (per node)                    Controller (per cluster)         UI
  Hardware readers (RAPL/hwmon/     <-- POST /api/v1/report --+
    Redfish/DCGM)                                             |
  eBPF (sched_switch, cpu_freq)     Aggregator (RwLock)       |
  Attribution engine                  pods: HashMap<uid, PodState>
  /proc scanning                      nodes: HashMap<name, NodeState>
                                    REST API (axum)
                                      GET /api/v1/cluster     --> ClusterOverview
                                      GET /api/v1/namespaces  --> NamespaceView
                                      GET /api/v1/nodes       --> NodesView
                                      GET /api/v1/applications --> ApplicationsView
```

## Key Design Decisions

**Power units**: everything internal is microwatts (u64). API responses convert to watts (f64 / 1e6).

**Source priority**: Measured (Redfish) > Estimated (RAPL) > Derived. The agent discovers all sources and auto-selects the best per component (CPU, Memory, GPU, Platform). When RAPL is the CPU source, eBPF frequency-weighted attribution kicks in. When Redfish is available, flat CPU time ratios are used (eBPF doesn't improve measured data).

**Attribution models** (auto-selected by available data):
- FullModel: `time x freq^2 x (1 + alpha*IPC + beta*cache_miss_rate)` -- requires per-PID hardware counters from eBPF
- FrequencyWeighted: `time x freq^2` -- requires eBPF cpu_frequency tracepoint
- CpuTimeRatio: `time` only -- /proc fallback

**Memory attribution**: 60% PSS (static DRAM) + 40% LLC miss ratio (dynamic DRAM access). PSS is cached and refreshed every 5 cycles because smaps_rollup is expensive.

**Classification** (controller-side, not agent): Pods are categorized by namespace lookup against ClusterOperators (platform), OLM Subscriptions (operator), or default (application). Classification runs in a separate background task with its own RwLock, decoupled from the aggregator.

**Normalization**: energy conservation is guaranteed. Sum of all process attributions equals the measured core/socket energy.

## Environment Variables

Agent: `KECK_CONTROLLER_URL`, `KECK_INTERVAL_SECS`, `KECK_API_KEY`, `KECK_AUTO_TUNE`, `KECK_ALPHA`, `KECK_BETA`, `KECK_CAPTURED_LABELS`, `DCGM_EXPORTER_URL`, `NODE_NAME`.

Controller: `KECK_API_KEY`, `KECK_CORS_ORIGIN`.

## Testing Patterns

Rust tests use `#[cfg(test)]` modules within each source file. The controller tests use axum's `tower::ServiceExt::oneshot` for HTTP handler testing with a `make_state()` / `make_agent_report()` factory pattern.

Go operator tests use the standard `testing` package with controller-runtime's `fake` client.

There are 139+ tests across all components. No external test infrastructure is required -- all tests run with `cargo test` / `go test`.
