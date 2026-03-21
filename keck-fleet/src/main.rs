// SPDX-License-Identifier: Apache-2.0

//! Fleet manager — multi-cluster power observability and governance.
//!
//! Runs as a standalone service (outside any single cluster).
//! Receives cluster summaries from cluster controllers and provides:
//!
//! 1. Multi-cluster aggregation and unified dashboard
//! 2. Power budget enforcement across clusters
//! 3. Carbon-aware workload placement recommendations
//! 4. ESG / regulatory compliance reporting
//! 5. Trend analysis and capacity planning
//!
//! Deployment: runs wherever the fleet operator chooses —
//! a management cluster, a VM, or a cloud service.

mod api;
mod policy;
mod registry;
mod reporting;

use std::sync::Arc;

use log::info;
use registry::ClusterRegistry;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    info!("Starting fleet manager");

    // Shared state: all registered clusters and their power data
    let registry = Arc::new(RwLock::new(ClusterRegistry::new()));

    // gRPC server: receives cluster controller reports
    let grpc_registry = registry.clone();
    let grpc_handle = tokio::spawn(async move {
        if let Err(e) = api::start_grpc_server(grpc_registry, "[::]:9091").await {
            log::error!("gRPC server failed: {}", e);
        }
    });

    // REST API: unified fleet dashboard and reporting endpoints
    let rest_registry = registry.clone();
    let rest_handle = tokio::spawn(async move {
        if let Err(e) = api::start_rest_server(rest_registry, "0.0.0.0:8090").await {
            log::error!("REST server failed: {}", e);
        }
    });

    // Policy engine: evaluates budget/carbon policies periodically
    let policy_registry = registry.clone();
    let policy_handle = tokio::spawn(async move {
        policy::run_policy_engine(policy_registry).await;
    });

    // Reporting engine: generates periodic ESG/compliance reports
    let report_registry = registry.clone();
    let report_handle = tokio::spawn(async move {
        reporting::run_report_generator(report_registry).await;
    });

    info!("Fleet manager running");
    info!("  gRPC:  [::]:9091 (cluster controller reports)");
    info!("  REST:  0.0.0.0:8090 (fleet dashboard + reporting)");

    tokio::select! {
        r = grpc_handle => { log::error!("gRPC exited: {:?}", r); }
        r = rest_handle => { log::error!("REST exited: {:?}", r); }
        r = policy_handle => { log::error!("Policy engine exited: {:?}", r); }
        r = report_handle => { log::error!("Report generator exited: {:?}", r); }
    }

    Ok(())
}
