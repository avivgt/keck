// SPDX-License-Identifier: Apache-2.0

//! KeckApplication CRD type definition and background watcher.
//!
//! Watches keck.io/v1alpha1 KeckApplication custom resources and feeds
//! application definitions into the aggregator for application-level
//! power grouping.

use std::sync::Arc;

use kube::{Api, Client, CustomResource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::aggregator::{ApplicationDef, ClusterAggregator};

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(group = "keck.io", version = "v1alpha1", kind = "KeckApplication")]
#[kube(status = "KeckApplicationStatus")]
#[kube(crates(kube_core = "::kube::core", k8s_openapi = "::k8s_openapi", schemars = "::schemars"))]
pub struct KeckApplicationSpec {
    #[serde(default)]
    pub namespaces: Vec<String>,
    #[serde(default, rename = "workloadSelectors")]
    pub workload_selectors: Vec<WorkloadSelector>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct WorkloadSelector {
    #[serde(default, rename = "matchLabels")]
    pub match_labels: std::collections::HashMap<String, String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct KeckApplicationStatus {
    #[serde(default)]
    pub total_watts: f64,
    #[serde(default)]
    pub pod_count: i32,
    #[serde(default)]
    pub workload_count: i32,
}

/// Background loop: polls KeckApplication CRDs every 30s and updates the aggregator.
pub async fn watch_applications(aggregator: Arc<RwLock<ClusterAggregator>>) {
    let client = match Client::try_default().await {
        Ok(c) => c,
        Err(e) => {
            log::warn!(
                "KeckApplication watcher: no K8s client ({}), application grouping disabled",
                e
            );
            return;
        }
    };

    let api: Api<KeckApplication> = Api::all(client);

    loop {
        match api.list(&Default::default()).await {
            Ok(list) => {
                let defs: Vec<ApplicationDef> = list
                    .items
                    .iter()
                    .map(|app| ApplicationDef {
                        name: app.metadata.name.clone().unwrap_or_default(),
                        namespaces: app.spec.namespaces.clone(),
                        label_selectors: app
                            .spec
                            .workload_selectors
                            .iter()
                            .map(|s| s.match_labels.clone())
                            .collect(),
                    })
                    .collect();

                let mut agg = aggregator.write().await;
                agg.set_applications(defs);
            }
            Err(e) => {
                log::warn!("Failed to list KeckApplications: {}", e);
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
    }
}
