// SPDX-License-Identifier: Apache-2.0

//! Cluster controller — aggregates power data from all node agents.
//!
//! Runs as a single Deployment (not DaemonSet) in the K8s cluster.
//!
//! Responsibilities:
//! 1. Receive pod-level power summaries from node agents via gRPC
//! 2. Aggregate: pod → deployment → namespace → cluster
//! 3. Integrate carbon intensity data (grid carbon per kWh)
//! 4. Compute cost (energy × $/kWh)
//! 5. Expose K8s custom metrics API (for HPA and scheduler)
//! 6. Power-aware scheduler extender
//! 7. Forward cluster summaries to fleet manager (if configured)

mod aggregator;
mod api;
mod carbon;
mod scheduler;

use std::sync::Arc;

use aggregator::ClusterAggregator;
use carbon::CarbonTracker;
use log::info;
use scheduler::PowerScheduler;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    info!("Starting cluster controller");

    // Shared state: the aggregator holds all cluster power data
    let aggregator = Arc::new(RwLock::new(ClusterAggregator::new()));

    // Carbon intensity tracker
    let carbon = Arc::new(RwLock::new(CarbonTracker::new()));

    // Start gRPC server to receive agent reports
    let grpc_aggregator = aggregator.clone();
    let grpc_handle = tokio::spawn(async move {
        if let Err(e) = api::start_grpc_server(grpc_aggregator, "[::]:9090").await {
            log::error!("gRPC server failed: {}", e);
        }
    });

    // Start REST/metrics API
    let rest_aggregator = aggregator.clone();
    let rest_carbon = carbon.clone();
    let rest_handle = tokio::spawn(async move {
        if let Err(e) = api::start_rest_server(rest_aggregator, rest_carbon, "0.0.0.0:8080").await
        {
            log::error!("REST server failed: {}", e);
        }
    });

    // Start K8s custom metrics API adapter
    let metrics_aggregator = aggregator.clone();
    let metrics_handle = tokio::spawn(async move {
        if let Err(e) = api::start_custom_metrics_api(metrics_aggregator).await {
            log::error!("Custom metrics API failed: {}", e);
        }
    });

    // Start scheduler extender
    let sched_aggregator = aggregator.clone();
    let sched_handle = tokio::spawn(async move {
        let scheduler = PowerScheduler::new(sched_aggregator);
        if let Err(e) = scheduler.run().await {
            log::error!("Scheduler extender failed: {}", e);
        }
    });

    // Start carbon intensity updater (polls external API)
    let carbon_handle = tokio::spawn(async move {
        carbon::run_updater(carbon).await;
    });

    info!("Cluster controller running");
    info!("  gRPC:            [::]:9090 (agent reports)");
    info!("  REST/metrics:    0.0.0.0:8080");
    info!("  Custom metrics:  K8s API registration");
    info!("  Scheduler:       extender webhook");

    // Wait for all tasks
    tokio::select! {
        r = grpc_handle => { log::error!("gRPC server exited: {:?}", r); }
        r = rest_handle => { log::error!("REST server exited: {:?}", r); }
        r = metrics_handle => { log::error!("Custom metrics exited: {:?}", r); }
        r = sched_handle => { log::error!("Scheduler exited: {:?}", r); }
        r = carbon_handle => { log::error!("Carbon updater exited: {:?}", r); }
    }

    Ok(())
}
