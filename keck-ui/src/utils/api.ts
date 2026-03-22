// SPDX-License-Identifier: Apache-2.0

// API client for Keck controller.
// Uses the console's proxy to route to keck-controller service.

const BASE = "/api/proxy/plugin/keck-power-management/keck-api";

async function get<T>(path: string): Promise<T> {
  const response = await fetch(`${BASE}${path}`);
  if (!response.ok) {
    throw new Error(`API error: ${response.status} ${response.statusText}`);
  }
  return response.json();
}

export interface ClusterPower {
  cpu_watts: number;
  memory_watts: number;
  gpu_watts: number;
  platform_watts: number;
  idle_watts: number;
  total_attributed_watts: number;
  node_count: number;
  pod_count: number;
  avg_error_ratio: number;
}

export interface NamespacePower {
  namespace: string;
  cpu_watts: number;
  memory_watts: number;
  gpu_watts: number;
  total_watts: number;
  pod_count: number;
}

export interface NodeSummary {
  node_name: string;
  cpu_watts: number;
  memory_watts: number;
  gpu_watts: number;
  platform_watts: number | null;
  idle_watts: number;
  error_ratio: number;
  pod_count: number;
  headroom_watts: number | null;
}

export interface PodPower {
  pod_uid: string;
  pod_name: string;
  namespace: string;
  node_name: string;
  cpu_watts: number;
  memory_watts: number;
  gpu_watts: number;
  total_watts: number;
}

export const api = {
  getClusterPower: () => get<ClusterPower>("/api/v1/cluster"),
  getNamespaces: () => get<NamespacePower[]>("/api/v1/namespaces"),
  getNamespacePods: (ns: string) => get<PodPower[]>(`/api/v1/pods-by-namespace?ns=${encodeURIComponent(ns)}`),
  getNodes: () => get<NodeSummary[]>("/api/v1/nodes"),
};
