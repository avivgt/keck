// SPDX-License-Identifier: Apache-2.0

// Namespace power breakdown — shows all namespaces with power consumption.
// Click a namespace to drill down to pods.

import * as React from "react";
import { useHistory } from "react-router-dom";
import {
  Page,
  PageSection,
  Title,
  Card,
  CardTitle,
  CardBody,
  Spinner,
  EmptyState,
  EmptyStateBody,
} from "@patternfly/react-core";
import {
  Table,
  Thead,
  Tbody,
  Tr,
  Th,
  Td,
  ThProps,
} from "@patternfly/react-table";
import { api, NamespacePower } from "../../utils/api";
import { formatWatts } from "../../utils/format";

type SortKey = "namespace" | "total_watts" | "cpu_watts" | "memory_watts" | "gpu_watts" | "pod_count";

const ClusterOverview: React.FC = () => {
  const [namespaces, setNamespaces] = React.useState<NamespacePower[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [sortBy, setSortBy] = React.useState<SortKey>("total_watts");
  const [sortDir, setSortDir] = React.useState<"asc" | "desc">("desc");
  const history = useHistory();

  React.useEffect(() => {
    const fetchData = () => {
      api.getNamespaces()
        .then(setNamespaces)
        .finally(() => setLoading(false));
    };
    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, []);

  if (loading) {
    return <Page><PageSection><Spinner /></PageSection></Page>;
  }

  const totalWatts = namespaces.reduce((sum, ns) => sum + ns.total_watts, 0);

  return (
    <Page>
      <PageSection>
        <Title headingLevel="h1" size="xl">
          Power by Namespace
        </Title>
        <p style={{ marginTop: 4, color: "var(--pf-v6-global--Color--200)" }}>
          {namespaces.length} namespaces, {formatWatts(totalWatts)} total.
          Click a namespace to see pod-level detail.
        </p>
      </PageSection>

      {/* Attribution Methodology */}
      <PageSection>
        <Card>
          <CardTitle>How Power is Calculated</CardTitle>
          <CardBody>
            <div style={{ fontSize: "0.9em", lineHeight: 1.7 }}>
              <p><strong>1. Hardware discovery (vendor-agnostic)</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                At startup, the agent probes the BMC's Redfish API to discover available power metrics.
                Three levels are tried in order:
                <br />• <strong>Level 1</strong>: TelemetryService MetricDefinitions (TotalCPUPower, TotalMemoryPower,
                TotalFanPower, TotalStoragePower, TotalPciePower)
                <br />• <strong>Level 2</strong>: Sensors API percentage readings (SystemBoardCPUUsage × board total)
                <br />• <strong>Level 3</strong>: Power API (PowerConsumedWatts — PSU total, all vendors)
                <br />Whatever is not covered by Redfish falls back to Intel RAPL energy counters (estimated).
                This works on any BMC vendor (Dell, HP, Lenovo, Supermicro) — no vendor-specific code.
              </p>

              <p style={{ marginTop: 12 }}><strong>2. Source priority (Measured {">"} Estimated)</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                For each component (CPU, Memory, GPU, Platform), the agent reads ALL available sources
                and automatically selects the most accurate: Measured (Redfish VR sensors, DCGM GPU)
                {">"} Estimated (RAPL firmware model) {">"} Unavailable.
                All sources and their selection status are shown in the Overview → Data Sources table.
              </p>

              <p style={{ marginTop: 12 }}><strong>3. Per-process CPU attribution</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                The agent reads <code>/proc/[pid]/stat</code> for every process on the node,
                computing the CPU time delta (utime + stime) between collection intervals.
                Threads are filtered out via <code>/proc/[pid]/status</code> Tgid check
                to prevent double-counting from goroutines and thread pools.
                <br />Formula: pod CPU power = node CPU power × (pod CPU time delta / total CPU time delta across all processes).
              </p>

              <p style={{ marginTop: 12 }}><strong>4. Per-process memory attribution (PSS + LLC misses)</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                DRAM power has two components, and the agent measures both:
                <br /><br />
                <strong>Static (60%)</strong>: PSS (Proportional Set Size) from <code>/proc/[pid]/smaps_rollup</code>.
                Unlike RSS, PSS splits shared memory pages (libc, etc.) proportionally among all
                processes using them, eliminating double-counting.
                This captures DRAM refresh power — proportional to how much memory a pod holds.
                <br /><br />
                <strong>Dynamic (40%)</strong>: LLC (Last Level Cache) miss counters from hardware
                performance counters via <code>perf_event_open</code>.
                Every LLC miss = one DRAM read/write = dynamic power cost.
                Pods actively streaming data through memory (ML inference, in-memory databases)
                cause more LLC misses and get charged more DRAM power than idle pods holding
                equivalent memory.
                <br /><br />
                When LLC counters are unavailable, falls back to 100% PSS.
                <br />Formula: pod memory power = node memory power × (0.6 × PSS ratio + 0.4 × LLC miss ratio).
              </p>

              <p style={{ marginTop: 12 }}><strong>5. Per-pod GPU attribution (DCGM)</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                GPU power is read from the NVIDIA DCGM exporter, which provides per-pod, per-GPU
                measured power directly from the GPU hardware. No estimation needed — DCGM metrics
                include pod name, namespace, and container labels. The agent auto-discovers the
                DCGM exporter pod on its node via the Kubernetes API.
              </p>

              <p style={{ marginTop: 12 }}><strong>6. Pod → Namespace aggregation</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                Processes are mapped to pods via cgroup v2 paths in <code>/proc/[pid]/cgroup</code>.
                Pod UIDs are resolved to names and namespaces via the Kubernetes API (cached, refreshed every 30s).
                Namespace power = sum of all pod power within the namespace.
              </p>

              <p style={{ marginTop: 12 }}><strong>7. eBPF kernel observation</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                eBPF programs attached to <code>sched_switch</code> and <code>cpu_frequency</code>
                tracepoints collect per-PID per-core CPU time with nanosecond precision
                and per-core frequency transitions. This data is collected for future
                per-core frequency-weighted attribution.
              </p>
            </div>
          </CardBody>
        </Card>
      </PageSection>

      <PageSection>
        {namespaces.length > 0 ? (() => {
          const sorted = [...namespaces].sort((a, b) => {
            const av = a[sortBy] as any;
            const bv = b[sortBy] as any;
            if (typeof av === "string") {
              return sortDir === "asc" ? av.localeCompare(bv) : bv.localeCompare(av);
            }
            return sortDir === "asc" ? av - bv : bv - av;
          });

          const getSortParams = (key: SortKey): ThProps["sort"] => ({
            sortBy: {
              index: ["namespace", "total_watts", "cpu_watts", "memory_watts", "gpu_watts", "pod_count"].indexOf(sortBy),
              direction: sortDir,
            },
            onSort: (_e, _idx, dir) => {
              setSortBy(key);
              setSortDir(dir as "asc" | "desc");
            },
            columnIndex: ["namespace", "total_watts", "cpu_watts", "memory_watts", "gpu_watts", "pod_count"].indexOf(key),
          });

          return (
            <Table aria-label="Namespace power table" variant="compact">
              <Thead>
                <Tr>
                  <Th sort={getSortParams("namespace")}>Namespace</Th>
                  <Th sort={getSortParams("total_watts")}>Total Power</Th>
                  <Th sort={getSortParams("cpu_watts")}>CPU</Th>
                  <Th sort={getSortParams("memory_watts")}>Memory</Th>
                  <Th sort={getSortParams("gpu_watts")}>GPU</Th>
                  <Th sort={getSortParams("pod_count")}>Pods</Th>
                </Tr>
              </Thead>
              <Tbody>
                {sorted.map((ns) => (
                  <Tr
                    key={ns.namespace}
                    isClickable
                    onRowClick={() => history.push(`/power-management/namespaces/${ns.namespace}`)}
                  >
                    <Td>{ns.namespace}</Td>
                    <Td>{formatWatts(ns.total_watts)}</Td>
                    <Td>{formatWatts(ns.cpu_watts)}</Td>
                    <Td>{formatWatts(ns.memory_watts)}</Td>
                    <Td>{formatWatts(ns.gpu_watts)}</Td>
                    <Td>{ns.pod_count}</Td>
                  </Tr>
                ))}
              </Tbody>
            </Table>
          );
        })() : (
          <EmptyState>
            <EmptyStateBody>No namespace power data available.</EmptyStateBody>
          </EmptyState>
        )}
      </PageSection>

    </Page>
  );
};

export default ClusterOverview;
