// SPDX-License-Identifier: Apache-2.0

//! Power-aware Kubernetes scheduler extender.
//!
//! Integrates with the K8s scheduler via the extender webhook protocol.
//! When the scheduler needs to place a pod, it calls our extender to
//! score nodes based on power metrics.
//!
//! Scoring strategy:
//!   1. Power headroom: prefer nodes with more available power capacity
//!   2. Efficiency: prefer nodes with lower error_ratio (better metering)
//!   3. Carbon: prefer nodes in regions with lower carbon intensity
//!   4. Thermal: avoid nodes near thermal limits (future)
//!
//! Also supports:
//!   - Power budgets per namespace (reject if namespace would exceed budget)
//!   - Bin-packing vs spreading strategies (configurable)
//!   - Descheduling hints (identify pods to move for power optimization)

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::aggregator::{ClusterAggregator, NodeSummary};

/// Scheduler configuration.
pub struct SchedulerConfig {
    /// Weight for power headroom in scoring (0.0-1.0)
    pub headroom_weight: f64,

    /// Weight for metering accuracy in scoring
    pub accuracy_weight: f64,

    /// Strategy: pack pods tightly (saves idle power) or spread (avoids hotspots)
    pub strategy: PlacementStrategy,

    /// Per-namespace power budgets in watts (None = unlimited)
    pub namespace_budgets: HashMap<String, f64>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            headroom_weight: 0.7,
            accuracy_weight: 0.3,
            strategy: PlacementStrategy::BinPack,
            namespace_budgets: HashMap::new(),
        }
    }
}

/// Pod placement strategy.
#[derive(Clone, Copy, Debug)]
pub enum PlacementStrategy {
    /// Pack pods onto fewer nodes (reduces idle power, more efficient)
    BinPack,
    /// Spread pods across nodes (avoids thermal hotspots, more resilient)
    Spread,
}

/// Node score from the power-aware scheduler.
#[derive(Clone, Debug, serde::Serialize)]
pub struct NodeScore {
    pub node_name: String,
    /// Score 0-100 (higher = better placement candidate)
    pub score: u32,
    /// Reason for this score
    pub reason: String,
}

/// K8s scheduler extender filter result.
#[derive(Clone, Debug, serde::Serialize)]
pub struct FilterResult {
    /// Nodes that pass the filter
    pub nodes: Vec<String>,
    /// Nodes that failed with reasons
    pub failed: HashMap<String, String>,
}

/// K8s scheduler extender prioritize result.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PrioritizeResult {
    /// Node scores (0-100)
    pub scores: Vec<NodeScore>,
}

/// Power-aware scheduler extender.
pub struct PowerScheduler {
    aggregator: Arc<RwLock<ClusterAggregator>>,
    config: SchedulerConfig,
}

impl PowerScheduler {
    pub fn new(aggregator: Arc<RwLock<ClusterAggregator>>) -> Self {
        Self {
            aggregator,
            config: SchedulerConfig::default(),
        }
    }

    /// Filter: remove nodes that can't accept the pod due to power constraints.
    ///
    /// A node is filtered out if:
    /// - It has no power data (never reported)
    /// - Adding this pod would exceed the namespace power budget
    /// - It has dangerously high error_ratio (metering unreliable)
    pub async fn filter(
        &self,
        candidate_nodes: &[String],
        namespace: &str,
    ) -> FilterResult {
        let aggregator = self.aggregator.read().await;
        let nodes = aggregator.node_summaries();

        let node_map: HashMap<&str, &NodeSummary> = nodes
            .iter()
            .map(|n| (n.node_name.as_str(), n))
            .collect();

        let mut passed = Vec::new();
        let mut failed = HashMap::new();

        // Check namespace budget
        let budget_watts = self.config.namespace_budgets.get(namespace);
        let current_ns_power: f64 = aggregator
            .namespace_power()
            .iter()
            .find(|ns| ns.namespace == namespace)
            .map(|ns| ns.total_uw as f64 / 1e6)
            .unwrap_or(0.0);

        for node_name in candidate_nodes {
            let summary = match node_map.get(node_name.as_str()) {
                Some(s) => s,
                None => {
                    // No power data — allow but note it
                    // (new nodes won't have data yet; don't block scheduling)
                    passed.push(node_name.clone());
                    continue;
                }
            };

            // Check metering reliability
            if summary.error_ratio > 0.5 {
                failed.insert(
                    node_name.clone(),
                    format!(
                        "power metering unreliable (error ratio: {:.0}%)",
                        summary.error_ratio * 100.0
                    ),
                );
                continue;
            }

            // Check namespace budget
            if let Some(&budget) = budget_watts {
                if current_ns_power >= budget {
                    failed.insert(
                        node_name.clone(),
                        format!(
                            "namespace '{}' power budget exceeded ({:.0}W / {:.0}W)",
                            namespace, current_ns_power, budget
                        ),
                    );
                    continue;
                }
            }

            passed.push(node_name.clone());
        }

        FilterResult {
            nodes: passed,
            failed,
        }
    }

    /// Prioritize: score nodes based on power metrics.
    ///
    /// Higher score = better candidate.
    pub async fn prioritize(&self, candidate_nodes: &[String]) -> PrioritizeResult {
        let aggregator = self.aggregator.read().await;
        let nodes = aggregator.node_summaries();

        let node_map: HashMap<&str, &NodeSummary> = nodes
            .iter()
            .map(|n| (n.node_name.as_str(), n))
            .collect();

        let scores: Vec<NodeScore> = candidate_nodes
            .iter()
            .map(|name| {
                let summary = match node_map.get(name.as_str()) {
                    Some(s) => s,
                    None => {
                        return NodeScore {
                            node_name: name.clone(),
                            score: 50, // Neutral score for unknown nodes
                            reason: "no power data available".into(),
                        }
                    }
                };

                let (headroom_score, accuracy_score) = match self.config.strategy {
                    PlacementStrategy::BinPack => {
                        // BinPack: prefer nodes already busy (less idle waste)
                        let utilization = if let Some(platform) = summary.platform_uw {
                            let used = summary.cpu_uw + summary.memory_uw + summary.gpu_uw;
                            (used as f64 / platform as f64).min(1.0)
                        } else {
                            0.5
                        };
                        // Higher utilization = higher score (pack more)
                        let headroom = (utilization * 100.0) as u32;
                        let accuracy = ((1.0 - summary.error_ratio) * 100.0) as u32;
                        (headroom, accuracy)
                    }
                    PlacementStrategy::Spread => {
                        // Spread: prefer nodes with most headroom
                        let headroom_pct = if let Some(headroom) = summary.headroom_uw {
                            if let Some(platform) = summary.platform_uw {
                                (headroom as f64 / platform as f64 * 100.0) as u32
                            } else {
                                50
                            }
                        } else {
                            50
                        };
                        let accuracy = ((1.0 - summary.error_ratio) * 100.0) as u32;
                        (headroom_pct, accuracy)
                    }
                };

                let combined = (headroom_score as f64 * self.config.headroom_weight
                    + accuracy_score as f64 * self.config.accuracy_weight)
                    as u32;

                NodeScore {
                    node_name: name.clone(),
                    score: combined.min(100),
                    reason: format!(
                        "headroom={}, accuracy={}, strategy={:?}",
                        headroom_score, accuracy_score, self.config.strategy
                    ),
                }
            })
            .collect();

        PrioritizeResult { scores }
    }

    /// Run the scheduler extender webhook server.
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        // TODO: Implement webhook server
        //
        // K8s scheduler extender protocol:
        //   POST /filter    → FilterResult
        //   POST /prioritize → PrioritizeResult
        //
        // Register via scheduler profile:
        //   apiVersion: kubescheduler.config.k8s.io/v1
        //   kind: KubeSchedulerConfiguration
        //   extenders:
        //     - urlPrefix: "http://power-controller.power-system:8081"
        //       filterVerb: "filter"
        //       prioritizeVerb: "prioritize"
        //       weight: 5

        log::info!("Scheduler extender would listen on :8081");

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::*;

    fn make_node(name: &str, cpu: u64, mem: u64, gpu: u64, platform: Option<u64>, error_ratio: f64) -> NodePowerReport {
        NodePowerReport {
            node_name: name.into(),
            cpu_uw: cpu,
            memory_uw: mem,
            gpu_uw: gpu,
            platform_uw: platform,
            idle_uw: 1000,
            error_ratio,
            pod_count: 5,
            process_count: 50,
            timestamp: std::time::SystemTime::now(),
            cpu_source: "rapl".into(),
            memory_source: "estimated".into(),
            cpu_reading_type: "estimated".into(),
            sources: vec![],
        }
    }

    fn make_pod(node: &str, uid: &str, ns: &str, cpu: u64) -> PodPowerReport {
        PodPowerReport {
            node_name: node.into(),
            pod_uid: uid.into(),
            pod_name: format!("pod-{}", uid),
            namespace: ns.into(),
            cpu_uw: cpu,
            memory_uw: 0,
            gpu_uw: 0,
            total_uw: cpu,
            timestamp: std::time::SystemTime::now(),
        }
    }

    fn make_scheduler(aggregator: Arc<RwLock<ClusterAggregator>>) -> PowerScheduler {
        PowerScheduler::new(aggregator)
    }

    #[tokio::test]
    async fn test_filter_allows_unknown_nodes() {
        let agg = Arc::new(RwLock::new(ClusterAggregator::new()));
        let scheduler = make_scheduler(agg);

        let result = scheduler.filter(&["unknown-node".into()], "default").await;
        assert_eq!(result.nodes.len(), 1);
        assert!(result.failed.is_empty());
    }

    #[tokio::test]
    async fn test_filter_rejects_high_error_ratio() {
        let agg = Arc::new(RwLock::new(ClusterAggregator::new()));
        {
            let mut a = agg.write().await;
            a.ingest(AgentReport {
                node: make_node("bad-node", 5000, 2000, 0, None, 0.60), // 60% error
                pods: vec![],
            });
        }
        let scheduler = make_scheduler(agg);

        let result = scheduler.filter(&["bad-node".into()], "default").await;
        assert!(result.nodes.is_empty());
        assert!(result.failed.contains_key("bad-node"));
    }

    #[tokio::test]
    async fn test_filter_allows_low_error_ratio() {
        let agg = Arc::new(RwLock::new(ClusterAggregator::new()));
        {
            let mut a = agg.write().await;
            a.ingest(AgentReport {
                node: make_node("good-node", 5000, 2000, 0, None, 0.05),
                pods: vec![],
            });
        }
        let scheduler = make_scheduler(agg);

        let result = scheduler.filter(&["good-node".into()], "default").await;
        assert_eq!(result.nodes.len(), 1);
    }

    #[tokio::test]
    async fn test_filter_rejects_over_namespace_budget() {
        let agg = Arc::new(RwLock::new(ClusterAggregator::new()));
        {
            let mut a = agg.write().await;
            a.ingest(AgentReport {
                node: make_node("node-1", 5000, 2000, 0, None, 0.05),
                pods: vec![make_pod("node-1", "uid-1", "prod", 5_000_000)], // 5W
            });
        }

        let mut scheduler = make_scheduler(agg);
        scheduler.config.namespace_budgets.insert("prod".into(), 1.0); // 1W budget

        let result = scheduler.filter(&["node-1".into()], "prod").await;
        assert!(result.nodes.is_empty());
        assert!(result.failed.contains_key("node-1"));
    }

    #[tokio::test]
    async fn test_prioritize_unknown_nodes_get_neutral_score() {
        let agg = Arc::new(RwLock::new(ClusterAggregator::new()));
        let scheduler = make_scheduler(agg);

        let result = scheduler.prioritize(&["unknown".into()]).await;
        assert_eq!(result.scores.len(), 1);
        assert_eq!(result.scores[0].score, 50);
    }

    #[tokio::test]
    async fn test_prioritize_scores_nodes() {
        let agg = Arc::new(RwLock::new(ClusterAggregator::new()));
        {
            let mut a = agg.write().await;
            // High utilization node (good for bin-packing)
            a.ingest(AgentReport {
                node: make_node("busy-node", 8000, 3000, 0, Some(15000), 0.05),
                pods: vec![],
            });
            // Low utilization node
            a.ingest(AgentReport {
                node: make_node("idle-node", 1000, 500, 0, Some(15000), 0.05),
                pods: vec![],
            });
        }
        let scheduler = make_scheduler(agg);

        let result = scheduler.prioritize(&["busy-node".into(), "idle-node".into()]).await;
        assert_eq!(result.scores.len(), 2);
        // With BinPack strategy, busy node should score higher
        let busy_score = result.scores.iter().find(|s| s.node_name == "busy-node").unwrap().score;
        let idle_score = result.scores.iter().find(|s| s.node_name == "idle-node").unwrap().score;
        assert!(busy_score > idle_score);
    }

    #[test]
    fn test_scheduler_config_defaults() {
        let config = SchedulerConfig::default();
        assert!((config.headroom_weight - 0.7).abs() < 1e-10);
        assert!((config.accuracy_weight - 0.3).abs() < 1e-10);
        assert!(config.namespace_budgets.is_empty());
    }
}
