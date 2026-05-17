// SPDX-License-Identifier: Apache-2.0

//! Classification data: discovers ClusterOperators and OLM Subscriptions
//! to classify pods into three non-overlapping categories:
//! 1. Cluster Operator (namespace in ClusterOperator relatedObjects)
//! 2. Operator (namespace has OLM Subscription, not a CO namespace)
//! 3. Application (everything else)

use std::collections::{HashMap, HashSet};

use kube::Client;

use crate::aggregator::ApplicationDef;
use crate::SharedClassification;

/// All data needed to classify and group pods. Built by the background watcher,
/// read by API handlers. Lives in a separate std::sync::RwLock (not the aggregator).
#[derive(Clone, Debug, Default)]
pub struct ClassificationData {
    /// namespace -> ClusterOperator name. Pods in these namespaces are "platform".
    pub co_namespaces: HashMap<String, String>,
    /// Namespaces with OLM Subscriptions (minus CO namespaces). Pods here are "operator".
    pub operator_namespaces: HashSet<String>,
    /// Currently unused (KeckApplication CRD removed). Reserved for future use.
    pub app_defs: Vec<ApplicationDef>,
}

impl ClassificationData {
    /// Classify a pod by its namespace. Returns ("platform"|"operator"|"application", Option<co_name>).
    pub fn classify(&self, namespace: &str) -> (&'static str, Option<&String>) {
        if let Some(co_name) = self.co_namespaces.get(namespace) {
            return ("platform", Some(co_name));
        }
        if self.operator_namespaces.contains(namespace) {
            return ("operator", None);
        }
        ("application", None)
    }
}

/// Background loop: polls K8s APIs every 30s to build classification data.
pub async fn watch_classification(shared: SharedClassification) {
    let client = match Client::try_default().await {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Classification watcher: no K8s client ({}), disabled", e);
            return;
        }
    };

    loop {
        let mut data = ClassificationData::default();

        data.co_namespaces = discover_co_namespaces(&client).await;

        let sub_namespaces = discover_subscription_namespaces(&client).await;
        for ns in sub_namespaces {
            if !data.co_namespaces.contains_key(&ns) {
                data.operator_namespaces.insert(ns);
            }
        }

        log::info!(
            "Classification: {} CO namespaces, {} operator namespaces",
            data.co_namespaces.len(),
            data.operator_namespaces.len(),
        );

        if let Ok(mut guard) = shared.write() {
            *guard = data;
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
    }
}

/// Map each namespace to its ClusterOperator via relatedObjects.
async fn discover_co_namespaces(client: &Client) -> HashMap<String, String> {
    use kube::api::{Api, DynamicObject};
    use kube::core::{ApiResource, GroupVersionKind};

    let gvk = GroupVersionKind::gvk("config.openshift.io", "v1", "ClusterOperator");
    let ar = ApiResource::from_gvk(&gvk);
    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);

    let list = match api.list(&Default::default()).await {
        Ok(l) => l,
        Err(e) => {
            log::debug!("ClusterOperator discovery unavailable ({})", e);
            return HashMap::new();
        }
    };

    let mut ns_to_co: HashMap<String, String> = HashMap::new();

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
            if ns == "openshift-config" || ns == "openshift-config-managed" {
                continue;
            }
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

    ns_to_co
}

/// List all namespaces that have OLM Subscriptions.
async fn discover_subscription_namespaces(client: &Client) -> HashSet<String> {
    use kube::api::{Api, DynamicObject};
    use kube::core::{ApiResource, GroupVersionKind};

    let gvk = GroupVersionKind::gvk("operators.coreos.com", "v1alpha1", "Subscription");
    let ar = ApiResource::from_gvk(&gvk);
    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);

    match api.list(&Default::default()).await {
        Ok(list) => {
            list.items
                .iter()
                .filter_map(|sub| sub.metadata.namespace.clone())
                .collect()
        }
        Err(e) => {
            log::debug!("OLM Subscription discovery unavailable ({})", e);
            HashSet::new()
        }
    }
}
