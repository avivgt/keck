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
    #[serde(default)]
    pub storage_uw: u64,
    #[serde(default)]
    pub io_uw: u64,
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
    #[serde(default)]
    pub psu_output_uw: Option<u64>,
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
    #[serde(default)]
    pub node_name: String,
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
    pub storage_uw: u64,
    pub io_uw: u64,
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
    pub psu_output_uw: u64,
    pub psu_loss_uw: u64,
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
    pub psu_output_uw: Option<u64>,
    pub idle_uw: u64,
    pub error_ratio: f64,
    pub pod_count: u32,
    /// Available power headroom (for scheduler)
    pub headroom_uw: Option<u64>,
    pub cpu_source: String,
    pub cpu_reading_type: String,
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
        let mut psu_output = 0u64;
        let mut idle = 0u64;
        let mut error_sum = 0.0f64;
        let mut node_count = 0usize;

        for state in self.nodes.values() {
            cpu += state.report.cpu_uw;
            memory += state.report.memory_uw;
            gpu += state.report.gpu_uw;
            platform += state.report.platform_uw.unwrap_or(0);
            psu_output += state.report.psu_output_uw.unwrap_or(0);
            idle += state.report.idle_uw;
            error_sum += state.report.error_ratio;
            node_count += 1;
        }

        let total_attributed = cpu + memory + gpu;
        let psu_loss = platform.saturating_sub(psu_output);
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
            psu_output_uw: psu_output,
            psu_loss_uw: psu_loss,
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
                    storage_uw: 0,
                    io_uw: 0,
                    total_uw: 0,
                    pod_count: 0,
                });

            entry.cpu_uw += state.report.cpu_uw;
            entry.memory_uw += state.report.memory_uw;
            entry.gpu_uw += state.report.gpu_uw;
            entry.storage_uw += state.report.storage_uw;
            entry.io_uw += state.report.io_uw;
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
                    psu_output_uw: r.psu_output_uw,
                    idle_uw: r.idle_uw,
                    error_ratio: r.error_ratio,
                    pod_count: r.pod_count,
                    headroom_uw: headroom,
                    cpu_source: r.cpu_source.clone(),
                    cpu_reading_type: r.cpu_reading_type.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Helper factories ────────────────────────────────────────

    fn make_pod(node: &str, uid: &str, name: &str, ns: &str, cpu: u64, mem: u64, gpu: u64) -> PodPowerReport {
        PodPowerReport {
            node_name: node.into(),
            pod_uid: uid.into(),
            pod_name: name.into(),
            namespace: ns.into(),
            cpu_uw: cpu,
            memory_uw: mem,
            gpu_uw: gpu,
            total_uw: cpu + mem + gpu,
            timestamp: SystemTime::now(),
        }
    }

    fn make_node(name: &str, cpu: u64, mem: u64, gpu: u64, platform: Option<u64>, idle: u64) -> NodePowerReport {
        NodePowerReport {
            node_name: name.into(),
            cpu_uw: cpu,
            memory_uw: mem,
            gpu_uw: gpu,
            platform_uw: platform,
            idle_uw: idle,
            error_ratio: 0.05,
            pod_count: 0,
            process_count: 0,
            timestamp: SystemTime::now(),
            cpu_source: "rapl".into(),
            memory_source: "estimated".into(),
            cpu_reading_type: "estimated".into(),
            sources: vec![],
        }
    }

    fn make_report(node: NodePowerReport, pods: Vec<PodPowerReport>) -> AgentReport {
        AgentReport { node, pods }
    }

    // ─── ingest() tests ──────────────────────────────────────────

    #[test]
    fn test_ingest_single_report() {
        let mut agg = ClusterAggregator::new();
        let pod = make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0);
        let node = make_node("node-1", 5000, 2000, 0, Some(10000), 3000);
        let report = make_report(node, vec![pod]);

        agg.ingest(report);

        assert_eq!(agg.nodes.len(), 1);
        assert_eq!(agg.pods.len(), 1);
        assert!(agg.pod_power("uid-a").is_some());
        assert_eq!(agg.pod_power("uid-a").unwrap().cpu_uw, 1000);
    }

    #[test]
    fn test_ingest_multiple_reports_same_node() {
        let mut agg = ClusterAggregator::new();

        // First report with two pods
        let pods1 = vec![
            make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0),
            make_pod("node-1", "uid-b", "web-2", "default", 2000, 800, 0),
        ];
        agg.ingest(make_report(make_node("node-1", 5000, 2000, 0, None, 1000), pods1));
        assert_eq!(agg.pods.len(), 2);

        // Second report from same node, updated values
        let pods2 = vec![
            make_pod("node-1", "uid-a", "web-1", "default", 1500, 600, 0),
            make_pod("node-1", "uid-b", "web-2", "default", 2500, 900, 0),
        ];
        agg.ingest(make_report(make_node("node-1", 6000, 2500, 0, None, 1200), pods2));
        assert_eq!(agg.pods.len(), 2);
        assert_eq!(agg.pod_power("uid-a").unwrap().cpu_uw, 1500);
    }

    #[test]
    fn test_ingest_multiple_nodes() {
        let mut agg = ClusterAggregator::new();

        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, None, 1000),
            vec![make_pod("node-1", "uid-a", "web-1", "ns-1", 1000, 500, 0)],
        ));
        agg.ingest(make_report(
            make_node("node-2", 3000, 1000, 0, None, 800),
            vec![make_pod("node-2", "uid-b", "api-1", "ns-2", 800, 300, 0)],
        ));

        assert_eq!(agg.nodes.len(), 2);
        assert_eq!(agg.pods.len(), 2);
    }

    #[test]
    fn test_ingest_pod_eviction_when_no_longer_reported() {
        let mut agg = ClusterAggregator::new();

        // Report with 3 pods
        let pods1 = vec![
            make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0),
            make_pod("node-1", "uid-b", "web-2", "default", 2000, 800, 0),
            make_pod("node-1", "uid-c", "web-3", "default", 3000, 1000, 0),
        ];
        agg.ingest(make_report(make_node("node-1", 8000, 3000, 0, None, 2000), pods1));
        assert_eq!(agg.pods.len(), 3);

        // Next report only has 2 pods — uid-b was terminated
        let pods2 = vec![
            make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0),
            make_pod("node-1", "uid-c", "web-3", "default", 3000, 1000, 0),
        ];
        agg.ingest(make_report(make_node("node-1", 6000, 2000, 0, None, 1500), pods2));
        assert_eq!(agg.pods.len(), 2);
        assert!(agg.pod_power("uid-b").is_none());
        assert!(agg.pod_power("uid-a").is_some());
        assert!(agg.pod_power("uid-c").is_some());
    }

    #[test]
    fn test_ingest_pod_eviction_does_not_affect_other_nodes() {
        let mut agg = ClusterAggregator::new();

        // Node-1 with pod-a, Node-2 with pod-b
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, None, 1000),
            vec![make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0)],
        ));
        agg.ingest(make_report(
            make_node("node-2", 3000, 1000, 0, None, 800),
            vec![make_pod("node-2", "uid-b", "api-1", "default", 800, 300, 0)],
        ));
        assert_eq!(agg.pods.len(), 2);

        // Node-1 reports with no pods — only uid-a should be evicted
        agg.ingest(make_report(
            make_node("node-1", 2000, 500, 0, None, 1500),
            vec![],
        ));
        assert_eq!(agg.pods.len(), 1);
        assert!(agg.pod_power("uid-a").is_none());
        assert!(agg.pod_power("uid-b").is_some());
    }

    #[test]
    fn test_ingest_records_history() {
        let mut agg = ClusterAggregator::new();
        let pod = make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0);
        agg.ingest(make_report(make_node("node-1", 5000, 2000, 0, None, 1000), vec![pod]));

        let since = SystemTime::now() - Duration::from_secs(10);
        let history = agg.namespace_history("default", since);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].1, 1500); // total_uw = cpu + mem + gpu
    }

    // ─── cluster_power() tests ───────────────────────────────────

    #[test]
    fn test_cluster_power_empty() {
        let agg = ClusterAggregator::new();
        let power = agg.cluster_power();
        assert_eq!(power.cpu_uw, 0);
        assert_eq!(power.memory_uw, 0);
        assert_eq!(power.gpu_uw, 0);
        assert_eq!(power.platform_uw, 0);
        assert_eq!(power.idle_uw, 0);
        assert_eq!(power.node_count, 0);
        assert_eq!(power.pod_count, 0);
        assert_eq!(power.avg_error_ratio, 0.0);
    }

    #[test]
    fn test_cluster_power_single_node() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 1000, Some(15000), 3000),
            vec![make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 200)],
        ));

        let power = agg.cluster_power();
        assert_eq!(power.cpu_uw, 5000);
        assert_eq!(power.memory_uw, 2000);
        assert_eq!(power.gpu_uw, 1000);
        assert_eq!(power.platform_uw, 15000);
        assert_eq!(power.idle_uw, 3000);
        assert_eq!(power.total_attributed_uw, 8000); // cpu + mem + gpu
        assert_eq!(power.node_count, 1);
        assert_eq!(power.pod_count, 1);
    }

    #[test]
    fn test_cluster_power_multi_node() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, Some(10000), 3000),
            vec![make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0)],
        ));
        agg.ingest(make_report(
            make_node("node-2", 3000, 1000, 500, Some(8000), 2000),
            vec![make_pod("node-2", "uid-b", "api-1", "prod", 800, 300, 100)],
        ));

        let power = agg.cluster_power();
        assert_eq!(power.cpu_uw, 8000);
        assert_eq!(power.memory_uw, 3000);
        assert_eq!(power.gpu_uw, 500);
        assert_eq!(power.platform_uw, 18000);
        assert_eq!(power.idle_uw, 5000);
        assert_eq!(power.node_count, 2);
        assert_eq!(power.pod_count, 2);
    }

    #[test]
    fn test_cluster_power_platform_uw_none_treated_as_zero() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, None, 3000),
            vec![],
        ));
        let power = agg.cluster_power();
        assert_eq!(power.platform_uw, 0);
    }

    #[test]
    fn test_cluster_power_avg_error_ratio() {
        let mut agg = ClusterAggregator::new();
        let mut node1 = make_node("node-1", 5000, 2000, 0, None, 3000);
        node1.error_ratio = 0.10;
        let mut node2 = make_node("node-2", 3000, 1000, 0, None, 2000);
        node2.error_ratio = 0.20;

        agg.ingest(make_report(node1, vec![]));
        agg.ingest(make_report(node2, vec![]));

        let power = agg.cluster_power();
        assert!((power.avg_error_ratio - 0.15).abs() < 1e-10);
    }

    // ─── namespace_power() tests ─────────────────────────────────

    #[test]
    fn test_namespace_power_empty() {
        let agg = ClusterAggregator::new();
        assert!(agg.namespace_power().is_empty());
    }

    #[test]
    fn test_namespace_power_single_namespace() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, None, 1000),
            vec![
                make_pod("node-1", "uid-a", "web-1", "prod", 1000, 500, 0),
                make_pod("node-1", "uid-b", "web-2", "prod", 2000, 800, 0),
            ],
        ));

        let ns = agg.namespace_power();
        assert_eq!(ns.len(), 1);
        assert_eq!(ns[0].namespace, "prod");
        assert_eq!(ns[0].cpu_uw, 3000);
        assert_eq!(ns[0].memory_uw, 1300);
        assert_eq!(ns[0].pod_count, 2);
    }

    #[test]
    fn test_namespace_power_multiple_namespaces_sorted() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 10000, 5000, 0, None, 2000),
            vec![
                make_pod("node-1", "uid-a", "web-1", "low-power", 100, 50, 0),
                make_pod("node-1", "uid-b", "api-1", "high-power", 5000, 2000, 0),
                make_pod("node-1", "uid-c", "ml-1", "medium-power", 1000, 500, 0),
            ],
        ));

        let ns = agg.namespace_power();
        assert_eq!(ns.len(), 3);
        // Sorted by total_uw descending
        assert_eq!(ns[0].namespace, "high-power");
        assert_eq!(ns[1].namespace, "medium-power");
        assert_eq!(ns[2].namespace, "low-power");
    }

    #[test]
    fn test_namespace_power_pods_across_nodes() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, None, 1000),
            vec![make_pod("node-1", "uid-a", "web-1", "prod", 1000, 500, 0)],
        ));
        agg.ingest(make_report(
            make_node("node-2", 3000, 1000, 0, None, 800),
            vec![make_pod("node-2", "uid-b", "web-2", "prod", 2000, 800, 0)],
        ));

        let ns = agg.namespace_power();
        assert_eq!(ns.len(), 1);
        assert_eq!(ns[0].cpu_uw, 3000);
        assert_eq!(ns[0].pod_count, 2);
    }

    // ─── pods_in_namespace() tests ───────────────────────────────

    #[test]
    fn test_pods_in_namespace_correct_filtering() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 10000, 5000, 0, None, 2000),
            vec![
                make_pod("node-1", "uid-a", "web-1", "prod", 1000, 500, 0),
                make_pod("node-1", "uid-b", "web-2", "staging", 2000, 800, 0),
                make_pod("node-1", "uid-c", "web-3", "prod", 3000, 1000, 0),
            ],
        ));

        let prod_pods = agg.pods_in_namespace("prod");
        assert_eq!(prod_pods.len(), 2);
        let uids: Vec<&str> = prod_pods.iter().map(|p| p.pod_uid.as_str()).collect();
        assert!(uids.contains(&"uid-a"));
        assert!(uids.contains(&"uid-c"));
    }

    #[test]
    fn test_pods_in_namespace_empty() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, None, 1000),
            vec![make_pod("node-1", "uid-a", "web-1", "prod", 1000, 500, 0)],
        ));

        assert!(agg.pods_in_namespace("nonexistent").is_empty());
    }

    // ─── node_summaries() tests ──────────────────────────────────

    #[test]
    fn test_node_summaries_headroom_with_platform() {
        let mut agg = ClusterAggregator::new();
        // cpu=5000, mem=2000, gpu=1000 => used=8000, platform=15000 => headroom=7000
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 1000, Some(15000), 3000),
            vec![],
        ));

        let summaries = agg.node_summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].headroom_uw, Some(7000));
    }

    #[test]
    fn test_node_summaries_headroom_without_platform() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, None, 3000),
            vec![],
        ));

        let summaries = agg.node_summaries();
        assert_eq!(summaries[0].headroom_uw, None);
    }

    #[test]
    fn test_node_summaries_headroom_saturating_sub() {
        let mut agg = ClusterAggregator::new();
        // used (5000+2000+1000=8000) > platform (5000) => headroom saturates to 0
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 1000, Some(5000), 0),
            vec![],
        ));

        let summaries = agg.node_summaries();
        assert_eq!(summaries[0].headroom_uw, Some(0));
    }

    #[test]
    fn test_node_summaries_forwards_cpu_source() {
        let mut agg = ClusterAggregator::new();
        let mut node = make_node("node-1", 5000, 2000, 0, None, 1000);
        node.cpu_source = "rapl_msr".into();
        node.cpu_reading_type = "measured".into();
        agg.ingest(make_report(node, vec![]));

        let summaries = agg.node_summaries();
        assert_eq!(summaries[0].cpu_source, "rapl_msr");
        assert_eq!(summaries[0].cpu_reading_type, "measured");
    }

    // ─── node_summary() tests ────────────────────────────────────

    #[test]
    fn test_node_summary_found() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, None, 1000),
            vec![],
        ));

        assert!(agg.node_summary("node-1").is_some());
        assert_eq!(agg.node_summary("node-1").unwrap().cpu_uw, 5000);
    }

    #[test]
    fn test_node_summary_not_found() {
        let agg = ClusterAggregator::new();
        assert!(agg.node_summary("nonexistent").is_none());
    }

    // ─── pod_power() tests ───────────────────────────────────────

    #[test]
    fn test_pod_power_found() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 5000, 2000, 0, None, 1000),
            vec![make_pod("node-1", "uid-a", "web-1", "default", 1234, 567, 89)],
        ));

        let pod = agg.pod_power("uid-a").unwrap();
        assert_eq!(pod.cpu_uw, 1234);
        assert_eq!(pod.memory_uw, 567);
        assert_eq!(pod.gpu_uw, 89);
        assert_eq!(pod.total_uw, 1890);
    }

    #[test]
    fn test_pod_power_not_found() {
        let agg = ClusterAggregator::new();
        assert!(agg.pod_power("nonexistent").is_none());
    }

    // ─── namespace_history() tests ───────────────────────────────

    #[test]
    fn test_namespace_history_filtering() {
        let mut agg = ClusterAggregator::new();
        let now = SystemTime::now();

        // Insert history entries manually
        agg.history.insert("prod".into(), vec![
            (now - Duration::from_secs(100), 5000),
            (now - Duration::from_secs(50), 6000),
            (now - Duration::from_secs(10), 7000),
        ]);

        // Only entries after 60 seconds ago
        let since = now - Duration::from_secs(60);
        let history = agg.namespace_history("prod", since);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].1, 6000);
        assert_eq!(history[1].1, 7000);
    }

    #[test]
    fn test_namespace_history_nonexistent_namespace() {
        let agg = ClusterAggregator::new();
        let since = SystemTime::now() - Duration::from_secs(3600);
        assert!(agg.namespace_history("nonexistent", since).is_empty());
    }

    // ─── cpu_source_info() and memory_source_info() tests ────────

    #[test]
    fn test_cpu_source_info_no_nodes() {
        let agg = ClusterAggregator::new();
        let (source, reading_type) = agg.cpu_source_info();
        assert_eq!(source, "unknown");
        assert_eq!(reading_type, "none");
    }

    #[test]
    fn test_memory_source_info_no_nodes() {
        let agg = ClusterAggregator::new();
        assert_eq!(agg.memory_source_info(), "unknown");
    }

    #[test]
    fn test_cpu_source_info_with_node() {
        let mut agg = ClusterAggregator::new();
        let mut node = make_node("node-1", 5000, 2000, 0, None, 1000);
        node.cpu_source = "rapl_sysfs".into();
        node.cpu_reading_type = "estimated".into();
        agg.ingest(make_report(node, vec![]));

        let (source, reading_type) = agg.cpu_source_info();
        assert_eq!(source, "rapl_sysfs");
        assert_eq!(reading_type, "estimated");
    }

    // ─── all_sources() tests ─────────────────────────────────────

    #[test]
    fn test_all_sources_empty() {
        let agg = ClusterAggregator::new();
        assert!(agg.all_sources().is_empty());
    }

    #[test]
    fn test_all_sources_merged_from_nodes() {
        let mut agg = ClusterAggregator::new();
        let mut node1 = make_node("node-1", 5000, 2000, 0, None, 1000);
        node1.sources = vec![SourceStatus {
            name: "rapl".into(),
            node_name: "node-1".into(),
            component: "cpu".into(),
            reading_type: "estimated".into(),
            available: true,
            selected: true,
            power_uw: 5000,
        }];
        let mut node2 = make_node("node-2", 3000, 1000, 0, None, 800);
        node2.sources = vec![SourceStatus {
            name: "hwmon".into(),
            node_name: "node-2".into(),
            component: "cpu".into(),
            reading_type: "estimated".into(),
            available: true,
            selected: true,
            power_uw: 3000,
        }];

        agg.ingest(make_report(node1, vec![]));
        agg.ingest(make_report(node2, vec![]));

        assert_eq!(agg.all_sources().len(), 2);
    }

    // ─── node_count() tests ──────────────────────────────────────

    #[test]
    fn test_node_count() {
        let mut agg = ClusterAggregator::new();
        assert_eq!(agg.node_count(), 0);

        agg.ingest(make_report(make_node("node-1", 5000, 2000, 0, None, 1000), vec![]));
        assert_eq!(agg.node_count(), 1);

        agg.ingest(make_report(make_node("node-2", 3000, 1000, 0, None, 800), vec![]));
        assert_eq!(agg.node_count(), 2);
    }

    // ─── Edge cases ──────────────────────────────────────────────

    #[test]
    fn test_ingest_empty_pods() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(make_node("node-1", 5000, 2000, 0, None, 1000), vec![]));
        assert_eq!(agg.nodes.len(), 1);
        assert_eq!(agg.pods.len(), 0);
    }

    #[test]
    fn test_large_power_values() {
        let mut agg = ClusterAggregator::new();
        let large = u64::MAX / 4;
        agg.ingest(make_report(
            make_node("node-1", large, large, 0, Some(u64::MAX / 2), large),
            vec![make_pod("node-1", "uid-a", "big", "default", large, large, 0)],
        ));

        let power = agg.cluster_power();
        assert_eq!(power.cpu_uw, large);
        assert_eq!(power.memory_uw, large);
    }

    #[test]
    fn test_zero_power_values() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(
            make_node("node-1", 0, 0, 0, Some(0), 0),
            vec![make_pod("node-1", "uid-a", "idle", "default", 0, 0, 0)],
        ));

        let power = agg.cluster_power();
        assert_eq!(power.total_attributed_uw, 0);
        assert_eq!(power.pod_count, 1);
    }

    #[test]
    fn test_repeated_node_updates_replace_state() {
        let mut agg = ClusterAggregator::new();
        agg.ingest(make_report(make_node("node-1", 5000, 2000, 0, None, 1000), vec![]));
        agg.ingest(make_report(make_node("node-1", 9000, 4000, 0, None, 2000), vec![]));

        let power = agg.cluster_power();
        assert_eq!(power.cpu_uw, 9000);
        assert_eq!(power.node_count, 1);
    }
}
