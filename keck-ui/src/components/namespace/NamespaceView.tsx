// SPDX-License-Identifier: Apache-2.0

// Namespace view: pods in a namespace with power breakdown.
// Third zoom level (fleet → cluster → namespace).

import { Link, useParams } from "react-router-dom";
import { useNamespacePods } from "@/hooks/useKeckData";
import { formatWatts } from "@/utils/format";

export function NamespaceView() {
  const { namespace } = useParams<{ namespace: string }>();
  const { data: pods, isLoading } = useNamespacePods(namespace || "");

  if (!namespace) return <div>No namespace specified</div>;
  if (isLoading) return <div>Loading pods...</div>;

  const totalWatts = pods?.reduce((sum, p) => sum + p.total_watts, 0) ?? 0;

  return (
    <div>
      <div className="breadcrumb">
        <Link to="/">Fleet</Link>
        <span>/</span>
        <Link to="/cluster">Cluster</Link>
        <span>/</span>
        {namespace}
      </div>

      <h2 className="section-title" style={{ marginBottom: 8 }}>
        {namespace}
      </h2>
      <p style={{ color: "var(--text-secondary)", marginBottom: 24 }}>
        {pods?.length ?? 0} pods, {formatWatts(totalWatts)} total
      </p>

      {pods && pods.length > 0 ? (
        <table className="data-table">
          <thead>
            <tr>
              <th>Pod</th>
              <th>Node</th>
              <th>Total</th>
              <th>CPU</th>
              <th>Memory</th>
              <th>GPU</th>
            </tr>
          </thead>
          <tbody>
            {pods.map((pod) => (
              <tr key={pod.pod_uid}>
                <td>
                  <Link to={`/pods/${pod.pod_uid}`}>{pod.pod_name}</Link>
                </td>
                <td>{pod.node_name}</td>
                <td>{formatWatts(pod.total_watts)}</td>
                <td>{formatWatts(pod.cpu_watts)}</td>
                <td>{formatWatts(pod.memory_watts)}</td>
                <td>{formatWatts(pod.gpu_watts)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <div className="card">No pods found in this namespace</div>
      )}
    </div>
  );
}
