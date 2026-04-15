# How Keck Calculates Per-Pod Power

This document explains every formula Keck uses to attribute power to
individual pods. Each section covers what we measure, why, and how.

## The problem

A server consumes 500W total. It runs 80 pods. Which pod is responsible
for how much power?

Hardware only measures power at the component level: total CPU, total
DRAM, total PSU. There is no per-pod power meter. We must estimate
each pod's share using observable signals.

## 1. CPU power attribution

### What we measure

For each process, on each CPU core, every 10 seconds:

- **Time**: how many nanoseconds it ran on that core (from eBPF
  `sched_switch` tracepoint — the kernel fires this every time one
  process replaces another on a core)
- **Frequency**: what clock speed the core was running at (from eBPF
  `cpu_frequency` tracepoint — fires when the core changes speed)
- **IPC**: instructions per cycle — how many instructions the CPU
  completed per clock tick (from hardware performance counters)
- **Cache miss ratio**: fraction of instructions that caused a
  last-level cache miss, triggering a slow DRAM access (from hardware
  performance counters)

### The formula

```
weight = time × freq² × (1 + α × IPC + β × cache_miss_ratio)
```

Then each process's share of core energy:

```
process_power = core_power × (process_weight / sum_of_all_weights_on_core)
```

### Why each term exists

**time** — The longer a process runs, the more power it uses. This is
the most basic signal. If you only had this, you'd get the same
result as Kepler and other tools.

**freq²** — CPU power scales with the square of frequency. This comes
from CMOS circuit physics: P = C × V² × f. When frequency increases,
voltage also increases (roughly linearly), so power scales as f × V²
≈ f³ in theory, but f² is a better empirical fit because voltage
doesn't scale perfectly. A process running at 3.5 GHz uses roughly
3× more power than one at 1.2 GHz for the same duration.

**1** — The baseline. Even a completely stalled process (IPC = 0, no
cache misses) still consumes power. The core is clocked, transistors
maintain state, leakage current flows. The "1" ensures a stalled
process still gets attributed a baseline share of core power.

**α × IPC** — IPC measures compute density. A process retiring 3
instructions per cycle (dense matrix math) switches more transistors
per tick than one retiring 0.5 instructions per cycle (waiting on
memory). More switching = more dynamic power. The α coefficient
controls how strongly this affects the power split. At α = 0.3,
a 6× difference in IPC produces a ~1.6× difference in attributed
power.

**β × cache_miss_ratio** — Every cache miss triggers a DRAM access
through the memory controller, bus drivers, and DRAM chips. This
activity costs power beyond what the core itself uses. A process with
many cache misses (streaming large datasets, random memory access)
causes more system-wide power consumption. At β = 1.5 with a typical
miss ratio of 0.01-0.05, this adds a 1.5-7.5% power adjustment.

### Why not just use CPU time?

Two processes with identical CPU time can consume 2-3× different
power depending on:

- **Frequency**: one might run during a turbo boost period (4.0 GHz),
  the other during thermal throttling (1.5 GHz)
- **Instruction mix**: AVX-512 vector math draws 2-4× more current
  than simple integer operations
- **Memory behavior**: a process that hits L1 cache every time uses
  less total system power than one that misses to DRAM on every access

CPU-time-only attribution (what Kepler uses) treats both equally.
Keck's model captures these differences.

### Energy conservation guarantee

After computing weights, we normalize so that:

```
sum of all process power on a core = core power from RAPL
```

This means no energy is created or lost in the attribution. The
total always matches the hardware measurement. The model only
affects how that total is *distributed* among processes.

### Default coefficients

- **α = 0.3** (IPC weight): produces a moderate adjustment. IPC
  ranges from 0.2 to 4.0 on x86, so α × IPC ranges from 0.06 to
  1.2. This means IPC can increase a process's weight by up to
  120% over baseline.
- **β = 1.5** (cache miss weight): cache miss ratio typically ranges
  from 0.001 to 0.05, so β × miss_ratio ranges from 0.0015 to
  0.075 — a small adjustment. This is intentional: cache misses
  affect DRAM power more than core power, and DRAM is handled
  separately (see section 2).

Both coefficients are configurable via `KECK_ALPHA` and `KECK_BETA`
environment variables. An optional online auto-tuner adjusts them
based on observed data consistency (enabled with `KECK_AUTO_TUNE=true`).

## 2. Memory (DRAM) power attribution

### What we measure

- **PSS** (Proportional Set Size): how much physical RAM a process
  uses, from `/proc/[pid]/smaps_rollup`. Unlike RSS, PSS splits
  shared memory pages (libc, shared libraries) proportionally among
  all processes using them — no double-counting.
- **LLC misses**: from hardware performance counters (same data as
  the CPU model).

### The formula

```
pod_memory_power = node_memory_power × (0.6 × pod_PSS/total_PSS + 0.4 × pod_LLC/total_LLC)
```

### Why two signals?

DRAM power has two components:

- **Static power (60% weight — PSS)**: DRAM chips must periodically
  refresh every cell to maintain data. More cells holding data = more
  refresh power. A pod holding 8 GB uses more static DRAM power than
  one holding 100 MB, even if both are idle.

- **Dynamic power (40% weight — LLC misses)**: Every LLC miss triggers
  a DRAM read or write. The memory controller, data bus, and DRAM row
  buffers all consume power during these accesses. A pod doing ML
  inference (streaming through large matrices, many LLC misses)
  causes more dynamic DRAM power than an idle pod holding the same
  amount of memory.

### Why not just PSS?

A pod holding 8 GB of cached data but never accessing it should not
be charged the same DRAM power as a pod holding 8 GB and actively
streaming through it. The LLC miss component captures this difference.

When LLC counters are unavailable, falls back to 100% PSS.

## 3. Storage power attribution

### What we measure

- **Node storage power**: from BMC Redfish TelemetryService
  (TotalStoragePower metric) — measured in watts directly from
  hardware.
- **Per-process disk I/O**: bytes read + written from
  `/proc/[pid]/io`. This tracks actual disk I/O (not page cache).

### The formula

```
pod_storage_power = node_storage_power × (pod_io_bytes / total_io_bytes)
```

### Why it can show 0

If no pods are actively reading or writing to disk during the
measurement interval, all pods get 0W storage power. The node's
storage subsystem still draws power (SSDs powered on, controllers
active), but that's idle power — not attributable to any specific pod.

## 4. Network power attribution

### What we measure

- **Node NIC power**: from BMC Redfish sensor (SystemBoardIOUsage
  percentage × board total, or TotalPciePower metric).
- **Per-process TCP bytes**: from eBPF kprobes on `tcp_sendmsg` (TX)
  and `tcp_recvmsg` (RX). These kernel functions are called every
  time a process sends or receives TCP data.

### The formula

```
pod_network_power = node_nic_power × (pod_tcp_bytes / total_tcp_bytes)
```

Falls back to CPU-ratio attribution when TCP kprobes are unavailable.

### Why TCP only?

TCP covers the vast majority of Kubernetes network traffic (HTTP APIs,
gRPC, database connections, container pulls). UDP traffic (DNS, some
streaming) is not tracked — this is a known limitation.

## 5. GPU power

### What we measure

GPU power is **directly measured per-pod** from NVIDIA DCGM (Data
Center GPU Manager). No estimation or model needed — DCGM reports
per-GPU watt readings with pod name and namespace labels.

This is the most accurate component because it's a direct hardware
measurement attributed to a specific pod by the GPU driver itself.

## 6. Ground truth reconciliation

### What we do

The agent compares the sum of all attributed component power against
the PSU total power (measured via Redfish):

```
error_ratio = |PSU_total - sum(CPU + Memory + GPU + Storage + NIC + Fans)| / PSU_total
```

### Why this matters

An error ratio of 0.10 means 10% of total server power is
unaccounted for. This could be:

- Motherboard chipset, voltage regulators, clock distribution
- PCIe bus power not captured by sensors
- Measurement inaccuracies in individual sensors

The error ratio is displayed in the dashboard so users know how
much of the server's power is being tracked vs unmeasured.

## Summary of data sources

| Component | Node-level source | Per-pod signal | Formula |
|-----------|------------------|----------------|---------|
| CPU | RAPL or Redfish | eBPF time + freq + IPC + LLC | time × freq² × (1 + α·IPC + β·miss_ratio) |
| Memory | RAPL DRAM or Redfish | PSS + LLC misses | 0.6 × PSS_ratio + 0.4 × LLC_ratio |
| GPU | DCGM (per-pod) | Direct measurement | N/A (measured) |
| Storage | Redfish MetricReports | /proc/[pid]/io bytes | io_bytes ratio |
| Network | Redfish sensor | eBPF TCP kprobes | tcp_bytes ratio |
| Fans | Redfish MetricReports | N/A (node-level only) | Not attributed to pods |

## Comparison with Kepler

| | Kepler | Keck |
|---|---|---|
| CPU model | `cpu_time / total_time` | `time × freq² × (1 + α·IPC + β·miss)` |
| Frequency awareness | None | Per-core freq² weighting |
| Hardware counters | Not used for attribution | IPC + LLC misses per PID |
| Memory attribution | Not attributed per-pod | PSS + LLC miss ratio |
| Network attribution | None | TCP kprobes per-PID |
| Storage attribution | None | /proc/[pid]/io + Redfish |
| Ground truth | RAPL only | RAPL + Redfish PSU reconciliation |
| Energy conservation | Not guaranteed | Guaranteed by normalization |
| Training required | Yes (regression model) | No (physics-based, auto-tunable) |
