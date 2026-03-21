// SPDX-License-Identifier: Apache-2.0

// Types matching the Keck REST API responses.
// These mirror the Rust serde::Serialize types in keck-controller and keck-fleet.

export interface PowerBreakdown {
  cpu_watts: number;
  memory_watts: number;
  gpu_watts: number;
  nic_watts: number;
  storage_watts: number;
  total_watts: number;
}

// ─── Cluster Controller API types ────────────────────────────────

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

export interface ProcessDetail {
  pid: number;
  comm: string;
  cpu_watts: number;
  memory_watts: number;
  gpu_watts: number;
  core_count: number;
}

export interface CoreAttribution {
  core: number;
  time_ns: number;
  avg_freq_khz: number;
  energy_uj: number;
  instructions: number;
  cycles: number;
  cache_misses: number;
}

export interface Reconciliation {
  platform_watts: number | null;
  component_sum_watts: number;
  attributed_sum_watts: number;
  unaccounted_watts: number;
  error_ratio: number;
}

// ─── Fleet Manager API types ─────────────────────────────────────

export interface FleetSummary {
  total_watts: number;
  total_cpu_watts: number;
  total_memory_watts: number;
  total_gpu_watts: number;
  total_idle_watts: number;
  total_carbon_grams_per_hour: number;
  total_cost_per_hour: number;
  total_cost_currency: string;
  cluster_count: number;
  total_nodes: number;
  total_pods: number;
  clusters: ClusterView[];
}

export interface ClusterView {
  cluster_id: string;
  cluster_name: string;
  region: string;
  total_watts: number;
  carbon_grams_per_hour: number;
  carbon_intensity: number;
  cost_per_hour: number;
  node_count: number;
  pod_count: number;
  error_ratio: number;
  last_seen_secs_ago: number;
}

export interface TeamPowerView {
  team: string;
  total_watts: number;
  carbon_grams_per_hour: number;
  cost_per_hour: number;
  per_cluster: TeamClusterBreakdown[];
}

export interface TeamClusterBreakdown {
  cluster_name: string;
  namespace: string;
  watts: number;
}

export interface CarbonRouting {
  recommendation: string;
  reason: string;
  clusters_ranked: {
    name: string;
    intensity: number;
    headroom_watts: number;
  }[];
}
