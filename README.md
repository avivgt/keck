# Keck

Accurate per-workload power metering for bare metal, VMs, and Kubernetes.

Named after the [W. M. Keck Observatory](https://www.keckobservatory.org/) and
built as a successor to [Kepler](https://github.com/sustainable-computing-io/kepler),
Keck measures energy consumption at every level — from individual CPU cores to
fleet-wide ESG reports — with hardware-grounded accuracy and transparent error bounds.

## Why Keck?

Existing tools attribute power using a single signal:

```
process_energy = node_energy × (process_cpu_time / total_cpu_time)
```

This is inaccurate. Two processes with the same CPU time can consume 3× different
power depending on frequency, workload type, and memory behavior. Keck fixes this
with **bottom-up attribution**: per-core energy, weighted by multiple hardware signals,
reconciled against PSU ground truth.

### What makes Keck different

| | Kepler | Keck |
|---|---|---|
| **Attribution** | Node-level CPU time ratio | Per-core, frequency-weighted, multi-signal |
| **Data source** | `/proc/[pid]/stat` polling | eBPF `sched_switch` tracepoint (in-kernel) |
| **Frequency awareness** | None | Per-core time-at-frequency tracking |
| **Hardware counters** | None | Instructions, cycles, LLC misses per core |
| **Accuracy validation** | None | PSU reconciliation with error bounds |
| **Architecture** | Passive exporter (scrape) | Active agent (aggregate, push, query) |
| **VM support** | Limited | Host/guest agent coordination |
| **Scale** | Per-node only | Node → Cluster → Fleet |
| **Language** | Go | Rust (no GC, eBPF via Aya) |

## Architecture

```
Layer 4:  keck-fleet         Fleet manager (multi-cluster governance)
              |
Layer 3:  keck-controller    Cluster controller (aggregation, scheduler, carbon)
              | gRPC
Layer 2:  keck-agent         Attribution engine, K8s enrichment, store, outputs
              | BPF maps
Layer 1:  keck-ebpf          Kernel programs (sched_switch, cpu_frequency)
              | sysfs/MSR
Layer 0:  keck-agent          Hardware readers (RAPL, hwmon, GPU, Redfish)
              |
          keck-common         Shared types (no_std, eBPF + userspace)
```

### Node Agent (keck-agent)

Runs as a DaemonSet. Collects and attributes power on each node.

**Layer 0 — Hardware signals:**
- RAPL energy counters (per-socket CPU + DRAM)
- hwmon power sensors (direct electrical measurements)
- GPU power (NVIDIA NVML, AMD ROCm)
- Platform power via Redfish/IPMI (PSU ground truth)
- Tiered polling: fast (100ms), medium (500ms), slow (3s), heartbeat (5s)
- Reconciliation: `Σ(components) vs PSU_input → error_ratio`

**Layer 1 — Kernel observation (eBPF):**
- `sched_switch` tracepoint: per-PID per-core CPU time (nanosecond precision)
- `cpu_frequency` tracepoint: per-core time-at-frequency tracking
- `perf_event_open`: per-core hardware counters (instructions, cycles, LLC misses)
- `cgroup_id` capture: pid → container mapping without `/proc` reads

**Layer 2 — Attribution engine:**
- Splits socket RAPL energy to per-core using frequency-weighted model
- Three attribution models (auto-selected by available data):
  - **FullModel**: time × freq² × (1 + α·IPC + β·cache_miss_rate)
  - **FrequencyWeighted**: time × freq²
  - **CpuTimeRatio**: time only (Kepler-equivalent fallback)
- Normalization: `Σ(process_energy) = core_energy` (energy conservation guaranteed)
- Memory attribution via LLC miss ratio
- Aggregation: process → container → pod → namespace
- Local ring buffer store with drill-down query API

### Cluster Controller (keck-controller)

Runs as a single Deployment. Aggregates power data across all nodes.

- Receives pod-level summaries from node agents via gRPC
- Aggregates: pod → namespace → cluster
- Carbon intensity integration (Electricity Maps, WattTime, or static)
- Cost calculation: energy × $/kWh (configurable per region)
- K8s custom metrics API (enables HPA scaling on power)
- **Power-aware scheduler extender:**
  - Filter: reject pods that would exceed namespace power budget
  - Prioritize: score nodes by power headroom and metering accuracy
  - Strategies: BinPack (reduce idle waste) or Spread (avoid hotspots)

### Fleet Manager (keck-fleet)

Runs standalone. Multi-cluster observability and governance.

- Unified fleet dashboard: power, carbon, cost per cluster/team
- Team views: namespace → team mapping across clusters
- Policy engine: power budgets, carbon budgets, metering quality, staleness alerts
- Carbon-aware routing: recommend lowest-carbon cluster for new workloads
- ESG reporting: daily/monthly reports with energy (kWh), carbon (kgCO2), cost

## Agent Profiles

Keck adapts to deployment size — from edge nodes to large datacenters:

| Aspect | Minimal | Standard | Full |
|--------|---------|----------|------|
| Memory | ~10MB | ~50MB | ~200MB |
| eBPF map size | 1K PIDs | 10K PIDs | 100K PIDs |
| Fast poll | 1s | 500ms | 100ms |
| Report upstream | Pod-level, 60s | Pod-level, 10s | Process-level, 5s |
| Attribution model | CpuTimeRatio | FrequencyWeighted | FullModel |

The agent self-monitors and automatically downgrades its profile if it
exceeds its resource budget.

## Zoom Model

Full detail never leaves the node unless requested. Drill down on demand:

```
Fleet (45kW total)
  └── Cluster "prod-east" (18kW)
        └── Namespace "ml-training" (4.2kW)
              └── Pod "trainer-7" (380W)
                    └── Container "train" (340W: GPU 280W, CPU 55W, Mem 5W)
                          └── PID 4521: python train.py
                                ├── Core 12: 22W (3.4GHz, 1.2B insn, 40K LLC miss)
                                └── Core 13: 18W (3.1GHz, 0.9B insn, 12K LLC miss)
```

## Project Structure

```
keck/
├── keck-common/       Shared types (no_std, works in kernel + userspace)
├── keck-ebpf/         eBPF programs (sched_switch, cpu_frequency)
├── keck-agent/        Node agent
│   └── src/
│       ├── hardware/    Layer 0: RAPL, hwmon, GPU, Redfish, tiered collector
│       ├── ebpf/        Layer 1: eBPF loader, map drainer, perf counters
│       ├── attribution/ Layer 2: models, engine, types
│       ├── k8s/         Layer 2: cgroup → container → pod enrichment
│       ├── store/       Layer 2: ring buffer with outbox
│       └── output/      Layer 2: Prometheus, query API
├── keck-controller/   Cluster controller
│   └── src/
│       ├── aggregator/  Cluster-wide state
│       ├── api/         gRPC + REST + K8s custom metrics
│       ├── carbon/      Carbon intensity tracking
│       └── scheduler/   Power-aware scheduler extender
└── keck-fleet/        Fleet manager
    └── src/
        ├── registry/    Multi-cluster state
        ├── api/         gRPC + REST
        ├── policy/      Budget and carbon policy engine
        └── reporting/   ESG report generation
```

## Building

Requires:
- Rust nightly (for eBPF target)
- Linux (eBPF programs target the Linux kernel)

```bash
# Build agent (includes eBPF compilation)
cargo build -p keck-agent

# Build cluster controller
cargo build -p keck-controller

# Build fleet manager
cargo build -p keck-fleet
```

## Status

**Early development.** The architecture and core algorithms are implemented
(~6,300 lines of Rust). The following integrations need to be completed
before first deployment:

- [ ] GPU power reading (NVIDIA NVML)
- [ ] Redfish/IPMI HTTP client
- [ ] Kubelet API client for cgroup resolution
- [ ] gRPC wiring between agent ↔ controller ↔ fleet
- [ ] REST API servers (axum)
- [ ] Prometheus metric registration
- [ ] Kubernetes manifests (Helm chart, DaemonSet, Deployment)
- [ ] Linux build validation and testing
- [ ] Benchmark: agent overhead vs Kepler

## License

Apache-2.0
