// SPDX-License-Identifier: Apache-2.0

// Pod-level power for a specific namespace. Drill-down from ClusterOverview.

import * as React from "react";
import { Link } from "react-router-dom";
import {
  Page,
  PageSection,
  Title,
  Breadcrumb,
  BreadcrumbItem,
  Spinner,
  EmptyState,
  EmptyStateBody,
  ExpandableSection,
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
import { api, PodPower } from "../../utils/api";
import { formatWatts } from "../../utils/format";

/** Extract namespace from URL path: /power-management/namespaces/<ns> */
function getNamespaceFromURL(): string {
  const path = window.location.pathname;
  const prefix = "/power-management/namespaces/";
  const idx = path.indexOf(prefix);
  if (idx >= 0) {
    const rest = path.slice(idx + prefix.length);
    // Take everything up to the next / or end
    const ns = rest.split("/")[0];
    return decodeURIComponent(ns);
  }
  return "";
}

type PodSortKey = "pod_name" | "node_name" | "total_watts" | "cpu_watts" | "memory_watts" | "gpu_watts" | "storage_watts" | "io_watts";

const NamespaceView: React.FC = () => {
  const [ns, setNs] = React.useState(() => getNamespaceFromURL());
  const [pods, setPods] = React.useState<PodPower[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [sortBy, setSortBy] = React.useState<PodSortKey>("total_watts");
  const [sortDir, setSortDir] = React.useState<"asc" | "desc">("desc");
  const [error, setError] = React.useState<string | null>(null);

  // Update ns if URL changes
  React.useEffect(() => {
    const checkUrl = () => {
      const current = getNamespaceFromURL();
      if (current && current !== ns) {
        setNs(current);
      }
    };
    checkUrl();
    // Poll for URL changes (console plugin routing may not trigger re-renders)
    const interval = setInterval(checkUrl, 500);
    return () => clearInterval(interval);
  }, [ns]);

  React.useEffect(() => {
    if (!ns) {
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);

    const fetchData = () => {
      api.getNamespacePods(ns)
        .then((data) => {
          setPods(data);
          setError(null);
        })
        .catch((e) => {
          setError(e.message);
        })
        .finally(() => setLoading(false));
    };

    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, [ns]);

  if (!ns) {
    return (
      <Page>
        <PageSection>
          <EmptyState>
            <EmptyStateBody>No namespace specified in URL.</EmptyStateBody>
          </EmptyState>
        </PageSection>
      </Page>
    );
  }

  if (loading) {
    return <Page><PageSection><Spinner aria-label="Loading pods" /></PageSection></Page>;
  }

  if (error) {
    return (
      <Page>
        <PageSection>
          <EmptyState>
            <Title headingLevel="h2" size="lg">Error loading pods</Title>
            <EmptyStateBody>{error}</EmptyStateBody>
          </EmptyState>
        </PageSection>
      </Page>
    );
  }

  const totalWatts = pods.reduce((sum, p) => sum + p.total_watts, 0);

  return (
    <Page>
      <PageSection>
        <Breadcrumb>
          <BreadcrumbItem>
            <Link to="/power-management">Power Management</Link>
          </BreadcrumbItem>
          <BreadcrumbItem>
            <Link to="/power-management/namespaces">Namespaces</Link>
          </BreadcrumbItem>
          <BreadcrumbItem isActive>{ns}</BreadcrumbItem>
        </Breadcrumb>

        <Title headingLevel="h1" size="xl" style={{ marginTop: 16 }}>
          {ns}
        </Title>
        <p style={{ color: "var(--pf-v6-global--Color--200)" }}>
          {pods.length} pods, {formatWatts(totalWatts)} total
        </p>
      </PageSection>

      <PageSection>
        <ExpandableSection toggleText="How Power is Calculated" isIndented>
          <div style={{ fontSize: "0.9em", lineHeight: 1.7 }}>
            <p><strong>1. Hardware power measurement</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              The agent probes each node's BMC via the Redfish API to discover per-subsystem
              power sensors (CPU, Memory, I/O, Platform, individual PSUs).
              Whatever Redfish doesn't cover falls back to Intel RAPL energy counters.
              Measured sources (Redfish VR sensors, DCGM GPU) are always preferred over
              estimated (RAPL).
            </p>

            <p style={{ marginTop: 12 }}><strong>2. Per-core CPU attribution (eBPF + hardware counters)</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              eBPF programs attached to kernel tracepoints (<code>sched_switch</code>,{" "}
              <code>cpu_frequency</code>) track per-PID per-core CPU time with nanosecond
              precision and per-core frequency transitions. Hardware performance counters
              (<code>perf_event_open</code>) measure instructions retired, CPU cycles, and
              LLC cache misses per core.
              <br /><br />
              The agent selects the best attribution model based on available data:
              <br />• <strong>Full model</strong>: weight = time × freq² × (1 + α·IPC + β·cache_miss_rate).
              Captures compute intensity (IPC) and memory-boundedness (cache misses).
              <br />• <strong>Frequency-weighted</strong>: weight = time × freq² (no counter data available).
              A process at 3.5 GHz is charged ~3× more than one at 1.2 GHz for the same duration.
              <br />• <strong>CPU time ratio</strong>: weight = time only (simplest fallback).
              <br /><br />
              All models guarantee energy conservation: Σ(process energy on core) = core energy.
            </p>

            <p style={{ marginTop: 12 }}><strong>3. Memory attribution (PSS + LLC misses)</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              DRAM power has two components:
              <br />• <strong>Static (60%)</strong>: PSS (Proportional Set Size) from{" "}
              <code>/proc/[pid]/smaps_rollup</code> — captures DRAM refresh power proportional
              to memory held. PSS splits shared pages among users, preventing double-counting.
              <br />• <strong>Dynamic (40%)</strong>: LLC miss counters — every LLC miss triggers a
              DRAM access. Pods streaming data (ML inference, in-memory DBs) are charged more
              than idle pods holding equivalent memory.
              <br /><br />
              Formula: pod memory power = node memory power × (0.6 × pod_PSS/total_PSS + 0.4 × pod_LLC/total_LLC).
              Falls back to 100% PSS when LLC counters are unavailable.
            </p>

            <p style={{ marginTop: 12 }}><strong>4. Network I/O attribution (TCP kprobes)</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              eBPF kprobes on <code>tcp_sendmsg</code> and <code>tcp_recvmsg</code> track
              per-PID TCP bytes sent and received. This data is used to attribute network-related
              power (NIC, switching fabric) to individual pods proportional to their traffic.
              Kprobes are optional — if unavailable, the agent falls back to CPU-ratio-based
              network attribution.
            </p>

            <p style={{ marginTop: 12 }}><strong>5. GPU attribution (DCGM)</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              GPU power is read from the NVIDIA DCGM exporter, providing per-pod, per-GPU
              measured power directly from the hardware. The agent auto-discovers the
              DCGM exporter pod on its node via the Kubernetes API.
            </p>

            <p style={{ marginTop: 12 }}><strong>6. Pod → Namespace aggregation</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              Processes are mapped to pods via cgroup v2 paths and eBPF cgroup tracking.
              Pod UIDs are resolved to names and namespaces via the Kubernetes API (cached, refreshed every 30s).
              Namespace power = sum of all pod power within the namespace.
              Non-pod processes (kernel threads, systemd) are not attributed to any pod,
              so the sum of pod power is always less than total node power.
            </p>
          </div>
        </ExpandableSection>
      </PageSection>

      <PageSection>
        {pods.length > 0 ? (() => {
          const cols: PodSortKey[] = ["pod_name", "node_name", "total_watts", "cpu_watts", "memory_watts", "gpu_watts", "storage_watts", "io_watts"];
          const sorted = [...pods].sort((a, b) => {
            const av = (a as any)[sortBy];
            const bv = (b as any)[sortBy];
            if (typeof av === "string") return sortDir === "asc" ? av.localeCompare(bv) : bv.localeCompare(av);
            return sortDir === "asc" ? av - bv : bv - av;
          });
          const getSortParams = (key: PodSortKey): ThProps["sort"] => ({
            sortBy: { index: cols.indexOf(sortBy), direction: sortDir },
            onSort: (_e, _idx, dir) => { setSortBy(key); setSortDir(dir as "asc" | "desc"); },
            columnIndex: cols.indexOf(key),
          });
          return (
            <Table aria-label="Pod power table" variant="compact">
              <Thead>
                <Tr>
                  <Th sort={getSortParams("pod_name")}>Pod</Th>
                  <Th sort={getSortParams("node_name")}>Node</Th>
                  <Th sort={getSortParams("total_watts")}>Total</Th>
                  <Th sort={getSortParams("cpu_watts")}>CPU</Th>
                  <Th sort={getSortParams("memory_watts")}>Memory</Th>
                  <Th sort={getSortParams("gpu_watts")}>GPU</Th>
                  <Th sort={getSortParams("storage_watts")}>Storage</Th>
                  <Th sort={getSortParams("io_watts")}>Network</Th>
                </Tr>
              </Thead>
              <Tbody>
                {sorted.map((pod) => (
                  <Tr key={pod.pod_uid}>
                    <Td>{pod.pod_name}</Td>
                    <Td>{pod.node_name}</Td>
                    <Td>{formatWatts(pod.total_watts)}</Td>
                    <Td>{formatWatts(pod.cpu_watts)}</Td>
                    <Td>{formatWatts(pod.memory_watts)}</Td>
                    <Td>{formatWatts(pod.gpu_watts)}</Td>
                    <Td>{formatWatts(pod.storage_watts)}</Td>
                    <Td>{formatWatts(pod.io_watts)}</Td>
                  </Tr>
                ))}
              </Tbody>
            </Table>
          );
        })() : (
          <EmptyState>
            <EmptyStateBody>No pods found in namespace {ns}.</EmptyStateBody>
          </EmptyState>
        )}
      </PageSection>
    </Page>
  );
};

export default NamespaceView;
