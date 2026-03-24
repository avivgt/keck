// SPDX-License-Identifier: Apache-2.0

//! Policy engine: enforces power budgets and carbon policies across the fleet.
//!
//! Policies:
//! 1. Power budgets: "Team X gets max 50kW across all clusters"
//! 2. Carbon budgets: "Namespace Y must stay under 100 kgCO2/month"
//! 3. Carbon-aware routing: "Prefer clusters with intensity < 200 gCO2/kWh"
//! 4. Alerts: "Notify if any cluster error_ratio > 20%"
//!
//! Policies are evaluated periodically (every 30s by default).
//! Violations produce alerts that can be:
//! - Logged
//! - Sent to a webhook (Slack, PagerDuty, etc.)
//! - Published as K8s events (via cluster controllers)
//! - Used to block scheduling (via cluster controller's scheduler extender)

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use crate::registry::ClusterRegistry;

/// A policy definition.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Policy {
    pub name: String,
    pub kind: PolicyKind,
    pub severity: Severity,
    pub enabled: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum PolicyKind {
    /// Team-wide power budget in watts across all clusters
    TeamPowerBudget {
        team: String,
        max_watts: f64,
    },

    /// Namespace carbon budget in gCO2eq per month
    NamespaceCarbonBudget {
        namespace: String,
        max_grams_per_month: f64,
    },

    /// Cluster-level metering quality threshold
    MeteringQuality {
        max_error_ratio: f64,
    },

    /// Prefer clusters below a carbon intensity threshold
    CarbonIntensityPreference {
        max_grams_per_kwh: f64,
    },

    /// Alert if a cluster hasn't reported within a threshold
    ClusterStaleness {
        max_stale_secs: u64,
    },
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// Result of evaluating a policy.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PolicyViolation {
    pub policy_name: String,
    pub severity: Severity,
    pub message: String,
    pub current_value: f64,
    pub threshold: f64,
    pub timestamp: DateTime<Utc>,
}

/// Evaluate all policies against current fleet state.
pub fn evaluate(
    registry: &ClusterRegistry,
    policies: &[Policy],
) -> Vec<PolicyViolation> {
    let mut violations = Vec::new();
    let now = Utc::now();

    for policy in policies {
        if !policy.enabled {
            continue;
        }

        match &policy.kind {
            PolicyKind::TeamPowerBudget { team, max_watts } => {
                let team_views = registry.team_power();
                if let Some(view) = team_views.iter().find(|v| v.team == *team) {
                    if view.total_watts > *max_watts {
                        violations.push(PolicyViolation {
                            policy_name: policy.name.clone(),
                            severity: policy.severity,
                            message: format!(
                                "Team '{}' power usage {:.0}W exceeds budget {:.0}W",
                                team, view.total_watts, max_watts
                            ),
                            current_value: view.total_watts,
                            threshold: *max_watts,
                            timestamp: now,
                        });
                    }
                }
            }

            PolicyKind::NamespaceCarbonBudget {
                namespace,
                max_grams_per_month,
            } => {
                // Estimate monthly carbon from current rate
                for cluster in registry.active_clusters() {
                    if let Some(ns) = cluster.namespaces.iter().find(|n| n.namespace == *namespace)
                    {
                        let watts = ns.total_watts;
                        let kwh_per_month = watts / 1000.0 * 24.0 * 30.44;
                        let carbon_per_month =
                            kwh_per_month * cluster.carbon.intensity_grams_per_kwh;

                        if carbon_per_month > *max_grams_per_month {
                            violations.push(PolicyViolation {
                                policy_name: policy.name.clone(),
                                severity: policy.severity,
                                message: format!(
                                    "Namespace '{}' in cluster '{}' projected at {:.0} gCO2/month (budget: {:.0})",
                                    namespace, cluster.cluster_name, carbon_per_month, max_grams_per_month
                                ),
                                current_value: carbon_per_month,
                                threshold: *max_grams_per_month,
                                timestamp: now,
                            });
                        }
                    }
                }
            }

            PolicyKind::MeteringQuality { max_error_ratio } => {
                for cluster in registry.active_clusters() {
                    if cluster.avg_error_ratio > *max_error_ratio {
                        violations.push(PolicyViolation {
                            policy_name: policy.name.clone(),
                            severity: policy.severity,
                            message: format!(
                                "Cluster '{}' metering error {:.1}% exceeds threshold {:.1}%",
                                cluster.cluster_name,
                                cluster.avg_error_ratio * 100.0,
                                max_error_ratio * 100.0
                            ),
                            current_value: cluster.avg_error_ratio,
                            threshold: *max_error_ratio,
                            timestamp: now,
                        });
                    }
                }
            }

            PolicyKind::CarbonIntensityPreference { max_grams_per_kwh } => {
                for cluster in registry.active_clusters() {
                    if cluster.carbon.intensity_grams_per_kwh > *max_grams_per_kwh {
                        violations.push(PolicyViolation {
                            policy_name: policy.name.clone(),
                            severity: policy.severity,
                            message: format!(
                                "Cluster '{}' carbon intensity {:.0} gCO2/kWh exceeds preference {:.0}",
                                cluster.cluster_name,
                                cluster.carbon.intensity_grams_per_kwh,
                                max_grams_per_kwh
                            ),
                            current_value: cluster.carbon.intensity_grams_per_kwh,
                            threshold: *max_grams_per_kwh,
                            timestamp: now,
                        });
                    }
                }
            }

            PolicyKind::ClusterStaleness { max_stale_secs } => {
                // This checks ALL registered clusters, including stale ones
                let summary = registry.fleet_summary();
                for cluster in &summary.clusters {
                    if cluster.last_seen_secs_ago > *max_stale_secs {
                        violations.push(PolicyViolation {
                            policy_name: policy.name.clone(),
                            severity: policy.severity,
                            message: format!(
                                "Cluster '{}' last reported {}s ago (threshold: {}s)",
                                cluster.cluster_name,
                                cluster.last_seen_secs_ago,
                                max_stale_secs
                            ),
                            current_value: cluster.last_seen_secs_ago as f64,
                            threshold: *max_stale_secs as f64,
                            timestamp: now,
                        });
                    }
                }
            }
        }
    }

    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::*;

    fn make_cluster_report(name: &str, total_watts: f64, carbon_intensity: f64, error_ratio: f64) -> ClusterReport {
        ClusterReport {
            cluster_id: format!("id-{}", name),
            cluster_name: name.into(),
            region: "us-east-1".into(),
            provider: "bare-metal".into(),
            power: ClusterPowerSummary {
                total_watts,
                cpu_watts: total_watts * 0.6,
                memory_watts: total_watts * 0.2,
                gpu_watts: 0.0,
                idle_watts: total_watts * 0.2,
                platform_watts: None,
            },
            namespaces: vec![
                NamespaceSummary { namespace: "prod".into(), total_watts: total_watts * 0.8, pod_count: 10 },
            ],
            carbon: CarbonData {
                intensity_grams_per_kwh: carbon_intensity,
                emissions_grams_per_hour: total_watts / 1000.0 * carbon_intensity,
                cost_per_kwh: 0.10,
                currency: "USD".into(),
            },
            node_count: 3,
            pod_count: 10,
            avg_error_ratio: error_ratio,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_evaluate_empty_no_violations() {
        let reg = ClusterRegistry::new();
        let policies = vec![];
        let violations = evaluate(&reg, &policies);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_disabled_policy_skipped() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("prod", 1000.0, 400.0, 0.50));

        let policies = vec![Policy {
            name: "quality".into(),
            kind: PolicyKind::MeteringQuality { max_error_ratio: 0.20 },
            severity: Severity::Warning,
            enabled: false, // Disabled
        }];

        let violations = evaluate(&reg, &policies);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_metering_quality_violation() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("prod", 1000.0, 400.0, 0.30)); // 30% error

        let policies = vec![Policy {
            name: "quality".into(),
            kind: PolicyKind::MeteringQuality { max_error_ratio: 0.20 },
            severity: Severity::Warning,
            enabled: true,
        }];

        let violations = evaluate(&reg, &policies);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].policy_name, "quality");
        assert!(violations[0].current_value > 0.20);
    }

    #[test]
    fn test_metering_quality_pass() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("prod", 1000.0, 400.0, 0.05)); // 5% error

        let policies = vec![Policy {
            name: "quality".into(),
            kind: PolicyKind::MeteringQuality { max_error_ratio: 0.20 },
            severity: Severity::Warning,
            enabled: true,
        }];

        let violations = evaluate(&reg, &policies);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_team_power_budget_violation() {
        let mut reg = ClusterRegistry::new();
        reg.set_namespace_owner("prod", "team-x");
        reg.ingest(make_cluster_report("prod-cluster", 10000.0, 400.0, 0.05));

        let policies = vec![Policy {
            name: "team-budget".into(),
            kind: PolicyKind::TeamPowerBudget {
                team: "team-x".into(),
                max_watts: 5000.0,
            },
            severity: Severity::Critical,
            enabled: true,
        }];

        let violations = evaluate(&reg, &policies);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("team-x"));
    }

    #[test]
    fn test_team_power_budget_pass() {
        let mut reg = ClusterRegistry::new();
        reg.set_namespace_owner("prod", "team-x");
        reg.ingest(make_cluster_report("prod-cluster", 1000.0, 400.0, 0.05));

        let policies = vec![Policy {
            name: "team-budget".into(),
            kind: PolicyKind::TeamPowerBudget {
                team: "team-x".into(),
                max_watts: 50000.0,
            },
            severity: Severity::Critical,
            enabled: true,
        }];

        let violations = evaluate(&reg, &policies);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_carbon_intensity_preference_violation() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("dirty-cluster", 1000.0, 800.0, 0.05));

        let policies = vec![Policy {
            name: "carbon-pref".into(),
            kind: PolicyKind::CarbonIntensityPreference { max_grams_per_kwh: 300.0 },
            severity: Severity::Info,
            enabled: true,
        }];

        let violations = evaluate(&reg, &policies);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].message.contains("dirty-cluster"));
    }

    #[test]
    fn test_carbon_intensity_preference_pass() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("clean-cluster", 1000.0, 100.0, 0.05));

        let policies = vec![Policy {
            name: "carbon-pref".into(),
            kind: PolicyKind::CarbonIntensityPreference { max_grams_per_kwh: 300.0 },
            severity: Severity::Info,
            enabled: true,
        }];

        let violations = evaluate(&reg, &policies);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_namespace_carbon_budget_violation() {
        let mut reg = ClusterRegistry::new();
        // 1000W total, prod gets 800W, at 400 gCO2/kWh
        // monthly: 800/1000 * 24 * 30.44 * 400 = ~233_395 gCO2/month
        reg.ingest(make_cluster_report("prod-cluster", 1000.0, 400.0, 0.05));

        let policies = vec![Policy {
            name: "ns-carbon".into(),
            kind: PolicyKind::NamespaceCarbonBudget {
                namespace: "prod".into(),
                max_grams_per_month: 1000.0, // Very low budget
            },
            severity: Severity::Warning,
            enabled: true,
        }];

        let violations = evaluate(&reg, &policies);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn test_multiple_policies_multiple_violations() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("bad-cluster", 1000.0, 800.0, 0.30));

        let policies = vec![
            Policy {
                name: "quality".into(),
                kind: PolicyKind::MeteringQuality { max_error_ratio: 0.20 },
                severity: Severity::Warning,
                enabled: true,
            },
            Policy {
                name: "carbon".into(),
                kind: PolicyKind::CarbonIntensityPreference { max_grams_per_kwh: 300.0 },
                severity: Severity::Info,
                enabled: true,
            },
        ];

        let violations = evaluate(&reg, &policies);
        assert_eq!(violations.len(), 2);
    }
}

/// Run the policy engine loop.
pub async fn run_policy_engine(registry: Arc<RwLock<ClusterRegistry>>) {
    // TODO: Load policies from config file or K8s CRD
    let policies = vec![
        Policy {
            name: "metering-quality".into(),
            kind: PolicyKind::MeteringQuality {
                max_error_ratio: 0.20,
            },
            severity: Severity::Warning,
            enabled: true,
        },
        Policy {
            name: "cluster-staleness".into(),
            kind: PolicyKind::ClusterStaleness {
                max_stale_secs: 120,
            },
            severity: Severity::Critical,
            enabled: true,
        },
    ];

    let eval_interval = Duration::from_secs(30);

    loop {
        tokio::time::sleep(eval_interval).await;

        let reg = registry.read().await;
        let violations = evaluate(&reg, &policies);

        for v in &violations {
            match v.severity {
                Severity::Critical => log::error!("[POLICY] {}: {}", v.policy_name, v.message),
                Severity::Warning => log::warn!("[POLICY] {}: {}", v.policy_name, v.message),
                Severity::Info => log::info!("[POLICY] {}: {}", v.policy_name, v.message),
            }
        }

        if violations.is_empty() {
            log::debug!("Policy evaluation: all {} policies passed", policies.len());
        }

        // TODO: Send violations to webhook (Slack, PagerDuty)
        // TODO: Publish as K8s events via cluster controllers
    }
}
