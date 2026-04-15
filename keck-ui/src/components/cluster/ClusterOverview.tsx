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
import { api, NamespacePower } from "../../utils/api";
import { formatWatts } from "../../utils/format";

type SortKey = "namespace" | "total_watts" | "cpu_watts" | "memory_watts" | "gpu_watts" | "storage_watts" | "io_watts" | "pod_count";

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

      {/* Attribution Methodology — same content as NamespaceView */}
      <PageSection>
        <ExpandableSection toggleText="How Power is Calculated" isIndented>
          <div style={{ fontSize: "0.9em", lineHeight: 1.7 }}>

            <p style={{ color: "var(--pf-v6-global--Color--200)" }}>
              A server consumes hundreds of watts, but hardware only measures power at the component
              level (total CPU, total DRAM, total PSU). There is no per-pod power meter. Keck estimates
              each pod's share using observable signals from the kernel and hardware.
            </p>

            <p style={{ marginTop: 16 }}><strong>1. Hardware power measurement</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              Each node's agent reads real power data from the server's management controller (BMC).
            </p>
            <ul style={{ marginLeft: 32, color: "var(--pf-v6-global--Color--200)" }}>
              <li><strong>Redfish</strong> — industry-standard BMC API that exposes per-subsystem power sensors (CPU, Memory, Storage, Fans, PSU).</li>
              <li><strong>RAPL</strong> (Running Average Power Limit) — Intel CPU-internal energy counters. Used as fallback when Redfish sensors are unavailable.</li>
              <li><strong>MetricReports</strong> — BMC telemetry feature that provides periodic power readings for storage and PCIe subsystems.</li>
              <li><strong>Measured {">"} Estimated</strong> — Redfish hardware sensors are always preferred over RAPL software estimates.</li>
            </ul>

            <p style={{ marginTop: 16 }}><strong>2. CPU power attribution</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              The node draws (for example) 130W of CPU power. 80 pods share those CPUs. The agent
              determines each pod's share using this formula:
            </p>
            <p style={{ marginLeft: 16, marginTop: 8, fontFamily: "monospace", fontSize: "0.95em" }}>
              weight = time × freq² × (1 + α × IPC + β × cache_miss_ratio)
            </p>
            <p style={{ marginLeft: 16, marginTop: 4, fontFamily: "monospace", fontSize: "0.95em" }}>
              pod CPU power = core power × (pod weight / sum of all weights on that core)
            </p>

            <p style={{ marginLeft: 16, marginTop: 12, color: "var(--pf-v6-global--Color--200)" }}>
              <strong>Why not just use CPU time?</strong> Because two pods using the same CPU time can consume
              very different power. One might run dense matrix math (high frequency, all execution units active),
              while the other is mostly stalled waiting for memory. CPU-time-only attribution treats both
              equally — Keck's model captures the difference.
            </p>

            <p style={{ marginLeft: 16, marginTop: 12, color: "var(--pf-v6-global--Color--200)" }}>
              Each term in the formula:
            </p>
            <ul style={{ marginLeft: 32, color: "var(--pf-v6-global--Color--200)" }}>
              <li><strong>time</strong> — how long the process ran on that core (nanoseconds). Measured via the <strong>sched_switch</strong> kernel
                tracepoint, which fires every time one process replaces another on a CPU core. The agent uses eBPF to capture this in-kernel with no polling overhead.</li>
              <li><strong>freq²</strong> — CPU power scales with the square of frequency (from circuit physics: P = C × V² × f).
                A process running at 3.5 GHz uses ~3× more power than one at 1.2 GHz for the same duration.
                Measured via the <strong>cpu_frequency</strong> kernel tracepoint, which fires when a core changes clock speed.</li>
              <li><strong>1</strong> — the baseline. Even a completely stalled process (IPC = 0, no cache misses) still
                consumes power — the core is clocked, transistors maintain state, leakage current flows. The "1"
                ensures a stalled process still gets a baseline share of core power.</li>
              <li><strong>α × IPC</strong> — <strong>IPC</strong> (Instructions Per Cycle) measures compute density. A process
                retiring 3 instructions per cycle switches more transistors per tick than one retiring 0.5 (waiting
                on memory). More switching = more dynamic power. <strong>α (default 0.3)</strong> controls how strongly
                this affects the power split. At α = 0.3, a 6× difference in IPC produces a ~1.6× difference in
                attributed power. Without α, the IPC effect would be too aggressive (2.7× for the same example).</li>
              <li><strong>β × cache_miss_ratio</strong> — every cache miss triggers a DRAM access through the memory controller
                and data bus, which costs power. <strong>cache_miss_ratio</strong> = cache misses / instructions
                (typically 0.001–0.05). <strong>β (default 1.5)</strong> produces a small adjustment (1.5–7.5%) because
                cache misses affect DRAM power more than core power — and DRAM is handled separately in section 3.</li>
            </ul>

            <p style={{ marginLeft: 16, marginTop: 12, color: "var(--pf-v6-global--Color--200)" }}>
              <strong>Energy conservation guarantee:</strong> after computing weights, the agent normalizes so that
              the sum of all process power on a core equals the core's measured power from RAPL. No energy
              is created or lost — the model only affects how the measured total is distributed.
            </p>
            <p style={{ marginLeft: 16, marginTop: 4, color: "var(--pf-v6-global--Color--200)" }}>
              <strong>Coefficients:</strong> α and β are configurable via <code>KECK_ALPHA</code> / <code>KECK_BETA</code> environment
              variables and can be auto-tuned at runtime (<code>KECK_AUTO_TUNE=true</code>) or calibrated offline
              with controlled workloads.
            </p>

            <p style={{ marginTop: 16 }}><strong>3. Memory (DRAM) power attribution</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              The node draws (for example) 14W of DRAM power. The agent splits this across pods based on
              how much memory they hold and how actively they use it.
            </p>
            <p style={{ marginLeft: 16, marginTop: 8, fontFamily: "monospace", fontSize: "0.95em" }}>
              pod memory power = node memory power × (0.6 × pod_PSS/total_PSS + 0.4 × pod_LLC/total_LLC)
            </p>
            <p style={{ marginLeft: 16, marginTop: 12, color: "var(--pf-v6-global--Color--200)" }}>
              <strong>Why two signals?</strong> A pod holding 8 GB of cached data but never accessing it should not
              be charged the same as a pod holding 8 GB and actively streaming through it. PSS captures
              how much memory is held; LLC misses capture how actively it's being used.
            </p>
            <ul style={{ marginLeft: 32, color: "var(--pf-v6-global--Color--200)" }}>
              <li><strong>PSS (60%)</strong> — Proportional Set Size: the amount of physical memory a process uses,
                with shared pages (libc, etc.) split fairly among all users. Read from <code>/proc/[pid]/smaps_rollup</code>.
                Captures DRAM refresh cost — chips must periodically refresh every cell holding data.</li>
              <li><strong>LLC misses (40%)</strong> — each Last-Level Cache miss triggers a DRAM read or write.
                Pods streaming large data sets (ML inference, in-memory databases) cause more DRAM activity
                and get charged more dynamic memory power.</li>
            </ul>

            <p style={{ marginTop: 16 }}><strong>4. Storage power attribution</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              Storage subsystem power (SSDs, HDDs) is measured from the BMC via Redfish MetricReports
              and distributed to pods proportionally to their disk I/O activity.
            </p>
            <p style={{ marginLeft: 16, marginTop: 8, fontFamily: "monospace", fontSize: "0.95em" }}>
              pod storage power = node storage power × (pod_io_bytes / total_io_bytes)
            </p>
            <ul style={{ marginLeft: 32, color: "var(--pf-v6-global--Color--200)" }}>
              <li><strong>/proc/[pid]/io</strong> — kernel file that tracks actual disk bytes read and written per
                process (bypassing page cache — only real disk I/O counts).</li>
              <li><strong>Why it can show 0</strong> — if no pods are actively doing disk I/O during the 10-second
                measurement interval, all pods get 0W storage. The node's SSDs still draw power (idle), but
                idle storage power is a node-level cost not attributable to any specific pod.</li>
            </ul>

            <p style={{ marginTop: 16 }}><strong>5. Network power attribution</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              Network interface power is measured from the BMC and distributed to pods based on
              their TCP traffic volume.
            </p>
            <p style={{ marginLeft: 16, marginTop: 8, fontFamily: "monospace", fontSize: "0.95em" }}>
              pod network power = node NIC power × (pod_tcp_bytes / total_tcp_bytes)
            </p>
            <ul style={{ marginLeft: 32, color: "var(--pf-v6-global--Color--200)" }}>
              <li><strong>tcp_sendmsg / tcp_recvmsg</strong> — kernel functions called when a process sends or receives
                TCP data. The agent uses eBPF kprobes to track bytes per process without polling.</li>
              <li><strong>kretprobe</strong> — for <code>tcp_recvmsg</code>, the agent reads the function's return value
                (actual bytes received) rather than the requested buffer size, for accuracy.</li>
              <li>Falls back to CPU-ratio-based attribution when TCP kprobes are unavailable.</li>
            </ul>

            <p style={{ marginTop: 16 }}><strong>6. GPU power (DCGM)</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              GPU power is the most accurate component — it's measured directly per-pod from hardware, no estimation needed.
            </p>
            <ul style={{ marginLeft: 32, color: "var(--pf-v6-global--Color--200)" }}>
              <li><strong>DCGM</strong> (Data Center GPU Manager) — NVIDIA's monitoring tool that reports per-GPU watt
                readings with pod name and namespace labels. The agent auto-discovers the DCGM exporter on each node.</li>
            </ul>

            <p style={{ marginTop: 16 }}><strong>7. Ground truth reconciliation</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              The agent cross-checks everything against the PSU total power measured by Redfish:
            </p>
            <p style={{ marginLeft: 16, marginTop: 8, fontFamily: "monospace", fontSize: "0.95em" }}>
              error = |PSU total - (CPU + Memory + GPU + Storage + NIC + Fans)| / PSU total
            </p>
            <ul style={{ marginLeft: 32, color: "var(--pf-v6-global--Color--200)" }}>
              <li>An error ratio of 0.10 means 10% of server power is unaccounted for (motherboard chipset,
                voltage regulators, clock distribution).</li>
              <li>The error ratio is shown in the dashboard so you know how complete the measurement is.</li>
            </ul>

            <p style={{ marginTop: 16 }}><strong>8. Pod → Namespace aggregation</strong></p>
            <p style={{ marginLeft: 16, color: "var(--pf-v6-global--Color--200)" }}>
              Namespace power = sum of all pod power in that namespace. Non-pod processes (kernel, systemd)
              are not attributed to any pod, so the sum of all pods is always less than total node power.
            </p>
            <ul style={{ marginLeft: 32, color: "var(--pf-v6-global--Color--200)" }}>
              <li><strong>cgroup v2</strong> — Linux kernel feature that groups processes into containers. The agent
                reads <code>/proc/[pid]/cgroup</code> to map each process to its pod.</li>
            </ul>
          </div>
        </ExpandableSection>
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

          const cols: SortKey[] = ["namespace", "total_watts", "cpu_watts", "memory_watts", "gpu_watts", "storage_watts", "io_watts", "pod_count"];
          const getSortParams = (key: SortKey): ThProps["sort"] => ({
            sortBy: {
              index: cols.indexOf(sortBy),
              direction: sortDir,
            },
            onSort: (_e, _idx, dir) => {
              setSortBy(key);
              setSortDir(dir as "asc" | "desc");
            },
            columnIndex: cols.indexOf(key),
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
                  <Th sort={getSortParams("storage_watts")}>Storage</Th>
                  <Th sort={getSortParams("io_watts")}>Network</Th>
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
                    <Td>{formatWatts(ns.storage_watts || 0)}</Td>
                    <Td>{formatWatts(ns.io_watts || 0)}</Td>
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
