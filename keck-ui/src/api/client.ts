// SPDX-License-Identifier: Apache-2.0

// API client for Keck controller and fleet manager.
// Uses fetch() — no axios dependency needed.

import type {
  ClusterPower,
  FleetSummary,
  NamespacePower,
  NodeSummary,
  PodPower,
  ProcessDetail,
  TeamPowerView,
  CarbonRouting,
  Reconciliation,
} from "@/types/power";

const CONTROLLER_BASE = "/api/v1";
const FLEET_BASE = "/api/v1/fleet";

async function get<T>(url: string): Promise<T> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`API error: ${response.status} ${response.statusText}`);
  }
  return response.json();
}

// ─── Cluster Controller API ──────────────────────────────────────

export const clusterApi = {
  getClusterPower: () => get<ClusterPower>(`${CONTROLLER_BASE}/cluster`),

  getNamespaces: () => get<NamespacePower[]>(`${CONTROLLER_BASE}/namespaces`),

  getNamespacePods: (namespace: string) =>
    get<PodPower[]>(`${CONTROLLER_BASE}/namespaces/${namespace}`),

  getNodes: () => get<NodeSummary[]>(`${CONTROLLER_BASE}/nodes`),

  getNode: (name: string) => get<NodeSummary>(`${CONTROLLER_BASE}/nodes/${name}`),

  getPod: (uid: string) => get<PodPower>(`${CONTROLLER_BASE}/pods/${uid}`),

  getPodProcesses: (uid: string) =>
    get<ProcessDetail[]>(`${CONTROLLER_BASE}/pods/${uid}/processes`),

  getReconciliation: () =>
    get<Reconciliation[]>(`${CONTROLLER_BASE}/reconciliation`),
};

// ─── Fleet Manager API ───────────────────────────────────────────

export const fleetApi = {
  getFleetSummary: () => get<FleetSummary>(FLEET_BASE),

  getTeams: () => get<TeamPowerView[]>(`${FLEET_BASE}/teams`),

  getTeam: (name: string) => get<TeamPowerView>(`${FLEET_BASE}/teams/${name}`),

  getCarbonRouting: () => get<CarbonRouting>(`${FLEET_BASE}/carbon`),
};
