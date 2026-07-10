# Keck Operations Runbook

## Prerequisites

- OpenShift 4.14+ or Kubernetes 1.28+
- Linux kernel 5.8+ (eBPF tracepoint support)
- Bare metal nodes (RAPL requires direct CPU access, not available in VMs/cloud)

## Network Requirements

| Source | Destination | Port | Protocol | Purpose |
|--------|-------------|------|----------|---------|
| keck-agent | keck-controller | 8080 | TCP/HTTP | Report ingestion |
| prometheus | keck-controller | 8080 | TCP/HTTP | Metrics scraping (/metrics) |
| prometheus | keck-agent | 9100 | TCP/HTTP | Agent health check |
| openshift-console | keck-controller | 9443 | TCP/HTTPS | UI plugin proxy |
| keck-agent | kube-apiserver | 6443 | TCP/HTTPS | Pod metadata lookup |
| keck-agent | BMC/iDRAC | 443 | TCP/HTTPS | Redfish power data (optional) |

## Capacity Planning

### Agent Resource Usage

| Profile | CPU (idle) | CPU (peak) | Memory | Notes |
|---------|-----------|-----------|--------|-------|
| Standard | 10-30m | 50-100m | 30-60 MB | Default, recommended |

Memory scales with pod count on the node. Each pod adds ~1 KB to the agent's working set. A node with 500 pods uses ~60 MB.

eBPF map sizes scale with unique PID count. The default BPF map capacity is 10K entries (configurable via KECK_EBPF_MAP_SIZE).

### Controller Resource Usage

The controller holds all pod power data in memory. Memory scales with total pod count across the cluster.

| Cluster Size | Controller Memory |
|-------------|------------------|
| 50 pods | ~20 MB |
| 500 pods | ~50 MB |
| 5000 pods | ~200 MB |
| 50000 pods | ~1.5 GB |

Stale data is evicted after 60 seconds of silence from an agent.

## Common Failure Modes

### Agent pods not starting

**Symptom:** Agent pods stuck in CrashLoopBackOff or Error.

**Check SCC (OpenShift):**
```bash
oc get scc privileged -o yaml | grep keck
# If missing:
oc adm policy add-scc-to-user privileged -z keck-agent -n keck-system
```

**Check kernel version:**
```bash
oc debug node/<node-name> -- uname -r
# Must be 5.8+ for eBPF tracepoints
```

**Check agent logs:**
```bash
oc logs ds/keck-agent -n keck-system --tail=50
```

Common log messages:
- `eBPF not available`: Kernel too old, missing BTF, or capabilities insufficient. Agent falls back to /proc-based attribution (less accurate but functional).
- `Failed to create K8s client`: ServiceAccount token not mounted. Check SA exists and has RBAC.
- `No power sources found`: RAPL not available (VM, cloud instance, or AMD CPU without powercap support). Agent will report zero power.

### Agent reporting zero power

**Symptom:** UI shows 0W for nodes.

**Check RAPL availability:**
```bash
oc debug node/<node-name> -- ls /sys/class/powercap/intel-rapl:0/
# Should show energy_uj, name, etc.
```

If RAPL is not available:
- VMs and cloud instances do not expose RAPL. Keck requires bare metal.
- Some AMD CPUs need the `powercap_amd_rapl` module loaded.
- SELinux may block access. Check `ausearch -m avc -ts recent | grep powercap`.

**Check Redfish (if configured):**
```bash
oc logs ds/keck-agent -n keck-system | grep -i redfish
# Should show "Redfish: probing <endpoint>"
```

### Controller not aggregating data

**Symptom:** Controller pod is running but UI shows no data.

**Check report flow:**
```bash
# Verify agent is POSTing
oc logs ds/keck-agent -n keck-system | grep "Reported:"
# Should show: "Reported: cpu=XX.XW(estimated) mem=XX.XW platform=XX.XW pods=N"

# Verify controller is receiving
oc logs deploy/keck-controller -n keck-system | grep "Ingested"
```

**Check authentication:**
```bash
# Verify both agent and controller use the same API key
oc get secret keck-api-key -n keck-system -o jsonpath='{.data.api-key}' | base64 -d
```

If agent logs show `Controller returned 401`: API key mismatch. Delete the secret and let the operator regenerate it, then restart both pods.

### High error_ratio on a node

**Symptom:** keck_node_error_ratio metric is > 0.3.

Error ratio measures the discrepancy between component-level power (RAPL CPU + RAPL DRAM + GPU) and PSU-level power (Redfish). High values indicate:

1. **Missing component**: GPU power not being read, but GPU is drawing power. Enable `gpuEnabled: true` in KeckCluster spec.
2. **Stale Redfish data**: BMC updating slower than the agent poll interval. Check Redfish endpoint response time.
3. **Unmetered components**: NIC, storage, fans consume power that RAPL does not measure. This is expected -- error ratios of 0.1-0.3 are normal when only RAPL is used.

### UI not showing in OpenShift console

**Symptom:** "Power Consumption" not in left navigation.

```bash
# Check plugin is registered
oc get consoleplugins keck-power-management

# Check it's enabled
oc get console.operator.openshift.io cluster -o jsonpath='{.spec.plugins}'
# Should contain "keck-power-management"

# If not enabled:
bash keck-ui/openshift/enable-plugin.sh

# Check plugin pod is running
oc get pods -n keck-system -l app.kubernetes.io/name=keck-power-management
```

After enabling the plugin, hard-refresh the browser (Ctrl+Shift+R).

## Alerting

Keck creates a PrometheusRule with these alerts:

| Alert | Severity | Fires When | Action |
|-------|----------|-----------|--------|
| KeckControllerDown | Critical | Controller unreachable for 2m | Check controller pod status, logs |
| KeckAgentNotReporting | Warning | Agent silent for 2m+ | Check agent pod on that node, network |
| KeckHighErrorRatio | Warning | error_ratio > 0.3 for 10m | Check missing power sources |
| KeckNoPowerData | Warning | Zero nodes reporting for 5m | Check all agent pods, controller |

## Metrics Reference

All metrics are exposed on the controller at `GET :8080/metrics`.

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| keck_cluster_power_watts | Gauge | component | Total cluster power by component (cpu/memory/gpu/platform) |
| keck_namespace_power_watts | Gauge | namespace, component | Total namespace power |
| keck_node_power_watts | Gauge | node | Total node power |
| keck_node_error_ratio | Gauge | node | PSU reconciliation error ratio |
| keck_agent_last_report_seconds_ago | Gauge | node | Seconds since last report |
| keck_agent_report_total | Counter | node | Total reports received |
| keck_pod_count | Gauge | -- | Number of pods with power data |
| keck_node_count | Gauge | -- | Number of reporting nodes |

## Uninstalling

```bash
# Delete KeckCluster (operator cleans up child resources via finalizer)
oc delete keckclusters --all

# Wait for finalizer cleanup
oc get pods -n keck-system
# Should show only the operator pod remaining

# Delete operator via OLM
oc delete sub keck-operator -n keck-system
oc delete csv keck-operator.v0.1.0 -n keck-system
oc delete operatorgroup keck-operator-group -n keck-system
oc delete catalogsource keck-operator-catalog -n openshift-marketplace

# Delete namespace
oc delete ns keck-system

# Delete cluster-scoped resources
oc delete clusterrole keck-agent
oc delete clusterrolebinding keck-agent
oc delete crd keckclusters.keck.io powerbudgets.keck.io powerprofiles.keck.io
```
