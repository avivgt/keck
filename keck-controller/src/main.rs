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

use aggregator::ClusterAggregator;
use application::ClassificationData;
use log::info;
use tokio::sync::RwLock;

pub type SharedClassification = Arc<std::sync::RwLock<ClassificationData>>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    info!("Starting Keck cluster controller");

    let aggregator = Arc::new(RwLock::new(ClusterAggregator::new()));
    let classification: SharedClassification = Arc::new(std::sync::RwLock::new(ClassificationData::default()));

    let classification_clone = classification.clone();
    tokio::spawn(async move {
        application::watch_classification(classification_clone).await;
    });

    api::start_rest_server(aggregator, classification, "0.0.0.0:8080").await?;

    Ok(())
}
