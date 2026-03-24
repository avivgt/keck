// SPDX-License-Identifier: Apache-2.0

//! Cluster registry: maintains the fleet-wide view of all clusters.
//!
//! Each cluster controller periodically sends a ClusterReport containing:
//! - Cluster identity (name, region, provider)
//! - Cluster-level power summary
//! - Per-namespace power breakdown
//! - Carbon intensity at the cluster's location
//! - Node count and health indicators
//!
//! The registry aggregates this into fleet-wide views for:
//! - Dashboard: total fleet power, per-cluster breakdown
//! - Policy: budget enforcement across clusters
//! - Routing: carbon-aware placement recommendations
//! - Reporting: ESG compliance data

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};

/// Report received from a cluster controller.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ClusterReport {
    /// Unique cluster identifier
    pub cluster_id: String,
    /// Human-readable name
    pub cluster_name: String,
    /// Geographic region (e.g., "us-east-1", "eu-west-1", "on-prem-dc1")
    pub region: String,
    /// Infrastructure provider
    pub provider: String,

    /// Power summary
    pub power: ClusterPowerSummary,

    /// Per-namespace breakdown (top N by power)
    pub namespaces: Vec<NamespaceSummary>,

    /// Carbon data at this cluster's location
    pub carbon: CarbonData,

    /// Infrastructure stats
    pub node_count: u32,
    pub pod_count: u32,
    pub avg_error_ratio: f64,

    /// When this report was generated
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ClusterPowerSummary {
    /// Total power in watts
    pub total_watts: f64,
    pub cpu_watts: f64,
    pub memory_watts: f64,
    pub gpu_watts: f64,
    pub idle_watts: f64,
    pub platform_watts: Option<f64>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NamespaceSummary {
    pub namespace: String,
    pub total_watts: f64,
    pub pod_count: u32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CarbonData {
    /// gCO2eq per kWh at this cluster's location
    pub intensity_grams_per_kwh: f64,
    /// Current carbon emission rate in gCO2eq/hour
    pub emissions_grams_per_hour: f64,
    /// Energy cost at this location ($/kWh)
    pub cost_per_kwh: f64,
    /// Currency
    pub currency: String,
}

/// Internal state for a registered cluster.
struct ClusterState {
    latest_report: ClusterReport,
    received_at: Instant,
    /// Rolling history of power readings for trend analysis
    power_history: Vec<(DateTime<Utc>, f64)>,
    /// Rolling history of carbon emissions
    carbon_history: Vec<(DateTime<Utc>, f64)>,
}

/// Fleet-wide aggregate view.
#[derive(Clone, Debug, serde::Serialize)]
pub struct FleetSummary {
    pub total_watts: f64,
    pub total_cpu_watts: f64,
    pub total_memory_watts: f64,
    pub total_gpu_watts: f64,
    pub total_idle_watts: f64,
    pub total_carbon_grams_per_hour: f64,
    pub total_cost_per_hour: f64,
    pub total_cost_currency: String,
    pub cluster_count: usize,
    pub total_nodes: u32,
    pub total_pods: u32,
    /// Per-cluster breakdown
    pub clusters: Vec<ClusterView>,
}

/// Per-cluster view within the fleet summary.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ClusterView {
    pub cluster_id: String,
    pub cluster_name: String,
    pub region: String,
    pub total_watts: f64,
    pub carbon_grams_per_hour: f64,
    pub carbon_intensity: f64,
    pub cost_per_hour: f64,
    pub node_count: u32,
    pub pod_count: u32,
    pub error_ratio: f64,
    pub last_seen_secs_ago: u64,
}

/// Team/owner power view across all clusters.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TeamPowerView {
    pub team: String,
    pub total_watts: f64,
    pub carbon_grams_per_hour: f64,
    pub cost_per_hour: f64,
    /// Breakdown per cluster
    pub per_cluster: Vec<TeamClusterBreakdown>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct TeamClusterBreakdown {
    pub cluster_name: String,
    pub namespace: String,
    pub watts: f64,
}

/// The fleet cluster registry.
pub struct ClusterRegistry {
    clusters: HashMap<String, ClusterState>,
    staleness_threshold: Duration,
    history_retention: Duration,
    /// Maps namespace → team/owner for cross-cluster team views
    namespace_owners: HashMap<String, String>,
}

impl ClusterRegistry {
    pub fn new() -> Self {
        Self {
            clusters: HashMap::new(),
            staleness_threshold: Duration::from_secs(120), // 2 minutes
            history_retention: Duration::from_secs(86400 * 30), // 30 days
            namespace_owners: HashMap::new(),
        }
    }

    /// Ingest a report from a cluster controller.
    pub fn ingest(&mut self, report: ClusterReport) {
        let now = Instant::now();
        let ts = report.timestamp;
        let total_watts = report.power.total_watts;
        let carbon_rate = report.carbon.emissions_grams_per_hour;

        let state = self
            .clusters
            .entry(report.cluster_id.clone())
            .or_insert_with(|| ClusterState {
                latest_report: report.clone(),
                received_at: now,
                power_history: Vec::new(),
                carbon_history: Vec::new(),
            });

        state.latest_report = report;
        state.received_at = now;
        state.power_history.push((ts, total_watts));
        state.carbon_history.push((ts, carbon_rate));

        // Trim old history
        let cutoff = Utc::now() - chrono::Duration::seconds(self.history_retention.as_secs() as i64);
        state.power_history.retain(|(ts, _)| *ts >= cutoff);
        state.carbon_history.retain(|(ts, _)| *ts >= cutoff);
    }

    /// Set namespace → team ownership mapping.
    /// Called from config or API.
    pub fn set_namespace_owner(&mut self, namespace: &str, team: &str) {
        self.namespace_owners
            .insert(namespace.to_string(), team.to_string());
    }

    /// Get fleet-wide summary.
    pub fn fleet_summary(&self) -> FleetSummary {
        let now = Instant::now();
        let mut total_watts = 0.0;
        let mut total_cpu = 0.0;
        let mut total_memory = 0.0;
        let mut total_gpu = 0.0;
        let mut total_idle = 0.0;
        let mut total_carbon = 0.0;
        let mut total_cost = 0.0;
        let mut total_nodes = 0u32;
        let mut total_pods = 0u32;
        let mut clusters = Vec::new();

        for state in self.clusters.values() {
            let r = &state.latest_report;
            let p = &r.power;

            total_watts += p.total_watts;
            total_cpu += p.cpu_watts;
            total_memory += p.memory_watts;
            total_gpu += p.gpu_watts;
            total_idle += p.idle_watts;
            total_carbon += r.carbon.emissions_grams_per_hour;

            let cost_per_hour = p.total_watts / 1000.0 * r.carbon.cost_per_kwh;
            total_cost += cost_per_hour;
            total_nodes += r.node_count;
            total_pods += r.pod_count;

            let secs_ago = now.duration_since(state.received_at).as_secs();

            clusters.push(ClusterView {
                cluster_id: r.cluster_id.clone(),
                cluster_name: r.cluster_name.clone(),
                region: r.region.clone(),
                total_watts: p.total_watts,
                carbon_grams_per_hour: r.carbon.emissions_grams_per_hour,
                carbon_intensity: r.carbon.intensity_grams_per_kwh,
                cost_per_hour,
                node_count: r.node_count,
                pod_count: r.pod_count,
                error_ratio: r.avg_error_ratio,
                last_seen_secs_ago: secs_ago,
            });
        }

        // Sort by power (highest first)
        clusters.sort_by(|a, b| b.total_watts.partial_cmp(&a.total_watts).unwrap());

        FleetSummary {
            total_watts,
            total_cpu_watts: total_cpu,
            total_memory_watts: total_memory,
            total_gpu_watts: total_gpu,
            total_idle_watts: total_idle,
            total_carbon_grams_per_hour: total_carbon,
            total_cost_per_hour: total_cost,
            total_cost_currency: "USD".into(), // TODO: multi-currency
            cluster_count: self.clusters.len(),
            total_nodes,
            total_pods,
            clusters,
        }
    }

    /// Get per-team power view across all clusters.
    pub fn team_power(&self) -> Vec<TeamPowerView> {
        let mut team_map: HashMap<String, TeamPowerView> = HashMap::new();

        for state in self.clusters.values() {
            let r = &state.latest_report;

            for ns in &r.namespaces {
                let team = self
                    .namespace_owners
                    .get(&ns.namespace)
                    .cloned()
                    .unwrap_or_else(|| "unassigned".into());

                let cost_per_hour = ns.total_watts / 1000.0 * r.carbon.cost_per_kwh;
                let carbon = ns.total_watts / 1000.0 * r.carbon.intensity_grams_per_kwh;

                let entry = team_map.entry(team.clone()).or_insert_with(|| TeamPowerView {
                    team: team.clone(),
                    total_watts: 0.0,
                    carbon_grams_per_hour: 0.0,
                    cost_per_hour: 0.0,
                    per_cluster: Vec::new(),
                });

                entry.total_watts += ns.total_watts;
                entry.carbon_grams_per_hour += carbon;
                entry.cost_per_hour += cost_per_hour;
                entry.per_cluster.push(TeamClusterBreakdown {
                    cluster_name: r.cluster_name.clone(),
                    namespace: ns.namespace.clone(),
                    watts: ns.total_watts,
                });
            }
        }

        let mut result: Vec<TeamPowerView> = team_map.into_values().collect();
        result.sort_by(|a, b| b.total_watts.partial_cmp(&a.total_watts).unwrap());
        result
    }

    /// Get power history for a cluster (for trend charts).
    pub fn cluster_power_history(
        &self,
        cluster_id: &str,
        since: DateTime<Utc>,
    ) -> Vec<(DateTime<Utc>, f64)> {
        self.clusters
            .get(cluster_id)
            .map(|s| {
                s.power_history
                    .iter()
                    .filter(|(ts, _)| *ts >= since)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Find the cluster with lowest carbon intensity (for carbon-aware routing).
    pub fn lowest_carbon_cluster(&self) -> Option<&ClusterReport> {
        self.clusters
            .values()
            .filter(|s| s.received_at.elapsed() < self.staleness_threshold)
            .min_by(|a, b| {
                a.latest_report
                    .carbon
                    .intensity_grams_per_kwh
                    .partial_cmp(&b.latest_report.carbon.intensity_grams_per_kwh)
                    .unwrap()
            })
            .map(|s| &s.latest_report)
    }

    /// Get all active cluster reports.
    pub fn active_clusters(&self) -> Vec<&ClusterReport> {
        self.clusters
            .values()
            .filter(|s| s.received_at.elapsed() < self.staleness_threshold)
            .map(|s| &s.latest_report)
            .collect()
    }

    /// Evict stale clusters.
    pub fn evict_stale(&mut self) {
        let threshold = self.staleness_threshold;
        self.clusters
            .retain(|_, state| state.received_at.elapsed() < threshold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cluster_report(id: &str, name: &str, region: &str, total_watts: f64, carbon_intensity: f64) -> ClusterReport {
        ClusterReport {
            cluster_id: id.into(),
            cluster_name: name.into(),
            region: region.into(),
            provider: "bare-metal".into(),
            power: ClusterPowerSummary {
                total_watts,
                cpu_watts: total_watts * 0.6,
                memory_watts: total_watts * 0.2,
                gpu_watts: total_watts * 0.1,
                idle_watts: total_watts * 0.1,
                platform_watts: Some(total_watts * 1.2),
            },
            namespaces: vec![
                NamespaceSummary { namespace: "prod".into(), total_watts: total_watts * 0.7, pod_count: 10 },
                NamespaceSummary { namespace: "staging".into(), total_watts: total_watts * 0.3, pod_count: 5 },
            ],
            carbon: CarbonData {
                intensity_grams_per_kwh: carbon_intensity,
                emissions_grams_per_hour: total_watts / 1000.0 * carbon_intensity,
                cost_per_kwh: 0.10,
                currency: "USD".into(),
            },
            node_count: 3,
            pod_count: 15,
            avg_error_ratio: 0.05,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_registry_new_empty() {
        let reg = ClusterRegistry::new();
        let summary = reg.fleet_summary();
        assert_eq!(summary.cluster_count, 0);
        assert_eq!(summary.total_watts, 0.0);
    }

    #[test]
    fn test_ingest_single_cluster() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("c1", "prod-us", "us-east-1", 1000.0, 400.0));

        let summary = reg.fleet_summary();
        assert_eq!(summary.cluster_count, 1);
        assert!((summary.total_watts - 1000.0).abs() < 1e-6);
        assert_eq!(summary.total_nodes, 3);
        assert_eq!(summary.total_pods, 15);
    }

    #[test]
    fn test_ingest_multiple_clusters() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("c1", "prod-us", "us-east-1", 1000.0, 400.0));
        reg.ingest(make_cluster_report("c2", "prod-eu", "eu-west-1", 500.0, 200.0));

        let summary = reg.fleet_summary();
        assert_eq!(summary.cluster_count, 2);
        assert!((summary.total_watts - 1500.0).abs() < 1e-6);
        assert_eq!(summary.total_nodes, 6);
        assert_eq!(summary.total_pods, 30);
    }

    #[test]
    fn test_ingest_updates_existing_cluster() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("c1", "prod-us", "us-east-1", 1000.0, 400.0));
        reg.ingest(make_cluster_report("c1", "prod-us", "us-east-1", 2000.0, 400.0));

        let summary = reg.fleet_summary();
        assert_eq!(summary.cluster_count, 1);
        assert!((summary.total_watts - 2000.0).abs() < 1e-6);
    }

    #[test]
    fn test_fleet_summary_sorted_by_power() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("c1", "low", "us-east-1", 100.0, 400.0));
        reg.ingest(make_cluster_report("c2", "high", "eu-west-1", 5000.0, 200.0));
        reg.ingest(make_cluster_report("c3", "mid", "ap-south-1", 1000.0, 600.0));

        let summary = reg.fleet_summary();
        assert_eq!(summary.clusters[0].cluster_name, "high");
        assert_eq!(summary.clusters[1].cluster_name, "mid");
        assert_eq!(summary.clusters[2].cluster_name, "low");
    }

    #[test]
    fn test_fleet_summary_cost_calculation() {
        let mut reg = ClusterRegistry::new();
        // 1000W at $0.10/kWh => cost_per_hour = 1kW * $0.10 = $0.10/hour
        reg.ingest(make_cluster_report("c1", "prod", "us-east-1", 1000.0, 400.0));

        let summary = reg.fleet_summary();
        assert!((summary.total_cost_per_hour - 0.10).abs() < 1e-6);
    }

    #[test]
    fn test_team_power_with_owners() {
        let mut reg = ClusterRegistry::new();
        reg.set_namespace_owner("prod", "team-a");
        reg.set_namespace_owner("staging", "team-b");
        reg.ingest(make_cluster_report("c1", "prod-us", "us-east-1", 1000.0, 400.0));

        let teams = reg.team_power();
        assert_eq!(teams.len(), 2);
        // Teams should be sorted by total_watts desc
        let team_a = teams.iter().find(|t| t.team == "team-a").unwrap();
        let team_b = teams.iter().find(|t| t.team == "team-b").unwrap();
        assert!(team_a.total_watts > team_b.total_watts);
    }

    #[test]
    fn test_team_power_unassigned() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("c1", "prod-us", "us-east-1", 1000.0, 400.0));

        let teams = reg.team_power();
        // All namespaces should be "unassigned"
        for team in &teams {
            assert_eq!(team.team, "unassigned");
        }
    }

    #[test]
    fn test_lowest_carbon_cluster() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("c1", "dirty", "us-east-1", 1000.0, 600.0));
        reg.ingest(make_cluster_report("c2", "clean", "eu-west-1", 1000.0, 100.0));
        reg.ingest(make_cluster_report("c3", "medium", "ap-south-1", 1000.0, 300.0));

        let lowest = reg.lowest_carbon_cluster().unwrap();
        assert_eq!(lowest.cluster_name, "clean");
        assert!((lowest.carbon.intensity_grams_per_kwh - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_lowest_carbon_cluster_empty() {
        let reg = ClusterRegistry::new();
        assert!(reg.lowest_carbon_cluster().is_none());
    }

    #[test]
    fn test_active_clusters() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("c1", "prod", "us-east-1", 1000.0, 400.0));

        let active = reg.active_clusters();
        assert_eq!(active.len(), 1);
    }

    #[test]
    fn test_cluster_power_history() {
        let mut reg = ClusterRegistry::new();
        reg.ingest(make_cluster_report("c1", "prod", "us-east-1", 1000.0, 400.0));
        reg.ingest(make_cluster_report("c1", "prod", "us-east-1", 1500.0, 400.0));

        let since = Utc::now() - chrono::Duration::seconds(60);
        let history = reg.cluster_power_history("c1", since);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_cluster_power_history_nonexistent() {
        let reg = ClusterRegistry::new();
        let since = Utc::now() - chrono::Duration::seconds(60);
        assert!(reg.cluster_power_history("nonexistent", since).is_empty());
    }

    #[test]
    fn test_set_namespace_owner() {
        let mut reg = ClusterRegistry::new();
        reg.set_namespace_owner("prod", "platform-team");

        // Verify via team_power after ingesting data
        reg.ingest(make_cluster_report("c1", "prod-us", "us-east-1", 1000.0, 400.0));
        let teams = reg.team_power();
        let platform = teams.iter().find(|t| t.team == "platform-team");
        assert!(platform.is_some());
    }
}
