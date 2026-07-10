// SPDX-License-Identifier: Apache-2.0

import * as React from "react";
import {
  Page,
  PageSection,
  Title,
  Card,
  CardTitle,
  CardBody,
  Label,
  Flex,
  FlexItem,
  Spinner,
  EmptyState,
  EmptyStateBody,
  ExpandableSection,
} from "@patternfly/react-core";
import { BoltIcon } from "@patternfly/react-icons";
import { api, ClusterPower, NamespacePower, NodeSummary } from "../../utils/api";
import { formatWatts } from "../../utils/format";
import { usePolling } from "../../utils/usePolling";

const cellStyle: React.CSSProperties = { padding: "8px", borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" };
const headerStyle: React.CSSProperties = { ...cellStyle, fontWeight: 600 };
const rightAlign: React.CSSProperties = { ...cellStyle, textAlign: "right" };

function deltaLabel(keck: number, kepler: number): React.ReactNode {
  if (keck === 0 && kepler === 0) return "-";
  const diff = keck - kepler;
  const pct = keck > 0 ? ((diff / keck) * 100) : 0;
  const color = Math.abs(pct) < 10 ? "green" : Math.abs(pct) < 25 ? "gold" : "red";
  const sign = diff > 0 ? "+" : "";
  return (
    <Label color={color} style={{ fontSize: "12px" }}>
      {sign}{formatWatts(diff)} ({sign}{pct.toFixed(0)}%)
    </Label>
  );
}

const KeplerComparisonView: React.FC = () => {
  const keckCluster = usePolling(() => api.getClusterPower("keck"), []);
  const keplerCluster = usePolling(() => api.getClusterPower("kepler"), []);
  const keckNs = usePolling(() => api.getNamespaces("keck"), []);
  const keplerNs = usePolling(() => api.getNamespaces("kepler"), []);
  const keckNodes = usePolling(() => api.getNodes("keck"), []);
  const keplerNodes = usePolling(() => api.getNodes("kepler"), []);

  const loading = keckCluster.loading || keplerCluster.loading;

  if (loading) {
    return (
      <Page>
        <PageSection><Spinner aria-label="Loading comparison data" /></PageSection>
      </Page>
    );
  }

  const kc = keckCluster.data;
  const kp = keplerCluster.data;

  if (!kc) {
    return (
      <Page>
        <PageSection>
          <EmptyState titleText="Kepler Comparison">
            <EmptyStateBody>No Keck data available. Ensure the Keck agent and controller are running.</EmptyStateBody>
          </EmptyState>
        </PageSection>
      </Page>
    );
  }

  const keplerAvailable = kp && kp.node_count > 0;

  const nsMap = new Map<string, { keck?: NamespacePower; kepler?: NamespacePower }>();
  (keckNs.data || []).forEach(ns => {
    nsMap.set(ns.namespace, { keck: ns });
  });
  (keplerNs.data || []).forEach(ns => {
    const existing = nsMap.get(ns.namespace) || {};
    nsMap.set(ns.namespace, { ...existing, kepler: ns });
  });
  const nsRows = Array.from(nsMap.entries())
    .map(([ns, d]) => ({ namespace: ns, keck: d.keck?.total_watts || 0, kepler: d.kepler?.total_watts || 0 }))
    .sort((a, b) => b.keck - a.keck);

  const nodeMap = new Map<string, { keck?: NodeSummary; kepler?: NodeSummary }>();
  (keckNodes.data || []).forEach(n => {
    nodeMap.set(n.node_name, { keck: n });
  });
  (keplerNodes.data || []).forEach(n => {
    const existing = nodeMap.get(n.node_name) || {};
    nodeMap.set(n.node_name, { ...existing, kepler: n });
  });
  const nodeRows = Array.from(nodeMap.entries())
    .map(([name, d]) => ({ name, keck: d.keck, kepler: d.kepler }));

  return (
    <Page>
      <PageSection>
        <Title headingLevel="h1" size="2xl">
          <BoltIcon /> Kepler Comparison
        </Title>
        <p style={{ marginTop: 8, color: "var(--pf-v6-global--Color--200)" }}>
          Side-by-side view of Keck hardware-metered data vs Kepler model-estimated data.
        </p>
      </PageSection>

      {!keplerAvailable && (
        <PageSection>
          <div style={{
            padding: "16px 20px",
            background: "var(--pf-v6-global--info-color--100, #2b9af3)",
            color: "#fff",
            borderRadius: 4,
          }}>
            Kepler data not available. Deploy Kepler and set <code>KEPLER_ENABLED=true</code> on the controller
            to enable side-by-side comparison.
          </div>
        </PageSection>
      )}

      {/* Cluster Totals */}
      <PageSection>
        <Card>
          <CardTitle>Cluster Totals</CardTitle>
          <CardBody>
            <table style={{ width: "100%", borderCollapse: "collapse" }}>
              <thead>
                <tr>
                  <th style={headerStyle}>Component</th>
                  <th style={{ ...headerStyle, textAlign: "right" }}>Keck</th>
                  <th style={{ ...headerStyle, textAlign: "right" }}>Kepler</th>
                  <th style={{ ...headerStyle, textAlign: "right" }}>Delta</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td style={cellStyle}>CPU</td>
                  <td style={rightAlign}>{formatWatts(kc.cpu_watts)}</td>
                  <td style={rightAlign}>{keplerAvailable ? formatWatts(kp!.cpu_watts) : "N/A"}</td>
                  <td style={rightAlign}>{keplerAvailable ? deltaLabel(kc.cpu_watts, kp!.cpu_watts) : "-"}</td>
                </tr>
                <tr>
                  <td style={cellStyle}>GPU</td>
                  <td style={rightAlign}>{formatWatts(kc.gpu_watts)}</td>
                  <td style={rightAlign}>{keplerAvailable ? formatWatts(kp!.gpu_watts) : "N/A"}</td>
                  <td style={rightAlign}>{keplerAvailable ? deltaLabel(kc.gpu_watts, kp!.gpu_watts) : "-"}</td>
                </tr>
                <tr>
                  <td style={cellStyle}>Memory</td>
                  <td style={rightAlign}>{formatWatts(kc.memory_watts)}</td>
                  <td style={rightAlign}><span style={{ color: "var(--pf-v6-global--Color--200)" }}>not provided</span></td>
                  <td style={rightAlign}>-</td>
                </tr>
                <tr>
                  <td style={cellStyle}>Platform (PSU)</td>
                  <td style={rightAlign}>{kc.platform_watts > 0 ? formatWatts(kc.platform_watts) : "N/A"}</td>
                  <td style={rightAlign}>{keplerAvailable && kp!.platform_watts > 0 ? formatWatts(kp!.platform_watts) : "N/A"}</td>
                  <td style={rightAlign}>{keplerAvailable && kc.platform_watts > 0 && kp!.platform_watts > 0 ? deltaLabel(kc.platform_watts, kp!.platform_watts) : "-"}</td>
                </tr>
                <tr>
                  <td style={cellStyle}>Nodes</td>
                  <td style={rightAlign}>{kc.node_count}</td>
                  <td style={rightAlign}>{keplerAvailable ? kp!.node_count : "-"}</td>
                  <td style={rightAlign}></td>
                </tr>
                <tr>
                  <td style={cellStyle}>Pods</td>
                  <td style={rightAlign}>{kc.pod_count}</td>
                  <td style={rightAlign}>{keplerAvailable ? kp!.pod_count : "-"}</td>
                  <td style={rightAlign}></td>
                </tr>
              </tbody>
            </table>
          </CardBody>
        </Card>
      </PageSection>

      {/* Per-Namespace Comparison */}
      {keplerAvailable && nsRows.length > 0 && (
        <PageSection>
          <Card>
            <CardTitle>Per-Namespace Comparison</CardTitle>
            <CardBody>
              <table style={{ width: "100%", borderCollapse: "collapse" }}>
                <thead>
                  <tr>
                    <th style={headerStyle}>Namespace</th>
                    <th style={{ ...headerStyle, textAlign: "right" }}>Keck (W)</th>
                    <th style={{ ...headerStyle, textAlign: "right" }}>Kepler (W)</th>
                    <th style={{ ...headerStyle, textAlign: "right" }}>Delta</th>
                  </tr>
                </thead>
                <tbody>
                  {nsRows.slice(0, 30).map(row => (
                    <tr key={row.namespace}>
                      <td style={cellStyle}>{row.namespace}</td>
                      <td style={rightAlign}>{formatWatts(row.keck)}</td>
                      <td style={rightAlign}>{formatWatts(row.kepler)}</td>
                      <td style={rightAlign}>{deltaLabel(row.keck, row.kepler)}</td>
                    </tr>
                  ))}
                  {nsRows.length > 30 && (
                    <tr>
                      <td style={cellStyle} colSpan={4}>
                        <span style={{ color: "var(--pf-v6-global--Color--200)" }}>
                          ... and {nsRows.length - 30} more namespaces
                        </span>
                      </td>
                    </tr>
                  )}
                </tbody>
              </table>
            </CardBody>
          </Card>
        </PageSection>
      )}

      {/* Per-Node Comparison */}
      {keplerAvailable && nodeRows.length > 0 && (
        <PageSection>
          <Card>
            <CardTitle>Per-Node Comparison</CardTitle>
            <CardBody>
              <table style={{ width: "100%", borderCollapse: "collapse" }}>
                <thead>
                  <tr>
                    <th style={headerStyle}>Node</th>
                    <th style={{ ...headerStyle, textAlign: "right" }}>Keck CPU</th>
                    <th style={{ ...headerStyle, textAlign: "right" }}>Kepler CPU</th>
                    <th style={{ ...headerStyle, textAlign: "right" }}>Keck GPU</th>
                    <th style={{ ...headerStyle, textAlign: "right" }}>Kepler GPU</th>
                    <th style={{ ...headerStyle, textAlign: "right" }}>Delta (total)</th>
                  </tr>
                </thead>
                <tbody>
                  {nodeRows.map(row => {
                    const keckTotal = (row.keck?.cpu_watts || 0) + (row.keck?.gpu_watts || 0);
                    const keplerTotal = (row.kepler?.cpu_watts || 0) + (row.kepler?.gpu_watts || 0);
                    return (
                      <tr key={row.name}>
                        <td style={cellStyle}>{row.name}</td>
                        <td style={rightAlign}>{row.keck ? formatWatts(row.keck.cpu_watts) : "-"}</td>
                        <td style={rightAlign}>{row.kepler ? formatWatts(row.kepler.cpu_watts) : "-"}</td>
                        <td style={rightAlign}>{row.keck ? formatWatts(row.keck.gpu_watts) : "-"}</td>
                        <td style={rightAlign}>{row.kepler ? formatWatts(row.kepler.gpu_watts) : "-"}</td>
                        <td style={rightAlign}>{deltaLabel(keckTotal, keplerTotal)}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </CardBody>
          </Card>
        </PageSection>
      )}

      {/* Methodology */}
      <PageSection>
        <Card>
          <CardBody>
            <ExpandableSection toggleText="Methodology">
              <div style={{ fontSize: "0.9em", lineHeight: 1.6, color: "var(--pf-v6-global--Color--200)" }}>
                <p><strong>Keck</strong> reads hardware power sensors (RAPL energy counters, Redfish PSU readings, DCGM GPU power) and attributes per-pod energy using eBPF-observed scheduling data weighted by CPU frequency and hardware counters (instructions, cycles, LLC misses).</p>
                <p style={{ marginTop: 8 }}><strong>Kepler</strong> estimates power from CPU performance counters using trained regression models. It does not read hardware power sensors directly on most configurations.</p>
                <p style={{ marginTop: 8 }}><strong>Why they differ:</strong> Different measurement methodology (hardware sensors vs model estimation), different attribution granularity (per-core frequency-weighted vs counter-based), and different component coverage (Kepler does not provide memory power attribution).</p>
                <p style={{ marginTop: 8 }}><strong>Delta interpretation:</strong> Positive delta means Keck reads higher than Kepler. Neither is necessarily "correct" -- the comparison helps validate both approaches. When Keck has PSU ground truth (Redfish), its total is measured; Kepler's total is always estimated.</p>
              </div>
            </ExpandableSection>
          </CardBody>
        </Card>
      </PageSection>
    </Page>
  );
};

export default KeplerComparisonView;
