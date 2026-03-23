// SPDX-License-Identifier: Apache-2.0

//! Cluster aggregator: maintains the cluster-wide view of power consumption.
//!
//! Receives pod-level power summaries from node agents and aggregates
//! them into higher-level views: deployment, namespace, cluster.
//!
//! Data retention:
//! - Current state: latest power for every known pod/namespace/node
//! - History: rolling window for trend analysis (configurable, default 1hr)
//!
//! Thread safety: behind RwLock in main.rs. Writes (from agent reports)
//! take write lock; reads (from API/scheduler) take read lock.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

/// Power summary received from a node agent (one per pod per report).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PodPowerReport {
    pub node_name: String,
    pub pod_uid: String,
    pub pod_name: String,
    pub namespace: String,
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    pub total_uw: u64,
    pub timestamp: SystemTime,
}

/// Node-level summary received from each agent.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NodePowerReport {
    pub node_name: String,
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    pub platform_uw: Option<u64>,
    pub idle_uw: u64,
    pub error_ratio: f64,
    pub pod_count: u32,
    pub process_count: u32,
    pub timestamp: SystemTime,
    #[serde(default)]
    pub cpu_source: String,
    #[serde(default)]
    pub memory_source: String,
    #[serde(default)]
    pub cpu_reading_type: String,
    #[serde(default)]
    pub sources: Vec<SourceStatus>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SourceStatus {
    pub name: String,
    pub component: String,
    pub reading_type: String,
    pub available: bool,
    pub selected: bool,
    pub power_uw: u64,
}

/// Aggregated report from a single agent push.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentReport {
    pub node: NodePowerReport,
    pub pods: Vec<PodPowerReport>,
}

/// Current power state for a pod.
#[derive(Clone, Debug)]
struct PodState {
    report: PodPowerReport,
    received_at: Instant,
}

/// Current power state for a node.
#[derive(Clone, Debug)]
struct NodeState {
    report: NodePowerReport,
    received_at: Instant,
}

/// Namespace-level aggregation.
#[derive(Clone, Debug, serde::Serialize)]
pub struct NamespacePower {
    pub namespace: String,
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    pub total_uw: u64,
    pub pod_count: usize,
}

/// Cluster-level aggregation.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ClusterPower {
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    pub platform_uw: u64,
    pub idle_uw: u64,
    pub total_attributed_uw: u64,
    pub node_count: usize,
    pub pod_count: usize,
    pub avg_error_ratio: f64,
}

/// Node summary for API responses.
#[derive(Clone, Debug, serde::Serialize)]
pub struct NodeSummary {
    pub node_name: String,
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    pub platform_uw: Option<u64>,
    pub idle_uw: u64,
    pub error_ratio: f64,
    pub pod_count: u32,
    /// Available power headroom (for scheduler)
    /// platform_uw - (cpu + memory + gpu)
    pub headroom_uw: Option<u64>,
    #[serde(skip)]
    pub last_seen: Instant,
    pub last_seen_secs_ago: u64,
}

/// The cluster aggregator.
pub struct ClusterAggregator {
    /// Current pod state, keyed by pod_uid
    pods: HashMap<String, PodState>,

    /// Current node state, keyed by node_name
    nodes: HashMap<String, NodeState>,

    /// How long to keep stale data before eviction
    staleness_threshold: Duration,

    /// History ring buffer (for trends)
    /// Key: namespace, Value: timestamped power values
    history: HashMap<String, Vec<(SystemTime, u64)>>,
    history_retention: Duration,
}

impl ClusterAggregator {
    pub fn new() -> Self {
        Self {
            pods: HashMap::new(),
            nodes: HashMap::new(),
            staleness_threshold: Duration::from_secs(60),
            history: HashMap::new(),
            history_retention: Duration::from_secs(3600), // 1 hour
        }
    }

    /// Ingest a report from a node agent.
    ///
    /// Updates current state for the node and its pods.
    /// Evicts pods that were previously on this node but are no longer reported
    /// (they've been terminated or moved).
    pub fn ingest(&mut self, report: AgentReport) {
        let now = Instant::now();
        let node_name = report.node.node_name.clone();

        // Update node state
        self.nodes.insert(
            node_name.clone(),
            NodeState {
                report: report.node,
                received_at: now,
            },
        );

        // Track which pods this node currently reports
        let reported_uids: Vec<String> = report.pods.iter().map(|p| p.pod_uid.clone()).collect();

        // Remove pods previously on this node that are no longer reported
        self.pods.retain(|uid, state| {
            if state.report.node_name == node_name && !reported_uids.contains(uid) {
                false // Pod was on this node but no longer reported → terminated
            } else {
                true
            }
        });

        // Update/insert reported pods
        for pod in report.pods {
            let namespace = pod.namespace.clone();
            let total = pod.total_uw;
            let ts = pod.timestamp;

            self.pods.insert(
                pod.pod_uid.clone(),
                PodState {
                    report: pod,
                    received_at: now,
                },
            );

            // Record in history
            self.history.entry(namespace).or_default().push((ts, total));
        }

        // Periodic cleanup
        self.evict_stale();
        self.trim_history();
    }

    /// Get cluster-wide power summary.
    pub fn cluster_power(&self) -> ClusterPower {
        let mut cpu = 0u64;
        let mut memory = 0u64;
        let mut gpu = 0u64;
        let mut platform = 0u64;
        let mut idle = 0u64;
        let mut error_sum = 0.0f64;
        let mut node_count = 0usize;

        for state in self.nodes.values() {
            cpu += state.report.cpu_uw;
            memory += state.report.memory_uw;
            gpu += state.report.gpu_uw;
            platform += state.report.platform_uw.unwrap_or(0);
            idle += state.report.idle_uw;
            error_sum += state.report.error_ratio;
            node_count += 1;
        }

        let total_attributed = cpu + memory + gpu;
        let avg_error = if node_count > 0 {
            error_sum / node_count as f64
        } else {
            0.0
        };

        ClusterPower {
            cpu_uw: cpu,
            memory_uw: memory,
            gpu_uw: gpu,
            platform_uw: platform,
            idle_uw: idle,
            total_attributed_uw: total_attributed,
            node_count,
            pod_count: self.pods.len(),
            avg_error_ratio: avg_error,
        }
    }

    /// Get power breakdown by namespace.
    pub fn namespace_power(&self) -> Vec<NamespacePower> {
        let mut ns_map: HashMap<String, NamespacePower> = HashMap::new();

        for state in self.pods.values() {
            let entry = ns_map
                .entry(state.report.namespace.clone())
                .or_insert_with(|| NamespacePower {
                    namespace: state.report.namespace.clone(),
                    cpu_uw: 0,
                    memory_uw: 0,
                    gpu_uw: 0,
                    total_uw: 0,
                    pod_count: 0,
                });

            entry.cpu_uw += state.report.cpu_uw;
            entry.memory_uw += state.report.memory_uw;
            entry.gpu_uw += state.report.gpu_uw;
            entry.total_uw += state.report.total_uw;
            entry.pod_count += 1;
        }

        let mut result: Vec<NamespacePower> = ns_map.into_values().collect();
        result.sort_by(|a, b| b.total_uw.cmp(&a.total_uw)); // Highest power first
        result
    }

    /// Get pods for a specific namespace.
    pub fn pods_in_namespace(&self, namespace: &str) -> Vec<&PodPowerReport> {
        self.pods
            .values()
            .filter(|s| s.report.namespace == namespace)
            .map(|s| &s.report)
            .collect()
    }

    /// Get all node summaries (for scheduler and API).
    pub fn node_summaries(&self) -> Vec<NodeSummary> {
        self.nodes
            .values()
            .map(|state| {
                let r = &state.report;
                let used = r.cpu_uw + r.memory_uw + r.gpu_uw;
                let headroom = r.platform_uw.map(|p| p.saturating_sub(used));

                NodeSummary {
                    node_name: r.node_name.clone(),
                    cpu_uw: r.cpu_uw,
                    memory_uw: r.memory_uw,
                    gpu_uw: r.gpu_uw,
                    platform_uw: r.platform_uw,
                    idle_uw: r.idle_uw,
                    error_ratio: r.error_ratio,
                    pod_count: r.pod_count,
                    headroom_uw: headroom,
                    last_seen: state.received_at,
                    last_seen_secs_ago: state.received_at.elapsed().as_secs(),
                }
            })
            .collect()
    }

    /// Get a specific node's summary.
    pub fn node_summary(&self, node_name: &str) -> Option<NodeSummary> {
        self.node_summaries()
            .into_iter()
            .find(|n| n.node_name == node_name)
    }

    /// Get power for a specific pod.
    pub fn pod_power(&self, pod_uid: &str) -> Option<&PodPowerReport> {
        self.pods.get(pod_uid).map(|s| &s.report)
    }

    /// Get namespace power history for trend analysis.
    pub fn namespace_history(
        &self,
        namespace: &str,
        since: SystemTime,
    ) -> Vec<(SystemTime, u64)> {
        self.history
            .get(namespace)
            .map(|h| h.iter().filter(|(ts, _)| *ts >= since).cloned().collect())
            .unwrap_or_default()
    }

    /// Number of active nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get CPU source info from the first reporting node.
    pub fn cpu_source_info(&self) -> (String, String) {
        self.nodes.values().next()
            .map(|s| (s.report.cpu_source.clone(), s.report.cpu_reading_type.clone()))
            .unwrap_or(("unknown".into(), "none".into()))
    }

    /// Get memory source info from the first reporting node.
    pub fn memory_source_info(&self) -> String {
        self.nodes.values().next()
            .map(|s| s.report.memory_source.clone())
            .unwrap_or("unknown".into())
    }

    /// Get all source statuses (merged from all nodes).
    pub fn all_sources(&self) -> Vec<&SourceStatus> {
        self.nodes.values()
            .flat_map(|s| s.report.sources.iter())
            .collect()
    }

    /// Evict stale pods and nodes that haven't reported recently.
    fn evict_stale(&mut self) {
        let threshold = Instant::now() - self.staleness_threshold;

        self.pods.retain(|_, state| state.received_at > threshold);
        self.nodes.retain(|_, state| state.received_at > threshold);
    }

    /// Trim history beyond retention window.
    fn trim_history(&mut self) {
        let cutoff = SystemTime::now() - self.history_retention;

        for entries in self.history.values_mut() {
            entries.retain(|(ts, _)| *ts >= cutoff);
        }

        // Remove empty namespaces
        self.history.retain(|_, entries| !entries.is_empty());
    }
}
