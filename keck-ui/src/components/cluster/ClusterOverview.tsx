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
              <p><strong>1. Node-level power measurement</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                Each node reads power from the best available source (auto-selected):
                Redfish BMC sensors (measured, preferred) or Intel RAPL energy counters (estimated, fallback).
                Platform total comes from PSU input power via Redfish.
              </p>

              <p style={{ marginTop: 12 }}><strong>2. Per-process CPU attribution</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                The agent reads <code>/proc/[pid]/stat</code> for every process on the node,
                computing the CPU time delta (utime + stime) between collection intervals.
                Threads (LWPs) are filtered out via <code>/proc/[pid]/status</code> Tgid check
                to prevent double-counting.
                Each pod's CPU power = node CPU power × (pod CPU time delta / total CPU time delta across all processes).
              </p>

              <p style={{ marginTop: 12 }}><strong>3. Per-process memory attribution</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                The agent reads <code>/proc/[pid]/statm</code> for RSS (Resident Set Size).
                Each pod's memory power = node memory power × (pod RSS / total RSS across all processes).
                Pods holding more memory in RAM get proportionally more memory power attributed.
              </p>

              <p style={{ marginTop: 12 }}><strong>4. Pod → Namespace aggregation</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                Processes are mapped to pods via cgroup v2 paths in <code>/proc/[pid]/cgroup</code>.
                Pod UIDs are resolved to names and namespaces via the Kubernetes API.
                Namespace power = sum of all pod power within the namespace.
              </p>

              <p style={{ marginTop: 12 }}><strong>5. eBPF kernel observation (active)</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                eBPF programs attached to <code>sched_switch</code> and <code>cpu_frequency</code>
                tracepoints collect per-PID per-core CPU time with nanosecond precision
                and per-core frequency transitions. This data is collected for future
                per-core frequency-weighted attribution.
              </p>

              <p style={{ marginTop: 12 }}><strong>6. Source priority</strong></p>
              <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
                When multiple sources are available for the same component (e.g., Redfish CPU
                and RAPL CPU), the agent automatically selects the most accurate:
                Measured (Redfish/hwmon) {">"} Estimated (RAPL) {">"} Unavailable.
                All sources and their selection status are shown in the Overview → Data Sources table.
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
