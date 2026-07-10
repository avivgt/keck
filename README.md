# Keck

Per-workload power metering for bare metal Kubernetes.

Named after the [W. M. Keck Observatory](https://www.keckobservatory.org/),
Keck measures energy consumption at every level -- from individual CPU cores to
cluster-wide namespace views -- with hardware-grounded accuracy and transparent
error bounds.

## Why Keck?

Existing tools attribute power using a single signal:

```
process_energy = node_energy x (process_cpu_time / total_cpu_time)
```

This is inaccurate. Two processes with the same CPU time can consume 3x different
power depending on frequency, workload type, and memory behavior. Keck fixes this
with bottom-up attribution: per-core energy, weighted by multiple hardware signals,
reconciled against PSU ground truth.

### What makes Keck different

| Aspect | Traditional approach | Keck |
|---|---|---|
| **Attribution** | Node-level CPU time ratio | Per-core, frequency-weighted, multi-signal |
| **Data source** | `/proc/[pid]/stat` polling | eBPF `sched_switch` tracepoint (in-kernel) |
| **Frequency awareness** | None | Per-core time-at-frequency tracking |
| **Hardware counters** | None | Instructions, cycles, LLC misses per core |
| **Accuracy validation** | None | PSU reconciliation with error bounds |
| **Architecture** | Passive exporter (scrape) | Active agent (aggregate, push, query) |
| **Language** | Go | Rust (no GC, eBPF via Aya) |

## Architecture

```
Layer 3:  keck-controller    Cluster controller (aggregation, REST API)
              | HTTP
Layer 2:  keck-agent         Attribution engine, K8s enrichment
              | BPF maps
Layer 1:  keck-ebpf          Kernel programs (sched_switch, cpu_frequency)
              | sysfs/MSR
Layer 0:  keck-agent         Hardware readers (RAPL, hwmon, GPU, Redfish)
              |
          keck-common         Shared types (no_std, eBPF + userspace)
```

### Node Agent (keck-agent)

Runs as a DaemonSet. Collects and attributes power on each node.

**Layer 0 -- Hardware signals:**
- RAPL energy counters (per-socket CPU + DRAM)
- hwmon power sensors (direct electrical measurements)
- GPU power (NVIDIA DCGM per-pod measured)
- Platform power via Redfish/IPMI (PSU ground truth)
- Reconciliation: components vs PSU input produces error_ratio

**Layer 1 -- Kernel observation (eBPF):**
- `sched_switch` tracepoint: per-PID per-core CPU time (nanosecond precision)
- `cpu_frequency` tracepoint: per-core time-at-frequency tracking
- `perf_event_open`: per-core hardware counters (instructions, cycles, LLC misses)
- `cgroup_id` capture: pid to container mapping without `/proc` reads

**Layer 2 -- Attribution engine:**
- Splits socket RAPL energy to per-core using frequency-weighted model
- Three attribution models (auto-selected by available data):
  - **FullModel**: time x freq^2 x (1 + alpha*IPC + beta*cache_miss_rate)
  - **FrequencyWeighted**: time x freq^2
  - **CpuTimeRatio**: time only (basic fallback)
- Normalization: sum of process energy = core energy (energy conservation guaranteed)
- Memory attribution: 60% PSS + 40% LLC miss ratio (with PSS caching)
- Aggregation: process to container to pod to namespace

### Cluster Controller (keck-controller)

Runs as a single Deployment. Aggregates power data across all nodes.

- Receives pod-level summaries from node agents via HTTP POST
- Aggregates: pod to namespace to cluster
- REST API with bearer token authentication and input validation
- Stale data eviction (60s threshold)
- Workload classification: platform / operator / application (by namespace)

### OpenShift Console Plugin (keck-ui)

Dynamic Console Plugin that adds "Power Consumption" to the OpenShift console navigation.

- Cluster overview with power breakdown by component
- Per-namespace, per-node, per-pod views
- Kepler side-by-side comparison (toggle between Keck and Kepler attribution)
- PatternFly 5 + React, no separate URL needed

## Project Structure

```
keck/
+-- keck-common/       Shared types (no_std, works in kernel + userspace)
+-- keck-ebpf/         eBPF programs (sched_switch, cpu_frequency)
+-- keck-agent/        Node agent
|   +-- src/
|       +-- hardware/    Layer 0: RAPL, hwmon, GPU, Redfish, tiered collector
|       +-- ebpf/        Layer 1: eBPF loader, map drainer, perf counters
|       +-- attribution/ Layer 2: models, engine, types
|       +-- k8s/         Layer 2: cgroup to container to pod enrichment
|       +-- store/       Layer 2: ring buffer with outbox
|       +-- output/      Layer 2: query API
+-- keck-controller/   Cluster controller
|   +-- src/
|       +-- aggregator/  Cluster-wide state
|       +-- api/         REST API (axum) + bearer token auth
|       +-- carbon/      Carbon intensity tracking (static fallback only)
|       +-- scheduler/   Power-aware scheduler (scoring logic, server not yet wired)
+-- keck-fleet/        Fleet manager (planned, not yet functional)
+-- keck-operator/     Kubernetes operator (Go, kubebuilder)
+-- keck-ui/           OpenShift console plugin (TypeScript, PatternFly)
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

# Build operator
cd keck-operator && make build
```

## Quick Install on OpenShift

Install the Keck operator with one command:

```bash
oc apply -f https://raw.githubusercontent.com/avivgt/keck/main/install.yaml
```

This creates the `keck-system` namespace, adds the Keck catalog to OLM,
and installs the operator automatically. After ~60 seconds:

1. Go to **Operators > Installed Operators** (namespace: `keck-system`)
2. Click **Keck Operator**
3. Click **Create KeckCluster** to deploy agents and controller

To remove:
```bash
oc delete sub keck-operator -n keck-system
oc delete csv keck-operator.v0.1.0 -n keck-system
oc delete operatorgroup keck-operator-group -n keck-system
oc delete catalogsource keck-operator-catalog -n openshift-marketplace
oc delete ns keck-system
```

## Deployment

For detailed step-by-step deployment instructions, see
**[docs/openshift-deployment.md](docs/openshift-deployment.md)**.

### Option 1: OpenShift / OLM (Recommended for Production)

The Keck operator follows the Red Hat Operator Lifecycle Manager (OLM)
standard. Use the quick install above, or build from source:

```bash
# Build and push operator, bundle, and catalog images to quay.io
./scripts/release.sh 0.1.0

# Users then install via: oc apply -f install.yaml
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
  image:
    repository: quay.io/aguetta
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
- Services for controller HTTP and gRPC endpoints

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

### Accessing the Dashboard

On **OpenShift**, the Keck UI integrates directly into the console as a
Dynamic Console Plugin. After deployment, "Power Consumption" appears in
the left navigation -- no separate URL needed.

For **non-OpenShift** clusters, port-forward to the controller REST API:

```bash
kubectl port-forward -n keck-system svc/keck-controller 8080:8080
# API available at http://localhost:8080/api/v1/cluster
```

## Status

**Deployed and running on OpenShift.**

Working:
- [x] Kubernetes operator with OLM bundle and finalizer cleanup
- [x] CRD: KeckCluster (deploys agent DaemonSet + controller Deployment)
- [x] OpenShift console plugin ("Power Consumption" in left nav)
- [x] GPU power via DCGM (per-pod, measured from hardware)
- [x] Vendor-agnostic Redfish discovery (3-level probing)
- [x] Source priority system (Measured > Estimated, auto-select)
- [x] REST API with bearer token auth and input validation
- [x] Per-process CPU attribution (/proc + eBPF frequency weighting)
- [x] Per-process memory attribution (PSS + LLC misses, cached reads)
- [x] Kepler side-by-side comparison and toggle in UI
- [x] 158 unit tests across all components

Not yet implemented:
- [ ] Prometheus /metrics endpoint (exporter skeleton exists)
- [ ] PowerBudget enforcement (CRD exists, reconciler needs controller API query)
- [ ] PowerProfile per-node controller (CRD exists, no reconciler)
- [ ] Power-aware scheduler extender (scoring logic tested, HTTP server needed)
- [ ] Carbon intensity API integration (static fallback only)
- [ ] Agent profile switching (minimal/standard/full)
- [ ] Fleet manager (data models exist, API servers needed)
- [ ] K8s custom metrics API for HPA
- [ ] Benchmark: agent overhead measurement

## Roadmap

### Near-term

**Prometheus metrics export** -- Implement /metrics on the controller exposing
cluster, namespace, node, and pod power gauges. Create ServiceMonitor and
PrometheusRule with basic alerts (agent down, controller down, high error ratio).

**Health probes** -- Add liveness and readiness probes to agent and controller
pods. The controller already serves /healthz; the agent needs a minimal HTTP
endpoint.

**PowerBudget enforcement** -- Wire the PowerBudget reconciler to query the
controller REST API for current namespace power. Populate status with real
usage data.

### Medium-term

**Scheduler extender** -- Expose the existing scoring logic via an HTTP server
that K8s can register as a scheduler extender. Filter by namespace power budget,
prioritize by power headroom and metering accuracy.

**Carbon API integration** -- Connect to Electricity Maps or WattTime for
real-time carbon intensity. The data model and cost calculation logic exist;
only the HTTP client is missing.

**PowerProfile controller** -- Reconcile PowerProfile CRDs to apply per-node
agent profile overrides via node selector matching.

### Long-term

**Fleet manager** -- Multi-cluster aggregation, team views, policy engine,
ESG reporting. Data models and policy logic exist in keck-fleet/; API servers
need implementation.

**Agent self-monitoring** -- Track agent resource usage, eBPF map pressure,
and auto-downgrade profile if budget exceeded.

**K8s custom metrics API** -- Enable HPA scaling based on power consumption
per pod or namespace.

## License

Apache-2.0
