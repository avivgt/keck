// SPDX-License-Identifier: Apache-2.0

//! Cluster controller — aggregates power data from all node agents.
//!
//! Agents POST their power reports to /api/v1/report.
//! UI and dashboards read from /api/v1/cluster, /namespaces, /nodes, /pods.

mod aggregator;
mod api;
mod application;
mod carbon;
mod kepler_scraper;
pub mod metrics;
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
    let metrics = Arc::new(metrics::Metrics::new());

    let classification_clone = classification.clone();
    tokio::spawn(async move {
        application::watch_classification(classification_clone).await;
    });

    let kepler_agg = aggregator.clone();
    tokio::spawn(async move {
        kepler_scraper::run_kepler_scraper(kepler_agg).await;
    });

    let node_watcher_agg = aggregator.clone();
    tokio::spawn(async move {
        watch_cluster_nodes(node_watcher_agg).await;
    });

    api::start_rest_server(aggregator, classification, metrics, "0.0.0.0:8080").await?;

    Ok(())
}

async fn watch_cluster_nodes(aggregator: Arc<RwLock<ClusterAggregator>>) {
    use kube::{Api, Client, api::ListParams};
    use k8s_openapi::api::core::v1::Node;

    let client = match Client::try_default().await {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Node watcher: no K8s client ({}), disabled", e);
            return;
        }
    };

    let nodes_api: Api<Node> = Api::all(client);

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;

        match nodes_api.list(&ListParams::default()).await {
            Ok(node_list) => {
                let live: Vec<String> = node_list
                    .items
                    .iter()
                    .filter_map(|n| n.metadata.name.clone())
                    .collect();
                let mut agg = aggregator.write().await;
                agg.evict_removed_nodes(&live);
            }
            Err(e) => {
                log::debug!("Node watcher: failed to list nodes ({})", e);
            }
        }
    }
}
