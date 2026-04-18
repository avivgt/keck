// SPDX-License-Identifier: Apache-2.0

// Per-node power breakdown. Shows all nodes with power, sources, and headroom.

import * as React from "react";
import {
  Page,
  PageSection,
  Title,
  Spinner,
  EmptyState,
  EmptyStateBody,
  Label,
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
import { api, NodeSummary } from "../../utils/api";
import { formatWatts } from "../../utils/format";

type SortKey = "node_name" | "platform_watts" | "cpu_watts" | "memory_watts" | "gpu_watts" | "idle_watts" | "error_ratio" | "pod_count";

const NodesView: React.FC = () => {
  const [nodes, setNodes] = React.useState<NodeSummary[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [sortBy, setSortBy] = React.useState<SortKey>("platform_watts");
  const [sortDir, setSortDir] = React.useState<"asc" | "desc">("desc");

  React.useEffect(() => {
    const fetchData = () => {
      api.getNodes()
        .then(setNodes)
        .finally(() => setLoading(false));
    };
    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, []);

  if (loading) {
    return <Page><PageSection><Spinner /></PageSection></Page>;
  }

  const totalPlatform = nodes.reduce((sum, n) => sum + (n.platform_watts || 0), 0);

  return (
    <Page>
      <PageSection>
        <Title headingLevel="h1" size="xl">Nodes</Title>
        <p style={{ marginTop: 4, color: "var(--pf-v6-global--Color--200)" }}>
          {nodes.length} nodes, {formatWatts(totalPlatform)} total (PSU measured).
        </p>
      </PageSection>

      <PageSection>
        {nodes.length > 0 ? (() => {
          const cols: SortKey[] = ["node_name", "platform_watts", "cpu_watts", "memory_watts", "gpu_watts", "idle_watts", "error_ratio", "pod_count"];
          const sorted = [...nodes].sort((a, b) => {
            const av = (a as any)[sortBy] ?? 0;
            const bv = (b as any)[sortBy] ?? 0;
            if (typeof av === "string") return sortDir === "asc" ? av.localeCompare(bv) : bv.localeCompare(av);
            return sortDir === "asc" ? av - bv : bv - av;
          });

          const getSortParams = (key: SortKey): ThProps["sort"] => ({
            sortBy: { index: cols.indexOf(sortBy), direction: sortDir },
            onSort: (_e, _idx, dir) => { setSortBy(key); setSortDir(dir as "asc" | "desc"); },
            columnIndex: cols.indexOf(key),
          });

          return (
            <Table aria-label="Node power table" variant="compact">
              <Thead>
                <Tr>
                  <Th sort={getSortParams("node_name")}>Node</Th>
                  <Th sort={getSortParams("platform_watts")}>PSU Total</Th>
                  <Th sort={getSortParams("cpu_watts")}>CPU</Th>
                  <Th sort={getSortParams("memory_watts")}>Memory</Th>
                  <Th sort={getSortParams("gpu_watts")}>GPU</Th>
                  <Th sort={getSortParams("idle_watts")}>Idle / Other</Th>
                  <Th sort={getSortParams("error_ratio")}>Error</Th>
                  <Th sort={getSortParams("pod_count")}>Pods</Th>
                  <Th>CPU Method</Th>
                </Tr>
              </Thead>
              <Tbody>
                {sorted.map((node) => (
                  <Tr key={node.node_name}>
                    <Td>{node.node_name}</Td>
                    <Td style={{ fontWeight: 600 }}>
                      {node.platform_watts ? formatWatts(node.platform_watts) : "N/A"}
                    </Td>
                    <Td>{formatWatts(node.cpu_watts)}</Td>
                    <Td>{formatWatts(node.memory_watts)}</Td>
                    <Td>{formatWatts(node.gpu_watts)}</Td>
                    <Td>{formatWatts(node.idle_watts)}</Td>
                    <Td>
                      <Label
                        color={node.error_ratio <= 0.05 ? "green" : node.error_ratio <= 0.15 ? "gold" : "red"}
                        style={{ fontSize: "12px" }}
                      >
                        {(node.error_ratio * 100).toFixed(1)}%
                      </Label>
                    </Td>
                    <Td>{node.pod_count}</Td>
                    <Td>
                      <Label
                        color={node.cpu_reading_type === "measured" ? "green" : node.cpu_reading_type === "estimated" ? "gold" : "red"}
                        style={{ fontSize: "11px" }}
                      >
                        {node.cpu_reading_type || "unknown"}
                      </Label>
                      {node.cpu_source && (
                        <span style={{ marginLeft: 6, fontSize: "0.8em", color: "var(--pf-v6-global--Color--200)" }}>
                          {node.cpu_source.replace(/\s*\(https?:\/\/[^)]+\)/, "")}
                        </span>
                      )}
                    </Td>
                  </Tr>
                ))}
              </Tbody>
            </Table>
          );
        })() : (
          <EmptyState>
            <EmptyStateBody>No node power data available.</EmptyStateBody>
          </EmptyState>
        )}
      </PageSection>
    </Page>
  );
};

export default NodesView;
