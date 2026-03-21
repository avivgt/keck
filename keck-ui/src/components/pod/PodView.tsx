// SPDX-License-Identifier: Apache-2.0

// Pod view: the deepest zoom level in the UI.
// Shows per-process power with per-core detail.
// Fleet → Cluster → Namespace → Pod → Process → Core

import { Link, useParams } from "react-router-dom";
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import { usePodProcesses } from "@/hooks/useKeckData";
import { formatWatts } from "@/utils/format";

export function PodView() {
  const { uid } = useParams<{ uid: string }>();
  const { data: processes, isLoading } = usePodProcesses(uid || "");

  if (!uid) return <div>No pod specified</div>;
  if (isLoading) return <div>Loading pod detail...</div>;

  const totalWatts = processes?.reduce(
    (sum, p) => sum + p.cpu_watts + p.memory_watts + p.gpu_watts,
    0
  ) ?? 0;

  // Chart data: per-process power breakdown
  const chartData = processes?.map((p) => ({
    name: `${p.comm} (${p.pid})`,
    cpu: p.cpu_watts,
    memory: p.memory_watts,
    gpu: p.gpu_watts,
  })) ?? [];

  return (
    <div>
      <div className="breadcrumb">
        <Link to="/">Fleet</Link>
        <span>/</span>
        <Link to="/cluster">Cluster</Link>
        <span>/</span>
        Pod
      </div>

      <h2 className="section-title" style={{ marginBottom: 8 }}>
        Pod Detail
      </h2>
      <p style={{ color: "var(--text-secondary)", marginBottom: 24 }}>
        {processes?.length ?? 0} processes, {formatWatts(totalWatts)} total
      </p>

      {/* Process power chart */}
      {chartData.length > 0 && (
        <div className="chart-container">
          <div className="chart-title">Power by Process</div>
          <ResponsiveContainer width="100%" height={300}>
            <BarChart data={chartData} layout="vertical">
              <XAxis type="number" tickFormatter={(v) => formatWatts(v)} />
              <YAxis type="category" dataKey="name" width={200} />
              <Tooltip
                formatter={(value: number) => formatWatts(value)}
                contentStyle={{ background: "#222633", border: "1px solid #2e3344" }}
              />
              <Bar dataKey="cpu" stackId="power" fill="#3b82f6" name="CPU" />
              <Bar dataKey="memory" stackId="power" fill="#8b5cf6" name="Memory" />
              <Bar dataKey="gpu" stackId="power" fill="#22c55e" name="GPU" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      {/* Process table */}
      {processes && processes.length > 0 ? (
        <table className="data-table">
          <thead>
            <tr>
              <th>PID</th>
              <th>Command</th>
              <th>CPU</th>
              <th>Memory</th>
              <th>GPU</th>
              <th>Total</th>
              <th>Cores</th>
            </tr>
          </thead>
          <tbody>
            {processes.map((proc) => (
              <tr key={proc.pid}>
                <td>{proc.pid}</td>
                <td><code>{proc.comm}</code></td>
                <td>{formatWatts(proc.cpu_watts)}</td>
                <td>{formatWatts(proc.memory_watts)}</td>
                <td>{formatWatts(proc.gpu_watts)}</td>
                <td>
                  {formatWatts(proc.cpu_watts + proc.memory_watts + proc.gpu_watts)}
                </td>
                <td>{proc.core_count}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <div className="card">
          No process data available. The node agent may need Full profile
          to expose process-level detail.
        </div>
      )}
    </div>
  );
}
