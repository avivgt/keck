// SPDX-License-Identifier: Apache-2.0

//! Cluster controller — aggregates power data from all node agents.
//!
//! Agents POST their power reports to /api/v1/report.
//! UI and dashboards read from /api/v1/cluster, /namespaces, /nodes, /pods.

mod aggregator;
mod api;
mod application;
mod carbon;
mod scheduler;

use std::sync::Arc;

use aggregator::{ApplicationDef, ClusterAggregator};
use log::info;
use tokio::sync::RwLock;

pub type AppDefs = Arc<std::sync::RwLock<Vec<ApplicationDef>>>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    info!("Starting Keck cluster controller");

    let aggregator = Arc::new(RwLock::new(ClusterAggregator::new()));
    let app_defs: AppDefs = Arc::new(std::sync::RwLock::new(Vec::new()));

    // Start background KeckApplication CRD watcher (writes to app_defs, not aggregator)
    let app_defs_clone = app_defs.clone();
    tokio::spawn(async move {
        application::watch_applications(app_defs_clone).await;
    });

    // Start REST API (handles both agent reports and UI queries)
    api::start_rest_server(aggregator, app_defs, "0.0.0.0:8080").await?;

    Ok(())
}
