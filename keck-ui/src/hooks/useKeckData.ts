// SPDX-License-Identifier: Apache-2.0

// React Query hooks for Keck API data.
// Auto-refreshes every 5 seconds for real-time power display.

import { useQuery } from "@tanstack/react-query";
import { clusterApi, fleetApi } from "@/api/client";

const REFRESH_INTERVAL = 5000; // 5 seconds

// ─── Cluster-level hooks ─────────────────────────────────────────

export function useClusterPower() {
  return useQuery({
    queryKey: ["cluster-power"],
    queryFn: clusterApi.getClusterPower,
    refetchInterval: REFRESH_INTERVAL,
  });
}

export function useNamespaces() {
  return useQuery({
    queryKey: ["namespaces"],
    queryFn: clusterApi.getNamespaces,
    refetchInterval: REFRESH_INTERVAL,
  });
}

export function useNamespacePods(namespace: string) {
  return useQuery({
    queryKey: ["namespace-pods", namespace],
    queryFn: () => clusterApi.getNamespacePods(namespace),
    refetchInterval: REFRESH_INTERVAL,
    enabled: !!namespace,
  });
}

export function useNodes() {
  return useQuery({
    queryKey: ["nodes"],
    queryFn: clusterApi.getNodes,
    refetchInterval: REFRESH_INTERVAL,
  });
}

export function useNode(name: string) {
  return useQuery({
    queryKey: ["node", name],
    queryFn: () => clusterApi.getNode(name),
    refetchInterval: REFRESH_INTERVAL,
    enabled: !!name,
  });
}

export function usePodProcesses(uid: string) {
  return useQuery({
    queryKey: ["pod-processes", uid],
    queryFn: () => clusterApi.getPodProcesses(uid),
    refetchInterval: REFRESH_INTERVAL,
    enabled: !!uid,
  });
}

// ─── Fleet-level hooks ───────────────────────────────────────────

export function useFleetSummary() {
  return useQuery({
    queryKey: ["fleet-summary"],
    queryFn: fleetApi.getFleetSummary,
    refetchInterval: REFRESH_INTERVAL,
  });
}

export function useTeams() {
  return useQuery({
    queryKey: ["teams"],
    queryFn: fleetApi.getTeams,
    refetchInterval: REFRESH_INTERVAL,
  });
}

export function useCarbonRouting() {
  return useQuery({
    queryKey: ["carbon-routing"],
    queryFn: fleetApi.getCarbonRouting,
    refetchInterval: 30000, // Carbon changes slowly — 30s refresh
  });
}
