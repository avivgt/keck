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

use crate::aggregator::ApplicationDef;
use crate::AppDefs;

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

/// Background loop: polls KeckApplication CRDs and ClusterOperators every 30s.
/// Merges both into the aggregator's application definitions.
pub async fn watch_applications(app_defs: AppDefs) {
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

    let keck_api: Api<KeckApplication> = Api::all(client.clone());

    loop {
        let mut defs: Vec<ApplicationDef> = Vec::new();

        // 1. Load KeckApplication CRDs (user-defined)
        match keck_api.list(&Default::default()).await {
            Ok(list) => {
                for app in &list.items {
                    defs.push(ApplicationDef {
                        name: app.metadata.name.clone().unwrap_or_default(),
                        namespaces: app.spec.namespaces.clone(),
                        label_selectors: app
                            .spec
                            .workload_selectors
                            .iter()
                            .map(|s| s.match_labels.clone())
                            .collect(),
                    });
                }
            }
            Err(e) => {
                log::warn!("Failed to list KeckApplications: {}", e);
            }
        }

        // 2. Load ClusterOperators (platform auto-detection)
        let co_defs = discover_cluster_operators(&client).await;
        defs.extend(co_defs);

        if let Ok(mut guard) = app_defs.write() {
            *guard = defs;
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
    }
}

/// Discover platform applications from ClusterOperator resources.
/// Each ClusterOperator becomes an ApplicationDef with its related namespaces.
async fn discover_cluster_operators(client: &Client) -> Vec<ApplicationDef> {
    use kube::api::{Api, DynamicObject};
    use kube::core::{ApiResource, GroupVersionKind};

    let gvk = GroupVersionKind::gvk("config.openshift.io", "v1", "ClusterOperator");
    let ar = ApiResource::from_gvk(&gvk);
    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);

    let list = match api.list(&Default::default()).await {
        Ok(l) => l,
        Err(e) => {
            log::debug!("ClusterOperator discovery unavailable ({}), skipping platform auto-detection", e);
            return Vec::new();
        }
    };

    // Build namespace -> CO assignments. Each namespace goes to the CO
    // whose name most closely matches (e.g., openshift-etcd -> etcd CO,
    // not kube-apiserver CO which also lists openshift-config).
    let mut ns_to_co: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for co in &list.items {
        let co_name = co.metadata.name.clone().unwrap_or_default();
        let related = co.data.get("status")
            .and_then(|s| s.get("relatedObjects"))
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();

        for obj in &related {
            let resource = obj.get("resource").and_then(|r| r.as_str()).unwrap_or("");
            if resource != "namespaces" {
                continue;
            }
            let ns = match obj.get("name").and_then(|n| n.as_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            // Skip shared config namespaces -- they belong to many COs
            if ns == "openshift-config" || ns == "openshift-config-managed" {
                continue;
            }
            // Assign namespace to this CO, but prefer a CO that already
            // has a more specific match (shorter distance from CO name to NS name).
            // Simple heuristic: if the namespace contains the CO name, it's a better match.
            let dominated = ns.contains(&co_name) || ns.contains(&co_name.replace('-', "_"));
            match ns_to_co.get(&ns) {
                None => { ns_to_co.insert(ns, co_name.clone()); }
                Some(existing) if dominated && !ns.contains(existing) => {
                    ns_to_co.insert(ns, co_name.clone());
                }
                _ => {}
            }
        }
    }

    // Invert: CO name -> list of namespaces
    let mut co_namespaces: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for (ns, co_name) in &ns_to_co {
        co_namespaces.entry(co_name.clone()).or_default().push(ns.clone());
    }

    let defs: Vec<ApplicationDef> = co_namespaces
        .into_iter()
        .map(|(name, namespaces)| ApplicationDef {
            name,
            namespaces,
            label_selectors: Vec::new(),
        })
        .collect();

    log::info!("Discovered {} ClusterOperator application groups", defs.len());
    defs
}
