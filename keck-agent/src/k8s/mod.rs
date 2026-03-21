// SPDX-License-Identifier: Apache-2.0

//! K8s enrichment: maps cgroup IDs → containers → pods → namespaces.
//!
//! Uses the kubelet API (not the API server) to resolve container metadata,
//! similar to Kepler's kubelet informer but using cgroup IDs from eBPF
//! instead of parsing /proc/[pid]/cgroup in userspace.
//!
//! The enrichment layer takes an AttributionSnapshot with raw PIDs/cgroup_ids
//! and fills in container names, pod UIDs, namespaces, and aggregates
//! process power into container/pod/namespace power.

use std::collections::HashMap;

use crate::attribution::{
    AttributionSnapshot, ContainerPower, NamespacePower, PodPower, PowerBreakdown, ProcessPower,
};

/// Container metadata resolved from kubelet.
#[derive(Clone, Debug)]
pub struct ContainerInfo {
    pub container_id: String,
    pub container_name: String,
    pub pod_uid: String,
    pub pod_name: String,
    pub namespace: String,
}

/// Maps cgroup IDs to container metadata.
///
/// Populated by polling the kubelet pod list API.
/// Updated periodically (every 15s by default).
pub struct CgroupResolver {
    /// cgroup_id → container info
    cgroup_map: HashMap<u64, ContainerInfo>,
}

impl CgroupResolver {
    pub fn new() -> Self {
        Self {
            cgroup_map: HashMap::new(),
        }
    }

    /// Update the cgroup → container mapping from kubelet.
    ///
    /// Calls kubelet's /pods endpoint and builds a map from
    /// cgroup v2 IDs to container metadata.
    pub fn refresh(&mut self) -> Result<(), String> {
        // TODO: Implement kubelet API client
        // 1. GET https://<kubelet>:10250/pods
        // 2. For each pod → container, resolve cgroup path
        // 3. Stat the cgroup path to get inode number (= cgroup_id)
        // 4. Build cgroup_id → ContainerInfo mapping
        Ok(())
    }

    /// Look up container info for a cgroup ID.
    pub fn resolve(&self, cgroup_id: u64) -> Option<&ContainerInfo> {
        self.cgroup_map.get(&cgroup_id)
    }
}

/// Enrich an attribution snapshot with K8s metadata.
///
/// Takes the raw process-level attribution and:
/// 1. Resolves PID → container → pod using cgroup IDs
/// 2. Fills in process comm names from /proc
/// 3. Aggregates process power → container power → pod power → namespace
pub fn enrich(
    snapshot: &mut AttributionSnapshot,
    resolver: &CgroupResolver,
) {
    // Group processes by container
    let mut container_processes: HashMap<String, Vec<ProcessPower>> = HashMap::new();
    let mut uncontained: Vec<ProcessPower> = Vec::new();

    for mut process in snapshot.processes.drain(..) {
        if let Some(info) = resolver.resolve(process.cgroup_id) {
            process.comm = read_comm(process.pid).unwrap_or_default();
            container_processes
                .entry(info.container_id.clone())
                .or_default()
                .push(process);
        } else {
            process.comm = read_comm(process.pid).unwrap_or_default();
            uncontained.push(process);
        }
    }

    // Build container power (aggregate from processes)
    let mut pod_containers: HashMap<String, Vec<ContainerPower>> = HashMap::new();

    for (container_id, processes) in container_processes {
        let info = match resolver.resolve(processes[0].cgroup_id) {
            Some(info) => info,
            None => continue,
        };

        let power = aggregate_power(processes.iter().map(|p| &p.power));

        let container = ContainerPower {
            container_id: container_id.clone(),
            name: info.container_name.clone(),
            cgroup_id: processes[0].cgroup_id,
            power,
            processes,
        };

        pod_containers
            .entry(info.pod_uid.clone())
            .or_default()
            .push(container);
    }

    // Build pod power (aggregate from containers)
    let mut namespace_pods: HashMap<String, Vec<PodPower>> = HashMap::new();

    for (pod_uid, containers) in pod_containers {
        // Get pod metadata from first container's info
        let first_cgroup = containers[0].cgroup_id;
        let info = match resolver.resolve(first_cgroup) {
            Some(info) => info,
            None => continue,
        };

        let power = aggregate_power(containers.iter().map(|c| &c.power));

        let pod = PodPower {
            pod_uid,
            name: info.pod_name.clone(),
            namespace: info.namespace.clone(),
            power,
            containers,
        };

        namespace_pods
            .entry(info.namespace.clone())
            .or_default()
            .push(pod);
    }

    // Build namespace power (aggregate from pods)
    let mut namespaces = Vec::new();
    let mut all_pods = Vec::new();

    for (namespace, pods) in namespace_pods {
        let power = aggregate_power(pods.iter().map(|p| &p.power));
        let pod_count = pods.len();

        namespaces.push(NamespacePower {
            namespace,
            power,
            pod_count,
        });

        all_pods.extend(pods);
    }

    // Put uncontained processes back
    snapshot.processes = uncontained;
    snapshot.pods = all_pods;
    snapshot.namespaces = namespaces;
}

/// Aggregate power breakdowns from multiple items.
fn aggregate_power<'a>(items: impl Iterator<Item = &'a PowerBreakdown>) -> PowerBreakdown {
    let mut total = PowerBreakdown::default();
    for p in items {
        total.cpu_uw += p.cpu_uw;
        total.memory_uw += p.memory_uw;
        total.gpu_uw += p.gpu_uw;
        total.nic_uw += p.nic_uw;
        total.storage_uw += p.storage_uw;
    }
    total
}

/// Read process comm name from /proc/[pid]/comm.
fn read_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{}/comm", pid))
        .ok()
        .map(|s| s.trim().to_string())
}
