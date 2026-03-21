// SPDX-License-Identifier: Apache-2.0

//! Reporting engine: generates periodic ESG and compliance reports.
//!
//! Report types:
//! 1. Daily summary: fleet power, carbon, cost breakdown by cluster/team
//! 2. Monthly ESG report: total emissions, trends, year-over-year
//! 3. On-demand audit: detailed power data for a date range
//!
//! Output formats:
//! - JSON (for API consumers and dashboards)
//! - CSV (for spreadsheet import)
//!
//! Reports are stored locally and exposed via the REST API.
//! Can also be pushed to external systems (S3, email, webhook).

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use crate::registry::ClusterRegistry;

/// A generated report.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Report {
    pub id: String,
    pub report_type: ReportType,
    pub generated_at: DateTime<Utc>,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub data: ReportData,
}

#[derive(Clone, Debug, serde::Serialize)]
pub enum ReportType {
    DailySummary,
    MonthlyESG,
    OnDemandAudit,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ReportData {
    /// Fleet totals for the period
    pub fleet: FleetPeriodSummary,
    /// Per-cluster breakdown
    pub clusters: Vec<ClusterPeriodSummary>,
    /// Per-team breakdown
    pub teams: Vec<TeamPeriodSummary>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct FleetPeriodSummary {
    /// Total energy consumed in kWh
    pub energy_kwh: f64,
    /// Total carbon emissions in kgCO2eq
    pub carbon_kg: f64,
    /// Total cost
    pub cost: f64,
    pub cost_currency: String,
    /// Average power in watts
    pub avg_power_watts: f64,
    /// Peak power in watts
    pub peak_power_watts: f64,
    /// Average carbon intensity (weighted by energy)
    pub avg_carbon_intensity: f64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ClusterPeriodSummary {
    pub cluster_name: String,
    pub region: String,
    pub energy_kwh: f64,
    pub carbon_kg: f64,
    pub cost: f64,
    pub avg_power_watts: f64,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct TeamPeriodSummary {
    pub team: String,
    pub energy_kwh: f64,
    pub carbon_kg: f64,
    pub cost: f64,
    pub namespace_count: usize,
}

/// Generate a daily summary report from current fleet state.
///
/// In a full implementation, this would aggregate from stored time-series
/// data. For now, it extrapolates from current readings.
pub fn generate_daily_report(registry: &ClusterRegistry) -> Report {
    let now = Utc::now();
    let period_start = now - chrono::Duration::hours(24);

    let fleet = registry.fleet_summary();

    // Extrapolate from current power readings to daily totals
    let hours = 24.0;
    let fleet_energy_kwh = fleet.total_watts / 1000.0 * hours;
    let fleet_carbon_kg = fleet.total_carbon_grams_per_hour * hours / 1000.0;
    let fleet_cost = fleet.total_cost_per_hour * hours;

    let clusters: Vec<ClusterPeriodSummary> = fleet
        .clusters
        .iter()
        .map(|c| {
            let energy = c.total_watts / 1000.0 * hours;
            let carbon = c.carbon_grams_per_hour * hours / 1000.0;
            let cost = c.cost_per_hour * hours;
            ClusterPeriodSummary {
                cluster_name: c.cluster_name.clone(),
                region: c.region.clone(),
                energy_kwh: energy,
                carbon_kg: carbon,
                cost,
                avg_power_watts: c.total_watts,
            }
        })
        .collect();

    let teams: Vec<TeamPeriodSummary> = registry
        .team_power()
        .iter()
        .map(|t| {
            let energy = t.total_watts / 1000.0 * hours;
            let carbon = t.carbon_grams_per_hour * hours / 1000.0;
            let cost = t.cost_per_hour * hours;
            TeamPeriodSummary {
                team: t.team.clone(),
                energy_kwh: energy,
                carbon_kg: carbon,
                cost,
                namespace_count: t.per_cluster.len(),
            }
        })
        .collect();

    let avg_intensity = if fleet_energy_kwh > 0.0 {
        fleet_carbon_kg * 1000.0 / fleet_energy_kwh
    } else {
        0.0
    };

    Report {
        id: format!("daily-{}", now.format("%Y%m%d-%H%M%S")),
        report_type: ReportType::DailySummary,
        generated_at: now,
        period_start,
        period_end: now,
        data: ReportData {
            fleet: FleetPeriodSummary {
                energy_kwh: fleet_energy_kwh,
                carbon_kg: fleet_carbon_kg,
                cost: fleet_cost,
                cost_currency: fleet.total_cost_currency,
                avg_power_watts: fleet.total_watts,
                peak_power_watts: fleet.total_watts, // TODO: track actual peak
                avg_carbon_intensity: avg_intensity,
            },
            clusters,
            teams,
        },
    }
}

/// Background task: generates daily reports automatically.
pub async fn run_report_generator(registry: Arc<RwLock<ClusterRegistry>>) {
    let report_interval = Duration::from_secs(86400); // 24 hours

    // Wait a bit before first report (let data accumulate)
    tokio::time::sleep(Duration::from_secs(60)).await;

    loop {
        {
            let reg = registry.read().await;
            let report = generate_daily_report(&reg);

            log::info!(
                "Generated {} report '{}': {:.0} kWh, {:.1} kgCO2, ${:.2}",
                match report.report_type {
                    ReportType::DailySummary => "daily",
                    ReportType::MonthlyESG => "monthly",
                    ReportType::OnDemandAudit => "audit",
                },
                report.id,
                report.data.fleet.energy_kwh,
                report.data.fleet.carbon_kg,
                report.data.fleet.cost,
            );

            // TODO: Store report persistently
            // TODO: Push to configured destinations (S3, webhook, email)
        }

        tokio::time::sleep(report_interval).await;
    }
}
