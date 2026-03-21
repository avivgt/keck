// SPDX-License-Identifier: Apache-2.0

//! Cluster controller — aggregates power data from all node agents.
//!
//! Agents POST their power reports to /api/v1/report.
//! UI and dashboards read from /api/v1/cluster, /namespaces, /nodes, /pods.

mod aggregator;
mod api;
mod carbon;
mod scheduler;

use std::sync::Arc;

use aggregator::ClusterAggregator;
use log::info;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    info!("Starting Keck cluster controller");

    let aggregator = Arc::new(RwLock::new(ClusterAggregator::new()));

    // Start REST API (handles both agent reports and UI queries)
    api::start_rest_server(aggregator, "0.0.0.0:8080").await?;

    Ok(())
}
