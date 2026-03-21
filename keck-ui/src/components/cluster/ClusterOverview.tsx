// SPDX-License-Identifier: Apache-2.0

// Cluster overview: namespaces, nodes, and power breakdown for one cluster.
// This is the second zoom level (fleet → cluster).

import { Link } from "react-router-dom";
import {
  PieChart,
  Pie,
  Cell,
  ResponsiveContainer,
  Tooltip,
} from "recharts";
import { useClusterPower, useNamespaces, useNodes } from "@/hooks/useKeckData";
import { formatWatts, formatErrorRatio } from "@/utils/format";
import { StatCard } from "@/components/common/StatCard";
import { ErrorBadge } from "@/components/common/ErrorBadge";

const COMPONENT_COLORS = {
  cpu: "#3b82f6",
  memory: "#8b5cf6",
  gpu: "#22c55e",
  idle: "#6b7280",
};

export function ClusterOverview() {
  const { data: cluster, isLoading: clusterLoading } = useClusterPower();
  const { data: namespaces } = useNamespaces();
  const { data: nodes } = useNodes();

  if (clusterLoading) return <div>Loading cluster data...</div>;
  if (!cluster) return <div>No cluster data available</div>;

  const pieData = [
    { name: "CPU", value: cluster.cpu_watts, color: COMPONENT_COLORS.cpu },
    { name: "Memory", value: cluster.memory_watts, color: COMPONENT_COLORS.memory },
    { name: "GPU", value: cluster.gpu_watts, color: COMPONENT_COLORS.gpu },
    { name: "Idle", value: cluster.idle_watts, color: COMPONENT_COLORS.idle },
  ].filter((d) => d.value > 0);

  return (
    <div>
      <div className="breadcrumb">
        <Link to="/">Fleet</Link>
        <span>/</span>
        Cluster
      </div>

      <h2 className="section-title" style={{ marginBottom: 24 }}>
        Cluster Overview
      </h2>

      {/* Stats */}
      <div className="stats-grid">
        <StatCard
          title="Total Power"
          value={formatWatts(cluster.total_attributed_watts)}
          subtitle={`Platform: ${cluster.platform_watts > 0 ? formatWatts(cluster.platform_watts) : "N/A"}`}
        />
        <StatCard title="CPU" value={formatWatts(cluster.cpu_watts)} color={COMPONENT_COLORS.cpu} />
        <StatCard title="Memory" value={formatWatts(cluster.memory_watts)} color={COMPONENT_COLORS.memory} />
        <StatCard title="GPU" value={formatWatts(cluster.gpu_watts)} color={COMPONENT_COLORS.gpu} />
        <StatCard title="Idle" value={formatWatts(cluster.idle_watts)} color={COMPONENT_COLORS.idle} />
        <StatCard
          title="Accuracy"
          value={formatErrorRatio(cluster.avg_error_ratio)}
          subtitle={`${cluster.node_count} nodes, ${cluster.pod_count} pods`}
        />
      </div>

      {/* Power breakdown pie chart */}
      <div className="chart-container">
        <div className="chart-title">Power by Component</div>
        <ResponsiveContainer width="100%" height={250}>
          <PieChart>
            <Pie
              data={pieData}
              dataKey="value"
              nameKey="name"
              cx="50%"
              cy="50%"
              outerRadius={90}
              label={({ name, value }) => `${name}: ${formatWatts(value)}`}
            >
              {pieData.map((entry, i) => (
                <Cell key={i} fill={entry.color} />
              ))}
            </Pie>
            <Tooltip
              formatter={(value: number) => formatWatts(value)}
              contentStyle={{ background: "#222633", border: "1px solid #2e3344" }}
            />
          </PieChart>
        </ResponsiveContainer>
      </div>

      {/* Namespaces table */}
      {namespaces && (
        <div className="table-grid">
          <div className="section-header">
            <h3 className="section-title">Namespaces</h3>
          </div>

          <table className="data-table">
            <thead>
              <tr>
                <th>Namespace</th>
                <th>Total Power</th>
                <th>CPU</th>
                <th>Memory</th>
                <th>GPU</th>
                <th>Pods</th>
              </tr>
            </thead>
            <tbody>
              {namespaces.map((ns) => (
                <tr key={ns.namespace}>
                  <td>
                    <Link to={`/namespaces/${ns.namespace}`}>{ns.namespace}</Link>
                  </td>
                  <td>{formatWatts(ns.total_watts)}</td>
                  <td>{formatWatts(ns.cpu_watts)}</td>
                  <td>{formatWatts(ns.memory_watts)}</td>
                  <td>{formatWatts(ns.gpu_watts)}</td>
                  <td>{ns.pod_count}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* Nodes table */}
      {nodes && (
        <div className="table-grid">
          <div className="section-header">
            <h3 className="section-title">Nodes</h3>
          </div>

          <table className="data-table">
            <thead>
              <tr>
                <th>Node</th>
                <th>CPU</th>
                <th>Memory</th>
                <th>GPU</th>
                <th>Platform</th>
                <th>Headroom</th>
                <th>Pods</th>
                <th>Accuracy</th>
              </tr>
            </thead>
            <tbody>
              {nodes.map((node) => (
                <tr key={node.node_name}>
                  <td>{node.node_name}</td>
                  <td>{formatWatts(node.cpu_watts)}</td>
                  <td>{formatWatts(node.memory_watts)}</td>
                  <td>{formatWatts(node.gpu_watts)}</td>
                  <td>{node.platform_watts ? formatWatts(node.platform_watts) : "N/A"}</td>
                  <td>{node.headroom_watts ? formatWatts(node.headroom_watts) : "N/A"}</td>
                  <td>{node.pod_count}</td>
                  <td><ErrorBadge ratio={node.error_ratio} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
