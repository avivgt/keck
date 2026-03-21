// SPDX-License-Identifier: Apache-2.0

//! Fleet manager API layer.
//!
//! Two interfaces:
//!
//! 1. gRPC server (port 9091): receives ClusterReport from cluster controllers.
//!    Each cluster controller opens a streaming connection and pushes
//!    reports every cluster_report_interval (default 30s).
//!
//! 2. REST server (port 8090): serves the unified fleet dashboard.
//!    Endpoints:
//!      GET /api/v1/fleet                    — fleet-wide power/carbon/cost summary
//!      GET /api/v1/fleet/clusters           — per-cluster breakdown
//!      GET /api/v1/fleet/clusters/{id}      — single cluster detail
//!      GET /api/v1/fleet/clusters/{id}/history — power trend for a cluster
//!      GET /api/v1/fleet/teams              — per-team power/carbon/cost
//!      GET /api/v1/fleet/teams/{name}       — single team detail
//!      GET /api/v1/fleet/carbon             — carbon routing recommendations
//!      GET /api/v1/fleet/reports            — list generated reports
//!      GET /api/v1/fleet/reports/{id}       — download a specific report
//!      GET /api/v1/fleet/policies           — list active policies
//!      GET /api/v1/fleet/violations         — current policy violations

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::registry::ClusterRegistry;

/// Start the gRPC server that receives cluster controller reports.
pub async fn start_grpc_server(
    registry: Arc<RwLock<ClusterRegistry>>,
    bind_addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Implement tonic gRPC server
    //
    // service FleetService {
    //   rpc ReportCluster(stream ClusterReport) returns (Ack);
    //   rpc GetRoutingAdvice(RoutingRequest) returns (RoutingResponse);
    // }
    //
    // On each incoming ClusterReport:
    //   registry.write().await.ingest(report);
    //
    // GetRoutingAdvice: returns the cluster with lowest carbon intensity
    //   that has sufficient power headroom for the requested workload.

    log::info!("Fleet gRPC server would listen on {}", bind_addr);

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
    }
}

/// Start the REST API for the fleet dashboard.
pub async fn start_rest_server(
    registry: Arc<RwLock<ClusterRegistry>>,
    bind_addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Implement with axum
    //
    // GET /api/v1/fleet → {
    //   "total_watts": 45000.0,
    //   "total_carbon_kg_per_day": 123.4,
    //   "total_cost_per_day": 108.00,
    //   "cluster_count": 5,
    //   "node_count": 340,
    //   "pod_count": 4500,
    //   "clusters": [
    //     { "name": "prod-east", "watts": 18000, "carbon": "low", ... },
    //     { "name": "prod-west", "watts": 12000, "carbon": "medium", ... },
    //     ...
    //   ]
    // }
    //
    // GET /api/v1/fleet/carbon → {
    //   "recommendation": "Route new workloads to 'prod-west'",
    //   "reason": "Lowest carbon intensity: 85 gCO2/kWh (vs fleet avg 310)",
    //   "clusters_ranked": [
    //     { "name": "prod-west", "intensity": 85, "headroom_watts": 5000 },
    //     { "name": "prod-east", "intensity": 420, "headroom_watts": 3000 },
    //     ...
    //   ]
    // }
    //
    // GET /api/v1/fleet/teams → {
    //   "teams": [
    //     { "name": "ml-platform", "watts": 12000, "carbon_kg_day": 45, "cost_day": 28.80 },
    //     { "name": "web-services", "watts": 8000, "carbon_kg_day": 30, "cost_day": 19.20 },
    //     ...
    //   ]
    // }

    log::info!("Fleet REST server would listen on {}", bind_addr);

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
    }
}
