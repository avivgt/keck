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
- GPU power (NVIDIA DCGM per-pod measured)
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
- Memory attribution: 60% PSS + 40% LLC miss ratio (with PSS caching)
- Aggregation: process → container → pod → namespace
- Local ring buffer store with drill-down query API

### Cluster Controller (keck-controller)

Runs as a single Deployment. Aggregates power data across all nodes.

- Receives pod-level summaries from node agents via HTTP POST
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
│       ├── api/         REST API (axum) + bearer token auth
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

# Build operator
cd keck-operator && make build
```

## Deployment

For detailed step-by-step OpenShift deployment instructions (building images
on OCP, installing via OLM, deploying the console plugin), see
**[docs/openshift-deployment.md](docs/openshift-deployment.md)**.

### Option 1: OpenShift / OLM (Recommended for Production)

The Keck operator follows the Red Hat Operator Lifecycle Manager (OLM)
standard. It can be installed from OperatorHub or from a custom catalog.

```bash
# Build and push all images
cd keck-operator
make release   # builds: operator, bundle, catalog images

# Add Keck catalog to the cluster
make catalog-deploy

# Install the operator via OLM Subscription
make subscription-deploy
```

After the operator is installed, create a `KeckCluster` resource to
deploy Keck to your cluster:

```yaml
apiVersion: keck.io/v1alpha1
kind: KeckCluster
metadata:
  name: keck
spec:
  agent:
    defaultProfile: standard
    gpuEnabled: false
  controller:
    replicas: 1
    schedulerEnabled: false
    carbonRegion: "US-CAL-CISO"
    energyCostPerKWh: "0.10"
  image:
    repository: ghcr.io/avivgt/keck
    tag: latest
```

```bash
kubectl apply -f keck-operator/config/samples/keckcluster.yaml
```

The operator will create:
- `keck-system` namespace
- `keck-agent` DaemonSet (one agent per node, privileged)
- `keck-controller` Deployment
- ServiceAccount, ClusterRole, ClusterRoleBinding
- Services for controller gRPC and HTTP endpoints

**Verify:**
```bash
kubectl get keckclusters
# NAME   AGENTS   CONTROLLER   PHASE     AGE
# keck   12       true         Running   2m

kubectl get pods -n keck-system
# NAME                               READY   STATUS    RESTARTS   AGE
# keck-agent-xxxxx                   1/1     Running   0          2m
# keck-agent-yyyyy                   1/1     Running   0          2m
# keck-controller-zzzzz-aaaaa        1/1     Running   0          2m
```

### Option 2: Direct Deployment (Without OLM)

For clusters without OLM (vanilla Kubernetes, k3s, etc.):

```bash
cd keck-operator

# Install CRDs
make install

# Deploy operator, RBAC, and manager
make deploy

# Create KeckCluster
kubectl apply -f config/samples/keckcluster.yaml
```

To remove:
```bash
make undeploy
```

### Option 3: Local Development

Run the operator outside the cluster for development:

```bash
cd keck-operator

# Install CRDs into your dev cluster
make install

# Run operator locally (uses ~/.kube/config)
make run
```

### Setting Power Budgets

Limit power consumption per namespace:

```yaml
apiVersion: keck.io/v1alpha1
kind: PowerBudget
metadata:
  name: ml-training-budget
  namespace: ml-training
spec:
  maxWatts: 10000
  action: reject   # alert | throttle | reject
```

```bash
kubectl apply -f keck-operator/config/samples/powerbudget.yaml

kubectl get powerbudgets -A
# NAMESPACE      NAME                 BUDGET (W)   CURRENT (W)   USAGE   EXCEEDED
# ml-training    ml-training-budget   10000        7234          72%     false
```

### Customizing Agent Profiles Per Node

Use `PowerProfile` to override the agent profile on specific nodes:

```yaml
# Full metering on GPU nodes
apiVersion: keck.io/v1alpha1
kind: PowerProfile
metadata:
  name: gpu-nodes-full
spec:
  profile: full
  nodeSelector:
    nvidia.com/gpu.present: "true"
  gpuEnabled: true
---
# Minimal overhead on edge nodes
apiVersion: keck.io/v1alpha1
kind: PowerProfile
metadata:
  name: edge-minimal
spec:
  profile: minimal
  nodeSelector:
    node-role.kubernetes.io/edge: ""
```

```bash
kubectl apply -f keck-operator/config/samples/powerprofile.yaml

kubectl get powerprofiles
# NAME              PROFILE   NODES   AGE
# gpu-nodes-full    full      4       1m
# edge-minimal      minimal   2       1m
```

### Multi-Cluster Setup (Fleet Manager)

For multi-cluster deployments, run the fleet manager separately and
point each cluster's controller to it:

```yaml
apiVersion: keck.io/v1alpha1
kind: KeckCluster
metadata:
  name: keck
spec:
  # ... agent and controller config ...
  fleetEndpoint: "fleet-manager.example.com:9091"
```

The fleet manager aggregates data from all clusters and provides:
- Unified dashboard at `http://<fleet-manager>:8090`
- Per-team power/carbon/cost views
- Carbon-aware routing recommendations
- Policy enforcement across clusters
- ESG reporting

### Accessing the Dashboard

On **OpenShift**, the Keck UI integrates directly into the console as a
Dynamic Console Plugin. After deployment, "Power Management" appears in
the left navigation — no separate URL needed.

For **non-OpenShift** clusters, port-forward to the controller REST API:

```bash
kubectl port-forward -n keck-system svc/keck-controller 8080:8080
# API available at http://localhost:8080/api/v1/cluster
```

## Status

**Deployed and running on OpenShift** (Dell PowerEdge R750, 2 nodes, NVIDIA A100 GPUs).

- [x] Kubernetes operator with OLM bundle and finalizer cleanup
- [x] CRDs: KeckCluster, PowerBudget, PowerProfile
- [x] OpenShift console plugin ("Power Management" in left nav)
- [x] GPU power via DCGM (per-pod, measured from hardware)
- [x] Vendor-agnostic Redfish discovery (3-level probing)
- [x] Source priority system (Measured > Estimated, auto-select)
- [x] REST API with bearer token auth and input validation
- [x] Per-process CPU attribution (/proc + eBPF frequency weighting)
- [x] Per-process memory attribution (PSS + LLC misses, cached reads)
- [x] Container images built on OCP (agent, controller, UI)
- [x] 139 unit tests across all components
- [ ] Prometheus /metrics endpoint
- [ ] Fleet manager deployment
- [ ] Carbon tracking connected to external API
- [ ] Benchmark: agent overhead vs Kepler

## License

Apache-2.0
