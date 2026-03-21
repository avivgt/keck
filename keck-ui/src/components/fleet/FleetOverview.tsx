// SPDX-License-Identifier: Apache-2.0

// Fleet overview: the top-level zoom. Shows all clusters, total power,
// carbon, cost, and per-team breakdown.

import { Link } from "react-router-dom";
import { useFleetSummary, useTeams } from "@/hooks/useKeckData";
import {
  formatWatts,
  formatCarbon,
  formatCost,
  formatIntensity,
  carbonColor,
} from "@/utils/format";
import { StatCard } from "@/components/common/StatCard";
import { ErrorBadge } from "@/components/common/ErrorBadge";

export function FleetOverview() {
  const { data: fleet, isLoading: fleetLoading } = useFleetSummary();
  const { data: teams, isLoading: teamsLoading } = useTeams();

  if (fleetLoading) return <div>Loading fleet data...</div>;
  if (!fleet) return <div>No fleet data available</div>;

  return (
    <div>
      <h2 className="section-title" style={{ marginBottom: 24 }}>
        Fleet Overview
      </h2>

      {/* Top-level stats */}
      <div className="stats-grid">
        <StatCard
          title="Total Power"
          value={formatWatts(fleet.total_watts)}
          subtitle={`${fleet.cluster_count} clusters, ${fleet.total_nodes} nodes`}
        />
        <StatCard
          title="Carbon Emissions"
          value={formatCarbon(fleet.total_carbon_grams_per_hour)}
          subtitle={`${fleet.total_carbon_grams_per_hour.toFixed(0)} gCO\u2082/hr`}
        />
        <StatCard
          title="Energy Cost"
          value={formatCost(fleet.total_cost_per_hour, fleet.total_cost_currency)}
          subtitle={`${fleet.total_cost_currency} ${fleet.total_cost_per_hour.toFixed(2)}/hr`}
        />
        <StatCard
          title="Workloads"
          value={fleet.total_pods.toString()}
          subtitle={`pods across ${fleet.cluster_count} clusters`}
        />
      </div>

      {/* Per-cluster table */}
      <div className="table-grid">
        <div className="section-header">
          <h3 className="section-title">Clusters</h3>
        </div>

        <table className="data-table">
          <thead>
            <tr>
              <th>Cluster</th>
              <th>Region</th>
              <th>Power</th>
              <th>Carbon Intensity</th>
              <th>Emissions</th>
              <th>Cost</th>
              <th>Nodes</th>
              <th>Pods</th>
              <th>Accuracy</th>
            </tr>
          </thead>
          <tbody>
            {fleet.clusters.map((cluster) => (
              <tr key={cluster.cluster_id}>
                <td>
                  <Link to={`/cluster?id=${cluster.cluster_id}`}>
                    {cluster.cluster_name}
                  </Link>
                </td>
                <td>{cluster.region}</td>
                <td>{formatWatts(cluster.total_watts)}</td>
                <td>
                  <span style={{ color: carbonColor(cluster.carbon_intensity) }}>
                    {formatIntensity(cluster.carbon_intensity)}
                  </span>
                </td>
                <td>{formatCarbon(cluster.carbon_grams_per_hour)}</td>
                <td>{formatCost(cluster.cost_per_hour)}</td>
                <td>{cluster.node_count}</td>
                <td>{cluster.pod_count}</td>
                <td><ErrorBadge ratio={cluster.error_ratio} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* Per-team table */}
      {teams && teams.length > 0 && (
        <div className="table-grid">
          <div className="section-header">
            <h3 className="section-title">Teams</h3>
          </div>

          <table className="data-table">
            <thead>
              <tr>
                <th>Team</th>
                <th>Power</th>
                <th>Emissions</th>
                <th>Cost</th>
                <th>Clusters</th>
              </tr>
            </thead>
            <tbody>
              {teams.map((team) => (
                <tr key={team.team}>
                  <td>{team.team}</td>
                  <td>{formatWatts(team.total_watts)}</td>
                  <td>{formatCarbon(team.carbon_grams_per_hour)}</td>
                  <td>{formatCost(team.cost_per_hour)}</td>
                  <td>{team.per_cluster.length}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
