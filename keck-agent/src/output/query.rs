// SPDX-License-Identifier: Apache-2.0

//! Query API: serves drill-down requests from the cluster controller or CLI.
//!
//! When a user clicks "show me process detail for pod X" in a dashboard,
//! the cluster controller forwards the request to this node's query API.
//! We serve directly from the local store — no recomputation needed.
//!
//! Protocol: gRPC (same connection as upstream reporting, bidirectional).
//! Fallback: HTTP/JSON for CLI and debugging.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::store::LocalStore;

/// Query request types.
pub enum QueryRequest {
    /// Get current node-level power summary
    NodeSummary,

    /// Get process-level detail for a specific pod
    PodDetail {
        pod_uid: String,
        /// How far back to look (default: last 5 minutes)
        lookback: Duration,
    },

    /// Get pod-level data for a namespace
    NamespacePods {
        namespace: String,
        lookback: Duration,
    },

    /// Get reconciliation history (attribution quality over time)
    ReconciliationHistory {
        count: usize,
    },

    /// Get agent self-monitoring data
    AgentHealth,
}

/// Query response — serialized to JSON or protobuf depending on transport.
pub enum QueryResponse {
    NodeSummary(NodeSummaryResponse),
    PodDetail(PodDetailResponse),
    NamespacePods(NamespacePodsResponse),
    ReconciliationHistory(ReconciliationHistoryResponse),
    AgentHealth(AgentHealthResponse),
    Error(String),
}

pub struct NodeSummaryResponse {
    pub cpu_watts: f64,
    pub memory_watts: f64,
    pub gpu_watts: f64,
    pub platform_watts: Option<f64>,
    pub idle_watts: f64,
    pub error_ratio: f64,
    pub pod_count: usize,
    pub process_count: usize,
}

pub struct PodDetailResponse {
    pub pod_uid: String,
    pub processes: Vec<ProcessDetail>,
}

pub struct ProcessDetail {
    pub pid: u32,
    pub comm: String,
    pub cpu_watts: f64,
    pub memory_watts: f64,
    pub gpu_watts: f64,
    pub core_count: usize,
}

pub struct NamespacePodsResponse {
    pub namespace: String,
    pub pods: Vec<PodSummaryResponse>,
}

pub struct PodSummaryResponse {
    pub name: String,
    pub namespace: String,
    pub total_watts: f64,
}

pub struct ReconciliationHistoryResponse {
    pub entries: Vec<ReconciliationEntry>,
}

pub struct ReconciliationEntry {
    pub error_ratio: f64,
    pub unaccounted_watts: f64,
}

pub struct AgentHealthResponse {
    pub memory_bytes: usize,
    pub cpu_usage_percent: f64,
    pub ebpf_map_entries: usize,
    pub uptime_secs: u64,
}

/// Query server that handles incoming requests.
pub struct QueryServer {
    // Store is shared with the main loop via Arc
    // (main loop writes, query server reads)
}

impl QueryServer {
    pub fn new() -> Self {
        Self {}
    }

    /// Handle a query request against the local store.
    pub fn handle(
        &self,
        store: &LocalStore,
        request: QueryRequest,
    ) -> QueryResponse {
        match request {
            QueryRequest::NodeSummary => {
                let snapshot = match store.latest() {
                    Some(s) => s,
                    None => return QueryResponse::Error("No data yet".into()),
                };

                QueryResponse::NodeSummary(NodeSummaryResponse {
                    cpu_watts: snapshot.node.measured.cpu_uw as f64 / 1e6,
                    memory_watts: snapshot.node.measured.memory_uw as f64 / 1e6,
                    gpu_watts: snapshot.node.measured.gpu_uw as f64 / 1e6,
                    platform_watts: snapshot.node.platform_uw.map(|v| v as f64 / 1e6),
                    idle_watts: snapshot.idle_power.total_uw() as f64 / 1e6,
                    error_ratio: snapshot.reconciliation.error_ratio,
                    pod_count: snapshot.pods.len(),
                    process_count: snapshot.processes.len()
                        + snapshot
                            .pods
                            .iter()
                            .flat_map(|p| &p.containers)
                            .flat_map(|c| &c.processes)
                            .count(),
                })
            }

            QueryRequest::PodDetail { pod_uid, lookback } => {
                let since = Instant::now() - lookback;
                let processes = store.query_pod_processes(&pod_uid, since);

                let details: Vec<ProcessDetail> = processes
                    .into_iter()
                    .map(|p| ProcessDetail {
                        pid: p.pid,
                        comm: p.comm.clone(),
                        cpu_watts: p.power.cpu_uw as f64 / 1e6,
                        memory_watts: p.power.memory_uw as f64 / 1e6,
                        gpu_watts: p.power.gpu_uw as f64 / 1e6,
                        core_count: p.core_detail.len(),
                    })
                    .collect();

                QueryResponse::PodDetail(PodDetailResponse {
                    pod_uid,
                    processes: details,
                })
            }

            QueryRequest::NamespacePods {
                namespace,
                lookback,
            } => {
                let since = Instant::now() - lookback;
                let pods = store.query_namespace_pods(&namespace, since);

                let pod_responses: Vec<PodSummaryResponse> = pods
                    .into_iter()
                    .map(|p| PodSummaryResponse {
                        name: p.name.clone(),
                        namespace: p.namespace.clone(),
                        total_watts: p.total_uw as f64 / 1e6,
                    })
                    .collect();

                QueryResponse::NamespacePods(NamespacePodsResponse {
                    namespace,
                    pods: pod_responses,
                })
            }

            QueryRequest::ReconciliationHistory { count } => {
                let history = store.reconciliation_history(count);

                let entries: Vec<ReconciliationEntry> = history
                    .into_iter()
                    .map(|r| ReconciliationEntry {
                        error_ratio: r.error_ratio,
                        unaccounted_watts: r.unaccounted_uw as f64 / 1e6,
                    })
                    .collect();

                QueryResponse::ReconciliationHistory(ReconciliationHistoryResponse {
                    entries,
                })
            }

            QueryRequest::AgentHealth => {
                QueryResponse::AgentHealth(AgentHealthResponse {
                    memory_bytes: store.estimated_memory(),
                    cpu_usage_percent: 0.0, // TODO: self-monitoring
                    ebpf_map_entries: 0,    // TODO: read from BPF maps
                    uptime_secs: 0,         // TODO: track startup time
                })
            }
        }
    }
}
