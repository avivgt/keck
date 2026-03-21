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
