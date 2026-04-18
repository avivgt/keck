// SPDX-License-Identifier: Apache-2.0

// Main Power Management page — the entry point in the OpenShift console.
// Accessible via the "Power Management" section in the left nav.

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
} from "@patternfly/react-core";
import {
  BoltIcon,
  ServerIcon,
} from "@patternfly/react-icons";
import { ChartDonut } from "@patternfly/react-charts";
import { api, ClusterPower } from "../utils/api";
import { formatWatts, formatErrorRatio, errorStatus } from "../utils/format";

const PowerManagementPage: React.FC = () => {
  const [data, setData] = React.useState<ClusterPower | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    const fetchData = () => {
      api.getClusterPower()
        .then(setData)
        .catch((e) => setError(e.message))
        .finally(() => setLoading(false));
    };

    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, []);

  if (loading) {
    return (
      <Page>
        <PageSection>
          <Spinner aria-label="Loading power data" />
        </PageSection>
      </Page>
    );
  }

  if (error || !data) {
    return (
      <Page>
        <PageSection>
          <EmptyState>
            <Title headingLevel="h2" size="lg">Power Management</Title>
            <EmptyStateBody>
              {error || "No power data available. Ensure Keck agent and controller are running."}
            </EmptyStateBody>
          </EmptyState>
        </PageSection>
      </Page>
    );
  }

  return (
    <Page>
      <PageSection>
        <Title headingLevel="h1" size="2xl">
          <BoltIcon /> Power Management
        </Title>
        <p style={{ marginTop: 8, color: "var(--pf-v6-global--Color--200)" }}>
          Real-time power consumption, carbon emissions, and cost across the cluster.
          Data refreshes every 5 seconds.
        </p>
      </PageSection>

      <PageSection>
        <Flex>
          {/* Pie Chart */}
          <FlexItem style={{ flex: "0 0 350px" }}>
            <Card>
              <CardTitle>
                <Flex>
                  <FlexItem><BoltIcon /></FlexItem>
                  <FlexItem>Cluster Power Breakdown</FlexItem>
                </Flex>
              </CardTitle>
              <CardBody>
                <div style={{ height: 275 }}>
                  <ChartDonut
                    constrainToVisibleArea
                    data={[
                      { x: "CPU", y: data.cpu_watts },
                      { x: "Memory", y: data.memory_watts },
                      { x: "GPU", y: data.gpu_watts },
                      { x: "Idle/Other", y: data.idle_watts },
                    ].filter(d => d.y > 0)}
                    labels={({ datum }) => `${datum.x}: ${formatWatts(datum.y)}`}
                    colorScale={["#0066cc", "#6753ac", "#3e8635", "#8a8d90"]}
                    title={data.platform_watts > 0 ? formatWatts(data.platform_watts) : formatWatts(data.total_attributed_watts)}
                    subTitle={data.platform_watts > 0 ? "PSU Measured" : "Estimated"}
                    padding={{ bottom: 20, left: 20, right: 20, top: 20 }}
                    innerRadius={80}
                    titleComponent={
                      React.createElement("text", {
                        x: "50%", y: "42%",
                        textAnchor: "middle",
                        dominantBaseline: "central",
                        style: { fill: "#e4e4e7", fontSize: 22, fontWeight: 700 }
                      }, data.platform_watts > 0 ? formatWatts(data.platform_watts) : formatWatts(data.total_attributed_watts))
                    }
                    subTitleComponent={
                      React.createElement("text", {
                        x: "50%", y: "52%",
                        textAnchor: "middle",
                        dominantBaseline: "central",
                        style: { fill: "#a1a1aa", fontSize: 13 }
                      }, data.platform_watts > 0 ? "PSU Measured" : "Estimated")
                    }
                  />
                </div>
              </CardBody>
            </Card>
          </FlexItem>

          {/* Summary Stats */}
          <FlexItem style={{ flex: 1 }}>
            <Card>
              <CardTitle>
                <Flex>
                  <FlexItem><ServerIcon /></FlexItem>
                  <FlexItem>Cluster Summary</FlexItem>
                </Flex>
              </CardTitle>
              <CardBody>
                <table style={{ width: "100%", borderCollapse: "collapse" }}>
                  <tbody>
                    <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                      <td style={{ padding: "10px 8px", fontWeight: 600 }}>Cluster Total</td>
                      <td style={{ padding: "10px 8px", textAlign: "right", fontSize: "1.3em", fontWeight: 700 }}>
                        {data.platform_watts > 0 ? formatWatts(data.platform_watts) : formatWatts(data.total_attributed_watts)}
                      </td>
                      <td style={{ padding: "10px 8px", fontSize: "0.85em", color: "var(--pf-v6-global--Color--200)" }}>
                        {data.platform_watts > 0 ? "PSU measured" : "estimated"}
                      </td>
                    </tr>
                    <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                      <td style={{ padding: "10px 8px", color: "#0066cc" }}>CPU</td>
                      <td style={{ padding: "10px 8px", textAlign: "right", fontWeight: 600 }}>{formatWatts(data.cpu_watts)}</td>
                      <td style={{ padding: "10px 8px", fontSize: "0.85em", color: "var(--pf-v6-global--Color--200)" }}>
                        {data.platform_watts > 0 ? `${((data.cpu_watts / data.platform_watts) * 100).toFixed(0)}%` : ""}
                      </td>
                    </tr>
                    <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                      <td style={{ padding: "10px 8px", color: "#6753ac" }}>Memory</td>
                      <td style={{ padding: "10px 8px", textAlign: "right", fontWeight: 600 }}>{formatWatts(data.memory_watts)}</td>
                      <td style={{ padding: "10px 8px", fontSize: "0.85em", color: "var(--pf-v6-global--Color--200)" }}>
                        {data.platform_watts > 0 ? `${((data.memory_watts / data.platform_watts) * 100).toFixed(0)}%` : ""}
                      </td>
                    </tr>
                    <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                      <td style={{ padding: "10px 8px", color: "#3e8635" }}>GPU</td>
                      <td style={{ padding: "10px 8px", textAlign: "right", fontWeight: 600 }}>{formatWatts(data.gpu_watts)}</td>
                      <td style={{ padding: "10px 8px", fontSize: "0.85em", color: "var(--pf-v6-global--Color--200)" }}>
                        {data.platform_watts > 0 ? `${((data.gpu_watts / data.platform_watts) * 100).toFixed(0)}%` : ""}
                      </td>
                    </tr>
                    <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                      <td style={{ padding: "10px 8px", color: "#8a8d90" }}>Idle / Other</td>
                      <td style={{ padding: "10px 8px", textAlign: "right", fontWeight: 600 }}>{formatWatts(data.idle_watts)}</td>
                      <td style={{ padding: "10px 8px", fontSize: "0.85em", color: "var(--pf-v6-global--Color--200)" }}>
                        {data.platform_watts > 0 ? `${((data.idle_watts / data.platform_watts) * 100).toFixed(0)}%` : ""}
                      </td>
                    </tr>
                    <tr>
                      <td style={{ padding: "10px 8px" }}>Infrastructure</td>
                      <td style={{ padding: "10px 8px", textAlign: "right" }}>{data.node_count} nodes, {data.pod_count} pods</td>
                      <td style={{ padding: "10px 8px" }}>
                        <Label color={errorStatus(data.avg_error_ratio) === "success" ? "green" : errorStatus(data.avg_error_ratio) === "warning" ? "gold" : "red"}>
                          RAPL {formatErrorRatio(data.avg_error_ratio)}
                        </Label>
                      </td>
                    </tr>
                  </tbody>
                </table>
              </CardBody>
            </Card>
          </FlexItem>
        </Flex>
      </PageSection>

      {/* Data Sources */}
      {(data as any).sources && (data as any).sources.length > 0 && (
        <PageSection>
          <Card>
            <CardTitle>Data Sources</CardTitle>
            <CardBody>
              <p style={{ marginBottom: 12, fontSize: "0.9em", color: "var(--pf-v6-global--Color--200)" }}>
                All discovered power sources. The most accurate available source is automatically
                selected per component (Measured &gt; Estimated).
              </p>
              <table style={{ width: "100%", borderCollapse: "collapse" }}>
                <thead>
                  <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                    <th style={{ textAlign: "left", padding: "8px" }}>Source</th>
                    <th style={{ textAlign: "left", padding: "8px" }}>Node</th>
                    <th style={{ textAlign: "left", padding: "8px" }}>Component</th>
                    <th style={{ textAlign: "left", padding: "8px" }}>Type</th>
                    <th style={{ textAlign: "right", padding: "8px" }}>Reading</th>
                    <th style={{ textAlign: "center", padding: "8px" }}>Available</th>
                    <th style={{ textAlign: "center", padding: "8px" }}>Selected</th>
                  </tr>
                </thead>
                <tbody>
                  {(data as any).sources.map((src: any, i: number) => (
                    <tr key={i} style={{
                      borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)",
                      opacity: src.available ? 1 : 0.5,
                      fontWeight: src.selected ? 600 : 400,
                    }}>
                      <td style={{ padding: "8px" }}>{src.name}</td>
                      <td style={{ padding: "8px", fontSize: "0.85em" }}>{src.node_name || "—"}</td>
                      <td style={{ padding: "8px" }}>
                        {src.component === "cpu" ? "CPU" : src.component === "gpu" ? "GPU" : src.component.charAt(0).toUpperCase() + src.component.slice(1)}
                      </td>
                      <td style={{ padding: "8px" }}>
                        <Label
                          color={src.reading_type === "measured" ? "green" : src.reading_type === "estimated" ? "gold" : "red"}
                          style={{ fontSize: "12px", fontWeight: 500, minWidth: "75px", textAlign: "center" }}
                        >
                          {src.reading_type}
                        </Label>
                      </td>
                      <td style={{ padding: "8px", textAlign: "right" }}>
                        {src.available ? formatWatts(src.watts) : "—"}
                      </td>
                      <td style={{ padding: "8px", textAlign: "center" }}>
                        {src.available ? "\u2705" : "\u274C"}
                      </td>
                      <td style={{ padding: "8px", textAlign: "center" }}>
                        {src.selected ? "\u2B50" : ""}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </CardBody>
          </Card>
        </PageSection>
      )}



      {/* Data Quality & Alerts */}
      {(data as any).data_quality && (
        <PageSection>
          <Card>
            <CardTitle>Data Quality</CardTitle>
            <CardBody>
              <table style={{ width: "100%", borderCollapse: "collapse" }}>
                <thead>
                  <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                    <th style={{ textAlign: "left", padding: "8px" }}>Component</th>
                    <th style={{ textAlign: "left", padding: "8px" }}>Source</th>
                    <th style={{ textAlign: "left", padding: "8px" }}>Type</th>
                    <th style={{ textAlign: "left", padding: "8px" }}>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {["cpu", "memory", "gpu", "platform"].map((comp) => {
                    const q = (data as any).data_quality[comp];
                    if (!q) return null;
                    return (
                      <tr key={comp} style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                        <td style={{ padding: "8px" }}>{comp === "cpu" ? "CPU" : comp === "gpu" ? "GPU" : comp.charAt(0).toUpperCase() + comp.slice(1)}</td>
                        <td style={{ padding: "8px" }}>{q.source}</td>
                        <td style={{ padding: "8px" }}>
                          <Label color={q.type === "measured" ? "green" : q.type === "estimated" ? "gold" : "red"}>
                            {q.type}
                          </Label>
                        </td>
                        <td style={{ padding: "8px", fontSize: "0.9em", color: "var(--pf-v6-global--Color--200)" }}>
                          {q.note}
                        </td>
                      </tr>
                    );
                  })}
                  <tr>
                    <td style={{ padding: "8px" }}>Attribution</td>
                    <td style={{ padding: "8px" }}>{(data as any).data_quality.attribution?.method}</td>
                    <td style={{ padding: "8px" }}>
                      <Label color="blue">active</Label>
                    </td>
                    <td style={{ padding: "8px", fontSize: "0.9em", color: "var(--pf-v6-global--Color--200)" }}>
                      {(data as any).data_quality.attribution?.note}
                    </td>
                  </tr>
                </tbody>
              </table>

              {(data as any).data_quality.alerts?.missing_ground_truth && (
                <div style={{
                  marginTop: 16,
                  padding: "12px 16px",
                  background: "var(--pf-v6-global--warning-color--100, #f0ab00)",
                  color: "#000",
                  borderRadius: 4,
                  fontSize: "0.9em"
                }}>
                  <strong>Warning:</strong> {(data as any).data_quality.alerts.message}
                </div>
              )}
            </CardBody>
          </Card>
        </PageSection>
      )}
    </Page>
  );
};

export default PowerManagementPage;
