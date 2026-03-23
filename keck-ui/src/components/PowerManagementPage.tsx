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
  Gallery,
  GalleryItem,
  Label,
  Flex,
  FlexItem,
  Spinner,
  EmptyState,
  EmptyStateBody,
} from "@patternfly/react-core";
import {
  BoltIcon,
  LeafIcon,
  MoneyBillIcon,
  ServerIcon,
  CubesIcon,
} from "@patternfly/react-icons";
import { api, ClusterPower } from "../utils/api";
import { formatWatts, formatCarbon, formatCost, formatErrorRatio, errorStatus } from "../utils/format";

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
        <Gallery hasGutter minWidths={{ default: "250px" }}>
          {/* Cluster Total */}
          <GalleryItem>
            <Card isCompact>
              <CardTitle>
                <Flex>
                  <FlexItem><BoltIcon /></FlexItem>
                  <FlexItem>Cluster Total</FlexItem>
                </Flex>
              </CardTitle>
              <CardBody>
                <div style={{ fontSize: "2em", fontWeight: 600 }}>
                  {data.platform_watts > 0 ? formatWatts(data.platform_watts) : formatWatts(data.total_attributed_watts)}
                </div>
                <div style={{ marginTop: 4, fontSize: "0.85em", color: "var(--pf-v6-global--Color--200)" }}>
                  {data.platform_watts > 0 ? "Measured at PSU" : "Estimated (no PSU data)"}
                </div>
              </CardBody>
            </Card>
          </GalleryItem>

          {/* CPU Power */}
          <GalleryItem>
            <Card isCompact>
              <CardTitle>CPU</CardTitle>
              <CardBody>
                <div style={{ fontSize: "1.8em", fontWeight: 600, color: "#0066cc" }}>
                  {formatWatts(data.cpu_watts)}
                </div>
              </CardBody>
            </Card>
          </GalleryItem>

          {/* Memory Power */}
          <GalleryItem>
            <Card isCompact>
              <CardTitle>Memory</CardTitle>
              <CardBody>
                <div style={{ fontSize: "1.8em", fontWeight: 600, color: "#6753ac" }}>
                  {formatWatts(data.memory_watts)}
                </div>
              </CardBody>
            </Card>
          </GalleryItem>

          {/* GPU Power */}
          <GalleryItem>
            <Card isCompact>
              <CardTitle>GPU</CardTitle>
              <CardBody>
                <div style={{ fontSize: "1.8em", fontWeight: 600, color: "#3e8635" }}>
                  {formatWatts(data.gpu_watts)}
                </div>
              </CardBody>
            </Card>
          </GalleryItem>

          {/* Idle */}
          <GalleryItem>
            <Card isCompact>
              <CardTitle>Idle</CardTitle>
              <CardBody>
                <div style={{ fontSize: "1.8em", fontWeight: 600, color: "#8a8d90" }}>
                  {formatWatts(data.idle_watts)}
                </div>
              </CardBody>
            </Card>
          </GalleryItem>

          {/* Infrastructure */}
          <GalleryItem>
            <Card isCompact>
              <CardTitle>
                <Flex>
                  <FlexItem><ServerIcon /></FlexItem>
                  <FlexItem>Infrastructure</FlexItem>
                </Flex>
              </CardTitle>
              <CardBody>
                <div>{data.node_count} nodes</div>
                <div>{data.pod_count} pods</div>
                <div style={{ marginTop: 8 }}>
                  Accuracy:{" "}
                  <Label color={errorStatus(data.avg_error_ratio) === "success" ? "green" : errorStatus(data.avg_error_ratio) === "warning" ? "gold" : "red"}>
                    {formatErrorRatio(data.avg_error_ratio)}
                  </Label>
                </div>
              </CardBody>
            </Card>
          </GalleryItem>
        </Gallery>
      </PageSection>

      {/* Per-Node Breakdown */}
      {(data as any).nodes && (data as any).nodes.length > 0 && (
        <PageSection>
          <Card>
            <CardTitle>
              <Flex>
                <FlexItem><ServerIcon /></FlexItem>
                <FlexItem>Per-Node Power</FlexItem>
              </Flex>
            </CardTitle>
            <CardBody>
              <table style={{ width: "100%", borderCollapse: "collapse" }}>
                <thead>
                  <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                    <th style={{ textAlign: "left", padding: "8px" }}>Node</th>
                    <th style={{ textAlign: "right", padding: "8px" }}>Platform (PSU)</th>
                    <th style={{ textAlign: "right", padding: "8px" }}>CPU</th>
                    <th style={{ textAlign: "right", padding: "8px" }}>Memory</th>
                    <th style={{ textAlign: "right", padding: "8px" }}>GPU</th>
                    <th style={{ textAlign: "right", padding: "8px" }}>Pods</th>
                  </tr>
                </thead>
                <tbody>
                  {(data as any).nodes.map((node: any) => (
                    <tr key={node.node_name} style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
                      <td style={{ padding: "8px" }}>{node.node_name}</td>
                      <td style={{ padding: "8px", textAlign: "right", fontWeight: 600 }}>
                        {node.platform_watts ? formatWatts(node.platform_watts) : "N/A"}
                      </td>
                      <td style={{ padding: "8px", textAlign: "right", color: "#0066cc" }}>
                        {formatWatts(node.cpu_watts)}
                      </td>
                      <td style={{ padding: "8px", textAlign: "right", color: "#6753ac" }}>
                        {formatWatts(node.memory_watts)}
                      </td>
                      <td style={{ padding: "8px", textAlign: "right", color: "#3e8635" }}>
                        {formatWatts(node.gpu_watts)}
                      </td>
                      <td style={{ padding: "8px", textAlign: "right" }}>
                        {node.pod_count}
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
