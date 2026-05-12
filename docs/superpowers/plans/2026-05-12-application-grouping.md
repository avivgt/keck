# Application-Level Power Grouping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Group pods by owning workload (Deployment, StatefulSet, etc.), categorize as platform/operator/application, and expose via API and UI.

**Architecture:** Agent enriches pod metadata with owner references and configurable labels during pod cache refresh. Controller aggregates by flexible `group_by` parameter. UI adds Applications page with category tabs.

**Tech Stack:** Rust (kube-rs for K8s API), Axum (REST API), React/PatternFly (UI)

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `keck-agent/src/k8s/workload.rs` | Owner chain resolution, label capture, category classification |
| Modify | `keck-agent/src/main.rs` | Use `PodIdentity` instead of `PodInfo`, include new fields in reports |
| Modify | `keck-agent/Cargo.toml` | Add `kube` + `k8s-openapi` dependencies |
| Modify | `keck-controller/src/aggregator/mod.rs` | Add `GroupBy`, `GroupPower`, `group_power()` method, update wire types |
| Modify | `keck-controller/src/api/mod.rs` | Add `/api/v1/applications` endpoint |
| Modify | `keck-ui/src/utils/api.ts` | Add `GroupPower` type and `getApplications()` API call |
| Create | `keck-ui/src/components/application/ApplicationsView.tsx` | Applications page with category tabs and group-by dropdown |
| Modify | `keck-ui/package.json` | Register new exposed module |
| Modify | `keck-ui/console-extensions.json` | Add Applications nav entry and route |
| Modify | `keck-operator/api/v1alpha1/types.go` | Add `CapturedLabels` field to `AgentSpec` |

---

### Task 1: Add kube-rs dependencies to keck-agent

**Files:**
- Modify: `keck-agent/Cargo.toml`

- [ ] **Step 1: Add kube and k8s-openapi dependencies**

In `keck-agent/Cargo.toml`, add to `[dependencies]`:

```toml
kube = { version = "0.98", features = ["client", "rustls-tls"], default-features = false }
k8s-openapi = { version = "0.23", features = ["v1_30"] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /Users/avivgt/keck/keck && cargo check -p keck-agent 2>&1 | tail -5`
Expected: compilation succeeds (warnings OK)

- [ ] **Step 3: Commit**

```bash
git add keck-agent/Cargo.toml
git commit -m "feat(agent): add kube-rs and k8s-openapi dependencies for workload resolution"
```

---

### Task 2: Implement workload resolution module

**Files:**
- Create: `keck-agent/src/k8s/workload.rs`

This module resolves pod owner chains and captures labels. It replaces the old `PodInfo` with `PodIdentity`.

- [ ] **Step 1: Write tests for workload resolution**

Create `keck-agent/src/k8s/workload.rs` with test module first:

```rust
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

/// Identity of a pod's owning workload, resolved from K8s owner references.
#[derive(Clone, Debug)]
pub struct PodIdentity {
    pub name: String,
    pub namespace: String,
    pub workload_uid: String,
    pub workload_name: String,
    pub workload_kind: String,
    pub workload_category: WorkloadCategory,
    pub labels: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkloadCategory {
    Platform,
    Operator,
    Application,
}

impl WorkloadCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Operator => "operator",
            Self::Application => "application",
        }
    }
}

/// Default labels to capture from pods.
pub const DEFAULT_CAPTURED_LABELS: &[&str] = &[
    "app.kubernetes.io/name",
    "app.kubernetes.io/part-of",
    "app.kubernetes.io/component",
    "argocd.argoproj.io/instance",
    "olm.owner",
];

/// Default label prefixes to capture (any label starting with these).
pub const DEFAULT_CAPTURED_PREFIXES: &[&str] = &[
    "operators.coreos.com/",
];

/// Parse the KECK_CAPTURED_LABELS env var into exact keys and prefix patterns.
/// Entries ending in `/*` are prefix matches. All others are exact.
pub fn parse_label_config(raw: &str) -> (Vec<String>, Vec<String>) {
    let mut exact = Vec::new();
    let mut prefixes = Vec::new();
    for entry in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if let Some(prefix) = entry.strip_suffix("/*") {
            prefixes.push(format!("{}/", prefix));
        } else {
            exact.push(entry.to_string());
        }
    }
    (exact, prefixes)
}

/// Extract matching labels from a pod's label map.
pub fn capture_labels(
    pod_labels: &std::collections::BTreeMap<String, String>,
    exact_keys: &[String],
    prefix_keys: &[String],
) -> HashMap<String, String> {
    let mut captured = HashMap::new();
    for key in exact_keys {
        if let Some(val) = pod_labels.get(key.as_str()) {
            captured.insert(key.clone(), val.clone());
        }
    }
    for prefix in prefix_keys {
        for (key, val) in pod_labels {
            if key.starts_with(prefix.as_str()) {
                captured.insert(key.clone(), val.clone());
            }
        }
    }
    captured
}

/// Determine workload category from namespace and labels.
pub fn classify_category(
    namespace: &str,
    labels: &std::collections::BTreeMap<String, String>,
) -> WorkloadCategory {
    if namespace.starts_with("openshift-") || namespace == "kube-system" {
        return WorkloadCategory::Platform;
    }
    if labels.keys().any(|k| k.starts_with("operators.coreos.com/")) {
        return WorkloadCategory::Operator;
    }
    if labels.get("olm.owner").is_some() {
        return WorkloadCategory::Operator;
    }
    WorkloadCategory::Application
}

/// Cached mapping from intermediate owner UID to top-level owner.
/// ReplicaSet UID -> (Deployment UID, Deployment name, "Deployment")
/// Job UID -> (CronJob UID, CronJob name, "CronJob")
#[derive(Clone, Debug)]
pub struct OwnerMapping {
    pub uid: String,
    pub name: String,
    pub kind: String,
}

/// Select the controlling owner reference from a list.
/// Prefers the one with controller=true, falls back to first.
pub fn select_controller_owner(
    owner_refs: &[k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference],
) -> Option<&k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference> {
    owner_refs
        .iter()
        .find(|o| o.controller == Some(true))
        .or_else(|| owner_refs.first())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_parse_label_config_exact_and_prefix() {
        let (exact, prefixes) = parse_label_config(
            "app.kubernetes.io/name, operators.coreos.com/*, custom/team"
        );
        assert_eq!(exact, vec!["app.kubernetes.io/name", "custom/team"]);
        assert_eq!(prefixes, vec!["operators.coreos.com/"]);
    }

    #[test]
    fn test_parse_label_config_empty() {
        let (exact, prefixes) = parse_label_config("");
        assert!(exact.is_empty());
        assert!(prefixes.is_empty());
    }

    #[test]
    fn test_capture_labels_exact() {
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("app.kubernetes.io/name".into(), "my-api".into());
        pod_labels.insert("unrelated".into(), "ignored".into());

        let exact = vec!["app.kubernetes.io/name".to_string()];
        let captured = capture_labels(&pod_labels, &exact, &[]);
        assert_eq!(captured.len(), 1);
        assert_eq!(captured["app.kubernetes.io/name"], "my-api");
    }

    #[test]
    fn test_capture_labels_prefix() {
        let mut pod_labels = BTreeMap::new();
        pod_labels.insert("operators.coreos.com/elastic".into(), "".into());
        pod_labels.insert("operators.coreos.com/prometheus".into(), "".into());
        pod_labels.insert("other/label".into(), "val".into());

        let prefixes = vec!["operators.coreos.com/".to_string()];
        let captured = capture_labels(&pod_labels, &[], &prefixes);
        assert_eq!(captured.len(), 2);
        assert!(captured.contains_key("operators.coreos.com/elastic"));
        assert!(captured.contains_key("operators.coreos.com/prometheus"));
    }

    #[test]
    fn test_capture_labels_no_matches() {
        let pod_labels = BTreeMap::new();
        let exact = vec!["app.kubernetes.io/name".to_string()];
        let captured = capture_labels(&pod_labels, &exact, &[]);
        assert!(captured.is_empty());
    }

    #[test]
    fn test_classify_platform_openshift() {
        let labels = BTreeMap::new();
        assert_eq!(
            classify_category("openshift-monitoring", &labels),
            WorkloadCategory::Platform
        );
    }

    #[test]
    fn test_classify_platform_kube_system() {
        let labels = BTreeMap::new();
        assert_eq!(
            classify_category("kube-system", &labels),
            WorkloadCategory::Platform
        );
    }

    #[test]
    fn test_classify_operator_by_label() {
        let mut labels = BTreeMap::new();
        labels.insert("operators.coreos.com/elasticsearch-operator".into(), "".into());
        assert_eq!(
            classify_category("default", &labels),
            WorkloadCategory::Operator
        );
    }

    #[test]
    fn test_classify_operator_by_olm_owner() {
        let mut labels = BTreeMap::new();
        labels.insert("olm.owner".into(), "my-operator.v1.0".into());
        assert_eq!(
            classify_category("my-operator-ns", &labels),
            WorkloadCategory::Operator
        );
    }

    #[test]
    fn test_classify_application() {
        let labels = BTreeMap::new();
        assert_eq!(
            classify_category("my-app-ns", &labels),
            WorkloadCategory::Application
        );
    }

    #[test]
    fn test_workload_category_as_str() {
        assert_eq!(WorkloadCategory::Platform.as_str(), "platform");
        assert_eq!(WorkloadCategory::Operator.as_str(), "operator");
        assert_eq!(WorkloadCategory::Application.as_str(), "application");
    }
}
```

- [ ] **Step 2: Register the module in k8s/mod.rs**

Add `pub mod workload;` to `keck-agent/src/k8s/mod.rs` at the top (after the existing module doc comment).

- [ ] **Step 3: Run tests to verify they pass**

Run: `cd /Users/avivgt/keck/keck && cargo test -p keck-agent -- workload 2>&1 | tail -15`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add keck-agent/src/k8s/workload.rs keck-agent/src/k8s/mod.rs
git commit -m "feat(agent): add workload resolution module with owner chain, label capture, and category classification"
```

---

### Task 3: Wire workload resolution into agent main loop

**Files:**
- Modify: `keck-agent/src/main.rs`

Replace `PodInfo` with `PodIdentity`. Update `refresh_pod_cache` to use kube-rs, deserialize owner references and labels, resolve owner chains. Update `PodPowerReport` wire type with new fields.

- [ ] **Step 1: Update PodPowerReport and PodInfo structs**

In `keck-agent/src/main.rs`, replace the `PodInfo` struct (around line 114-117):

```rust
/// Pod info resolved from the K8s API.
struct PodInfo {
    name: String,
    namespace: String,
}
```

with:

```rust
use k8s::workload::{PodIdentity, WorkloadCategory, classify_category, capture_labels, parse_label_config, select_controller_owner, OwnerMapping};
```

And add `mod k8s;` to the top with the other module declarations if not present (it should already be there but check it's not dead code).

Then add `PodIdentity` usage: replace every `HashMap<String, PodInfo>` with `HashMap<String, PodIdentity>` throughout main.rs.

Add the new fields to `PodPowerReport`:

```rust
#[derive(Serialize)]
struct PodPowerReport {
    node_name: String,
    pod_uid: String,
    pod_name: String,
    namespace: String,
    cpu_uw: u64,
    memory_uw: u64,
    gpu_uw: u64,
    #[serde(default)]
    storage_uw: u64,
    #[serde(default)]
    io_uw: u64,
    total_uw: u64,
    timestamp: SystemTime,
    #[serde(default)]
    workload_uid: String,
    #[serde(default)]
    workload_name: String,
    #[serde(default)]
    workload_kind: String,
    #[serde(default)]
    workload_category: String,
    #[serde(default)]
    labels: HashMap<String, String>,
}
```

- [ ] **Step 2: Replace refresh_pod_cache with kube-rs**

Replace the `refresh_pod_cache` function and the `build_k8s_client`, `k8s_token`, `PodList`, `Pod`, `PodMetadata` types. The new version uses kube-rs:

```rust
use kube::{Api, Client, api::ListParams};
use k8s_openapi::api::apps::v1::ReplicaSet;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod as K8sPod;

/// Cache of intermediate owner UIDs to top-level owners.
/// ReplicaSet UID -> OwnerMapping (Deployment), Job UID -> OwnerMapping (CronJob)
type OwnerCache = HashMap<String, OwnerMapping>;

async fn refresh_pod_cache(
    client: &Client,
    node_name: &str,
    cache: &mut HashMap<String, PodIdentity>,
    owner_cache: &mut OwnerCache,
    exact_labels: &[String],
    prefix_labels: &[String],
) {
    let pods: Api<K8sPod> = Api::all(client.clone());
    let lp = ListParams::default()
        .fields(&format!("spec.nodeName={}", node_name));

    let pod_list = match pods.list(&lp).await {
        Ok(pl) => pl,
        Err(e) => {
            warn!("Failed to list pods: {}", e);
            return;
        }
    };

    cache.clear();
    for pod in pod_list {
        let meta = &pod.metadata;
        let uid = match &meta.uid {
            Some(u) => u.clone(),
            None => continue,
        };
        let name = meta.name.clone().unwrap_or_default();
        let namespace = meta.namespace.clone().unwrap_or_default();
        let pod_labels = meta.labels.clone().unwrap_or_default();
        let owner_refs = meta.owner_references.clone().unwrap_or_default();

        let captured = capture_labels(&pod_labels, exact_labels, prefix_labels);
        let category = classify_category(&namespace, &pod_labels);

        let (wl_uid, wl_name, wl_kind) = resolve_owner(
            client, &uid, &name, &owner_refs, owner_cache
        ).await;

        cache.insert(uid, PodIdentity {
            name,
            namespace,
            workload_uid: wl_uid,
            workload_name: wl_name,
            workload_kind: wl_kind,
            workload_category: category,
            labels: captured,
        });
    }

    log::debug!("Refreshed pod cache: {} pods on node {}", cache.len(), node_name);
}

async fn resolve_owner(
    client: &Client,
    pod_uid: &str,
    pod_name: &str,
    owner_refs: &[k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference],
    owner_cache: &mut OwnerCache,
) -> (String, String, String) {
    let owner = match select_controller_owner(owner_refs) {
        Some(o) => o,
        None => return (pod_uid.to_string(), pod_name.to_string(), "Pod".to_string()),
    };

    let owner_uid = &owner.uid;
    let owner_kind = &owner.kind;
    let owner_name = &owner.name;

    match owner_kind.as_str() {
        "ReplicaSet" | "Job" => {
            if let Some(cached) = owner_cache.get(owner_uid) {
                return (cached.uid.clone(), cached.name.clone(), cached.kind.clone());
            }

            let resolved = resolve_intermediate_owner(client, owner_uid, owner_name, owner_kind).await;
            let mapping = resolved.unwrap_or(OwnerMapping {
                uid: owner_uid.clone(),
                name: owner_name.clone(),
                kind: owner_kind.clone(),
            });
            let result = (mapping.uid.clone(), mapping.name.clone(), mapping.kind.clone());
            owner_cache.insert(owner_uid.clone(), mapping);
            result
        }
        "StatefulSet" | "DaemonSet" | "CronJob" => {
            (owner_uid.clone(), owner_name.clone(), owner_kind.clone())
        }
        _ => {
            (owner_uid.clone(), owner_name.clone(), owner_kind.clone())
        }
    }
}

async fn resolve_intermediate_owner(
    client: &Client,
    owner_uid: &str,
    owner_name: &str,
    owner_kind: &str,
) -> Option<OwnerMapping> {
    match owner_kind {
        "ReplicaSet" => {
            let rs_api: Api<ReplicaSet> = Api::all(client.clone());
            let rs_list = rs_api.list(&ListParams::default()
                .fields(&format!("metadata.uid={}", owner_uid))).await.ok()?;
            let rs = rs_list.items.into_iter().next()?;
            let rs_owners = rs.metadata.owner_references.unwrap_or_default();
            let deployment = select_controller_owner(&rs_owners)?;
            Some(OwnerMapping {
                uid: deployment.uid.clone(),
                name: deployment.name.clone(),
                kind: deployment.kind.clone(),
            })
        }
        "Job" => {
            let job_api: Api<Job> = Api::all(client.clone());
            let job_list = job_api.list(&ListParams::default()
                .fields(&format!("metadata.uid={}", owner_uid))).await.ok()?;
            let job = job_list.items.into_iter().next()?;
            let job_owners = job.metadata.owner_references.unwrap_or_default();
            let cronjob = select_controller_owner(&job_owners)?;
            Some(OwnerMapping {
                uid: cronjob.uid.clone(),
                name: cronjob.name.clone(),
                kind: cronjob.kind.clone(),
            })
        }
        _ => None,
    }
}
```

- [ ] **Step 3: Update main() initialization**

In `main()`, replace the old `k8s_client` and pod_cache initialization with:

```rust
let k8s_client = Client::try_default().await
    .expect("Failed to create K8s client (not in-cluster?)");

let (exact_labels, prefix_labels) = {
    let raw = std::env::var("KECK_CAPTURED_LABELS").unwrap_or_default();
    if raw.is_empty() {
        let exact: Vec<String> = k8s::workload::DEFAULT_CAPTURED_LABELS.iter().map(|s| s.to_string()).collect();
        let prefixes: Vec<String> = k8s::workload::DEFAULT_CAPTURED_PREFIXES.iter().map(|s| s.to_string()).collect();
        (exact, prefixes)
    } else {
        parse_label_config(&raw)
    }
};

let mut owner_cache: OwnerCache = HashMap::new();
let mut pod_cache: HashMap<String, PodIdentity> = HashMap::new();
```

Update the `refresh_pod_cache` call sites (initial + in loop) to pass the new arguments:

```rust
refresh_pod_cache(&k8s_client, &node_name, &mut pod_cache, &mut owner_cache, &exact_labels, &prefix_labels).await;
```

- [ ] **Step 4: Update all PodPowerReport construction sites**

Every place that builds a `PodPowerReport` needs the new fields. Search for `PodPowerReport {` in main.rs. There are ~5 locations (enumerate_pods, enumerate_pods_ebpf, enumerate_pods_ebpf_weighted, add_gpu_power_to_pods). In each, resolve the identity from `pod_cache`:

```rust
let identity = pod_cache.get(pod_uid);
// ...
PodPowerReport {
    // ... existing fields ...
    workload_uid: identity.map(|i| i.workload_uid.clone()).unwrap_or_default(),
    workload_name: identity.map(|i| i.workload_name.clone()).unwrap_or_default(),
    workload_kind: identity.map(|i| i.workload_kind.clone()).unwrap_or_default(),
    workload_category: identity.map(|i| i.workload_category.as_str().to_string()).unwrap_or("application".into()),
    labels: identity.map(|i| i.labels.clone()).unwrap_or_default(),
}
```

- [ ] **Step 5: Remove old K8s types and build_k8s_client**

Delete: `PodList`, `Pod`, `PodMetadata`, `PodInfo`, `build_k8s_client()`, `k8s_token()`, `count_unique_namespaces()` (unused). Remove `refresh_pod_cache` old implementation.

- [ ] **Step 6: Verify it compiles**

Run: `cd /Users/avivgt/keck/keck && cargo check -p keck-agent 2>&1 | tail -10`
Expected: compiles (eBPF cross-compile may warn, that's fine)

- [ ] **Step 7: Commit**

```bash
git add keck-agent/src/main.rs keck-agent/src/k8s/
git commit -m "feat(agent): wire workload resolution into pod cache and reports

Replaces raw reqwest K8s client with kube-rs. Pod reports now include
workload_uid, workload_name, workload_kind, workload_category, and
captured labels."
```

---

### Task 4: Update controller wire types and aggregator

**Files:**
- Modify: `keck-controller/src/aggregator/mod.rs`

- [ ] **Step 1: Update PodPowerReport with new fields**

In `keck-controller/src/aggregator/mod.rs`, add the new fields to `PodPowerReport` (the controller-side copy):

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PodPowerReport {
    pub node_name: String,
    pub pod_uid: String,
    pub pod_name: String,
    pub namespace: String,
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    #[serde(default)]
    pub storage_uw: u64,
    #[serde(default)]
    pub io_uw: u64,
    pub total_uw: u64,
    pub timestamp: SystemTime,
    #[serde(default)]
    pub workload_uid: String,
    #[serde(default)]
    pub workload_name: String,
    #[serde(default)]
    pub workload_kind: String,
    #[serde(default)]
    pub workload_category: String,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}
```

- [ ] **Step 2: Add GroupBy, GroupPower, and PodPowerSummary types**

Add after the existing `NodeSummary` struct:

```rust
#[derive(Clone, Debug)]
pub enum GroupBy {
    Workload,
    Label(String),
    Namespace,
}

impl GroupBy {
    pub fn parse(raw: &str) -> Self {
        if raw == "namespace" {
            GroupBy::Namespace
        } else if let Some(label_key) = raw.strip_prefix("label:") {
            GroupBy::Label(label_key.to_string())
        } else {
            GroupBy::Workload
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct GroupPower {
    pub group_key: String,
    pub group_name: String,
    pub group_kind: String,
    pub namespace: String,
    pub category: String,
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    pub storage_uw: u64,
    pub io_uw: u64,
    pub total_uw: u64,
    pub pod_count: usize,
    pub pods: Vec<PodPowerSummary>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PodPowerSummary {
    pub pod_uid: String,
    pub pod_name: String,
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    pub total_uw: u64,
}
```

- [ ] **Step 3: Write failing test for group_power**

Add to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_group_power_by_workload() {
    let mut agg = ClusterAggregator::new();
    let mut pod_a = make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0);
    pod_a.workload_uid = "deploy-uid-1".into();
    pod_a.workload_name = "web".into();
    pod_a.workload_kind = "Deployment".into();
    pod_a.workload_category = "application".into();

    let mut pod_b = make_pod("node-1", "uid-b", "web-2", "default", 2000, 800, 0);
    pod_b.workload_uid = "deploy-uid-1".into();
    pod_b.workload_name = "web".into();
    pod_b.workload_kind = "Deployment".into();
    pod_b.workload_category = "application".into();

    let mut pod_c = make_pod("node-1", "uid-c", "api-1", "default", 3000, 1000, 0);
    pod_c.workload_uid = "deploy-uid-2".into();
    pod_c.workload_name = "api".into();
    pod_c.workload_kind = "Deployment".into();
    pod_c.workload_category = "application".into();

    agg.ingest(make_report(
        make_node("node-1", 10000, 5000, 0, None, 1000),
        vec![pod_a, pod_b, pod_c],
    ));

    let groups = agg.group_power(GroupBy::Workload, None);
    assert_eq!(groups.len(), 2);
    let web = groups.iter().find(|g| g.group_name == "web").unwrap();
    assert_eq!(web.cpu_uw, 3000);
    assert_eq!(web.pod_count, 2);
    let api = groups.iter().find(|g| g.group_name == "api").unwrap();
    assert_eq!(api.cpu_uw, 3000);
    assert_eq!(api.pod_count, 1);
}

#[test]
fn test_group_power_by_label() {
    let mut agg = ClusterAggregator::new();
    let mut pod_a = make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0);
    pod_a.labels.insert("app.kubernetes.io/name".into(), "frontend".into());
    pod_a.workload_category = "application".into();

    let mut pod_b = make_pod("node-1", "uid-b", "api-1", "default", 2000, 800, 0);
    pod_b.labels.insert("app.kubernetes.io/name".into(), "frontend".into());
    pod_b.workload_category = "application".into();

    let mut pod_c = make_pod("node-1", "uid-c", "db-1", "default", 3000, 1000, 0);
    pod_c.labels.insert("app.kubernetes.io/name".into(), "backend".into());
    pod_c.workload_category = "application".into();

    agg.ingest(make_report(
        make_node("node-1", 10000, 5000, 0, None, 1000),
        vec![pod_a, pod_b, pod_c],
    ));

    let groups = agg.group_power(GroupBy::Label("app.kubernetes.io/name".into()), None);
    assert_eq!(groups.len(), 2);
    let frontend = groups.iter().find(|g| g.group_name == "frontend").unwrap();
    assert_eq!(frontend.cpu_uw, 3000);
    assert_eq!(frontend.pod_count, 2);
}

#[test]
fn test_group_power_category_filter() {
    let mut agg = ClusterAggregator::new();
    let mut pod_a = make_pod("node-1", "uid-a", "web-1", "default", 1000, 500, 0);
    pod_a.workload_uid = "d1".into();
    pod_a.workload_name = "web".into();
    pod_a.workload_kind = "Deployment".into();
    pod_a.workload_category = "application".into();

    let mut pod_b = make_pod("node-1", "uid-b", "mon-1", "openshift-monitoring", 2000, 800, 0);
    pod_b.workload_uid = "d2".into();
    pod_b.workload_name = "prometheus".into();
    pod_b.workload_kind = "StatefulSet".into();
    pod_b.workload_category = "platform".into();

    agg.ingest(make_report(
        make_node("node-1", 10000, 5000, 0, None, 1000),
        vec![pod_a, pod_b],
    ));

    let apps = agg.group_power(GroupBy::Workload, Some("application"));
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].group_name, "web");

    let platform = agg.group_power(GroupBy::Workload, Some("platform"));
    assert_eq!(platform.len(), 1);
    assert_eq!(platform[0].group_name, "prometheus");
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cd /Users/avivgt/keck/keck && cargo test -p keck-controller -- group_power 2>&1 | tail -10`
Expected: FAIL (group_power method doesn't exist yet)

- [ ] **Step 5: Implement group_power method**

Add to `impl ClusterAggregator`:

```rust
pub fn group_power(&self, group_by: GroupBy, category: Option<&str>) -> Vec<GroupPower> {
    let mut groups: HashMap<String, GroupPower> = HashMap::new();

    for state in self.pods.values() {
        let report = &state.report;

        if let Some(cat) = category {
            if report.workload_category != cat {
                continue;
            }
        }

        let (key, name, kind) = match &group_by {
            GroupBy::Workload => {
                if report.workload_uid.is_empty() {
                    (report.pod_uid.clone(), report.pod_name.clone(), "Pod".to_string())
                } else {
                    (report.workload_uid.clone(), report.workload_name.clone(), report.workload_kind.clone())
                }
            }
            GroupBy::Label(label_key) => {
                match report.labels.get(label_key) {
                    Some(val) => (val.clone(), val.clone(), label_key.clone()),
                    None => continue,
                }
            }
            GroupBy::Namespace => {
                (report.namespace.clone(), report.namespace.clone(), "Namespace".to_string())
            }
        };

        let entry = groups.entry(key.clone()).or_insert_with(|| GroupPower {
            group_key: key,
            group_name: name.clone(),
            group_kind: kind.clone(),
            namespace: report.namespace.clone(),
            category: report.workload_category.clone(),
            cpu_uw: 0,
            memory_uw: 0,
            gpu_uw: 0,
            storage_uw: 0,
            io_uw: 0,
            total_uw: 0,
            pod_count: 0,
            pods: Vec::new(),
        });

        entry.cpu_uw += report.cpu_uw;
        entry.memory_uw += report.memory_uw;
        entry.gpu_uw += report.gpu_uw;
        entry.storage_uw += report.storage_uw;
        entry.io_uw += report.io_uw;
        entry.total_uw += report.total_uw;
        entry.pod_count += 1;
        entry.pods.push(PodPowerSummary {
            pod_uid: report.pod_uid.clone(),
            pod_name: report.pod_name.clone(),
            cpu_uw: report.cpu_uw,
            memory_uw: report.memory_uw,
            gpu_uw: report.gpu_uw,
            total_uw: report.total_uw,
        });

        if entry.namespace != report.namespace {
            entry.namespace = String::new();
        }
    }

    let mut result: Vec<GroupPower> = groups.into_values().collect();
    result.sort_by(|a, b| b.total_uw.cmp(&a.total_uw));
    result
}
```

- [ ] **Step 6: Update make_pod test helper**

The `make_pod` helper needs new fields. Update it:

```rust
fn make_pod(node: &str, uid: &str, name: &str, ns: &str, cpu: u64, mem: u64, gpu: u64) -> PodPowerReport {
    PodPowerReport {
        node_name: node.into(),
        pod_uid: uid.into(),
        pod_name: name.into(),
        namespace: ns.into(),
        cpu_uw: cpu,
        memory_uw: mem,
        gpu_uw: gpu,
        total_uw: cpu + mem + gpu,
        timestamp: SystemTime::now(),
        workload_uid: String::new(),
        workload_name: String::new(),
        workload_kind: String::new(),
        workload_category: "application".into(),
        labels: HashMap::new(),
    }
}
```

Also update `make_agent_report` in `api/mod.rs` tests with the same new fields.

- [ ] **Step 7: Run all tests**

Run: `cd /Users/avivgt/keck/keck && cargo test -p keck-controller 2>&1 | tail -20`
Expected: ALL tests pass (old + new)

- [ ] **Step 8: Commit**

```bash
git add keck-controller/src/aggregator/mod.rs
git commit -m "feat(controller): add group_power() with workload/label/namespace grouping and category filtering"
```

---

### Task 5: Add /api/v1/applications endpoint

**Files:**
- Modify: `keck-controller/src/api/mod.rs`

- [ ] **Step 1: Write failing test for the endpoint**

Add to the test module in `api/mod.rs`:

```rust
#[tokio::test]
async fn test_get_applications_by_workload() {
    let state = make_state();
    {
        let mut agg = state.aggregator.write().await;
        let mut report = make_agent_report();
        report.pods[0].workload_uid = "deploy-1".into();
        report.pods[0].workload_name = "web".into();
        report.pods[0].workload_kind = "Deployment".into();
        report.pods[0].workload_category = "application".into();
        agg.ingest(report);
    }
    let app = build_app(state);

    let req = Request::builder()
        .uri("/api/v1/applications?group_by=workload")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["group_name"], "web");
    assert_eq!(arr[0]["group_kind"], "Deployment");
    assert_eq!(arr[0]["category"], "application");
}

#[tokio::test]
async fn test_get_applications_with_category_filter() {
    let state = make_state();
    {
        let mut agg = state.aggregator.write().await;
        let mut report = make_agent_report();
        report.pods[0].workload_category = "application".into();
        agg.ingest(report);

        let mut report2 = make_agent_report();
        report2.node.node_name = "node-2".into();
        report2.pods[0].pod_uid = "uid-platform".into();
        report2.pods[0].namespace = "openshift-monitoring".into();
        report2.pods[0].workload_category = "platform".into();
        report2.pods[0].node_name = "node-2".into();
        agg.ingest(report2);
    }
    let app = build_app(state);

    let req = Request::builder()
        .uri("/api/v1/applications?category=application")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["category"], "application");
}
```

- [ ] **Step 2: Add the handler and route**

Add the route to the Router in `start_rest_server`:

```rust
.route("/api/v1/applications", get(handle_applications))
```

Add the handler:

```rust
async fn handle_applications(
    State(state): State<ServerState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let agg = state.aggregator.read().await;
    let group_by = params.get("group_by")
        .map(|s| crate::aggregator::GroupBy::parse(s))
        .unwrap_or(crate::aggregator::GroupBy::Workload);
    let category = params.get("category").map(|s| s.as_str());

    let groups = agg.group_power(group_by, category);

    let list: Vec<serde_json::Value> = groups.iter().map(|g| {
        serde_json::json!({
            "group_key": g.group_key,
            "group_name": g.group_name,
            "group_kind": g.group_kind,
            "namespace": g.namespace,
            "category": g.category,
            "cpu_watts": g.cpu_uw as f64 / 1e6,
            "memory_watts": g.memory_uw as f64 / 1e6,
            "gpu_watts": g.gpu_uw as f64 / 1e6,
            "storage_watts": g.storage_uw as f64 / 1e6,
            "io_watts": g.io_uw as f64 / 1e6,
            "total_watts": g.total_uw as f64 / 1e6,
            "pod_count": g.pod_count,
            "pods": g.pods.iter().map(|p| serde_json::json!({
                "pod_uid": p.pod_uid,
                "pod_name": p.pod_name,
                "cpu_watts": p.cpu_uw as f64 / 1e6,
                "memory_watts": p.memory_uw as f64 / 1e6,
                "gpu_watts": p.gpu_uw as f64 / 1e6,
                "total_watts": p.total_uw as f64 / 1e6,
            })).collect::<Vec<_>>(),
        })
    }).collect();

    Json(serde_json::Value::Array(list))
}
```

Also add the route to `build_app` in tests:

```rust
.route("/api/v1/applications", get(handle_applications))
```

- [ ] **Step 3: Run tests**

Run: `cd /Users/avivgt/keck/keck && cargo test -p keck-controller 2>&1 | tail -20`
Expected: ALL pass

- [ ] **Step 4: Commit**

```bash
git add keck-controller/src/api/mod.rs
git commit -m "feat(controller): add /api/v1/applications endpoint with group_by and category filtering"
```

---

### Task 6: Add UI API client and types

**Files:**
- Modify: `keck-ui/src/utils/api.ts`

- [ ] **Step 1: Add GroupPower type and getApplications function**

Add to `api.ts`:

```typescript
export interface PodPowerSummary {
  pod_uid: string;
  pod_name: string;
  cpu_watts: number;
  memory_watts: number;
  gpu_watts: number;
  total_watts: number;
}

export interface GroupPower {
  group_key: string;
  group_name: string;
  group_kind: string;
  namespace: string;
  category: string;
  cpu_watts: number;
  memory_watts: number;
  gpu_watts: number;
  storage_watts: number;
  io_watts: number;
  total_watts: number;
  pod_count: number;
  pods: PodPowerSummary[];
}
```

Add to the `api` object:

```typescript
getApplications: (groupBy: string = "workload", category?: string) => {
  let url = `/api/v1/applications?group_by=${encodeURIComponent(groupBy)}`;
  if (category) url += `&category=${encodeURIComponent(category)}`;
  return get<GroupPower[]>(url);
},
```

- [ ] **Step 2: Commit**

```bash
git add keck-ui/src/utils/api.ts
git commit -m "feat(ui): add GroupPower type and getApplications API client"
```

---

### Task 7: Build the Applications page

**Files:**
- Create: `keck-ui/src/components/application/ApplicationsView.tsx`

- [ ] **Step 1: Create the ApplicationsView component**

```tsx
// SPDX-License-Identifier: Apache-2.0

import * as React from "react";
import {
  Page,
  PageSection,
  Title,
  Spinner,
  EmptyState,
  EmptyStateBody,
  Label,
  Tabs,
  Tab,
  TabTitleText,
  Select,
  SelectOption,
  MenuToggle,
  MenuToggleElement,
} from "@patternfly/react-core";
import {
  Table,
  Thead,
  Tbody,
  Tr,
  Th,
  Td,
  ThProps,
} from "@patternfly/react-table";
import { api, GroupPower } from "../../utils/api";
import { formatWatts } from "../../utils/format";

type SortKey = "group_name" | "total_watts" | "cpu_watts" | "memory_watts" | "gpu_watts" | "storage_watts" | "io_watts" | "pod_count";

const CATEGORIES = ["all", "application", "operator", "platform"] as const;
type Category = typeof CATEGORIES[number];

const ApplicationsView: React.FC = () => {
  const [groups, setGroups] = React.useState<GroupPower[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [category, setCategory] = React.useState<Category>("all");
  const [groupBy, setGroupBy] = React.useState("workload");
  const [sortBy, setSortBy] = React.useState<SortKey>("total_watts");
  const [sortDir, setSortDir] = React.useState<"asc" | "desc">("desc");
  const [groupByOpen, setGroupByOpen] = React.useState(false);

  React.useEffect(() => {
    const fetchData = () => {
      const cat = category === "all" ? undefined : category;
      api.getApplications(groupBy, cat)
        .then(setGroups)
        .finally(() => setLoading(false));
    };
    fetchData();
    const interval = setInterval(fetchData, 5000);
    return () => clearInterval(interval);
  }, [category, groupBy]);

  if (loading) {
    return <Page><PageSection><Spinner /></PageSection></Page>;
  }

  const totalWatts = groups.reduce((sum, g) => sum + g.total_watts, 0);
  const totalPods = groups.reduce((sum, g) => sum + g.pod_count, 0);

  const sorted = [...groups].sort((a, b) => {
    const av = (a as any)[sortBy];
    const bv = (b as any)[sortBy];
    if (typeof av === "string") return sortDir === "asc" ? av.localeCompare(bv) : bv.localeCompare(av);
    return sortDir === "asc" ? av - bv : bv - av;
  });

  const cols: SortKey[] = ["group_name", "total_watts", "cpu_watts", "memory_watts", "gpu_watts", "storage_watts", "io_watts", "pod_count"];
  const getSortParams = (key: SortKey): ThProps["sort"] => ({
    sortBy: { index: cols.indexOf(sortBy), direction: sortDir },
    onSort: (_e, _idx, dir) => { setSortBy(key); setSortDir(dir as "asc" | "desc"); },
    columnIndex: cols.indexOf(key),
  });

  const groupByOptions = [
    { value: "workload", label: "Workload" },
    { value: "namespace", label: "Namespace" },
    { value: "label:app.kubernetes.io/name", label: "App Label" },
    { value: "label:app.kubernetes.io/part-of", label: "Part Of" },
    { value: "label:argocd.argoproj.io/instance", label: "ArgoCD App" },
  ];

  const categoryColor = (cat: string) => {
    if (cat === "platform") return "purple";
    if (cat === "operator") return "blue";
    return "green";
  };

  return (
    <Page>
      <PageSection>
        <Title headingLevel="h1" size="xl">Applications</Title>
        <p style={{ marginTop: 4, color: "var(--pf-v6-global--Color--200)" }}>
          {groups.length} groups, {totalPods} pods, {formatWatts(totalWatts)} total.
        </p>
      </PageSection>

      <PageSection>
        <div style={{ display: "flex", alignItems: "center", gap: 16, marginBottom: 16 }}>
          <Tabs activeKey={category} onSelect={(_e, key) => setCategory(key as Category)}>
            <Tab eventKey="all" title={<TabTitleText>All</TabTitleText>} />
            <Tab eventKey="application" title={<TabTitleText>Applications</TabTitleText>} />
            <Tab eventKey="operator" title={<TabTitleText>Operators</TabTitleText>} />
            <Tab eventKey="platform" title={<TabTitleText>Platform</TabTitleText>} />
          </Tabs>
          <div style={{ marginLeft: "auto" }}>
            <Select
              isOpen={groupByOpen}
              onOpenChange={setGroupByOpen}
              onSelect={(_e, val) => { setGroupBy(val as string); setGroupByOpen(false); }}
              selected={groupBy}
              toggle={(toggleRef: React.Ref<MenuToggleElement>) => (
                <MenuToggle ref={toggleRef} onClick={() => setGroupByOpen(!groupByOpen)} isExpanded={groupByOpen}>
                  Group by: {groupByOptions.find(o => o.value === groupBy)?.label || groupBy}
                </MenuToggle>
              )}
            >
              {groupByOptions.map(opt => (
                <SelectOption key={opt.value} value={opt.value}>{opt.label}</SelectOption>
              ))}
            </Select>
          </div>
        </div>

        {sorted.length > 0 ? (
          <Table aria-label="Application power table" variant="compact">
            <Thead>
              <Tr>
                <Th sort={getSortParams("group_name")}>Name</Th>
                <Th>Kind</Th>
                <Th>Category</Th>
                <Th>Namespace</Th>
                <Th sort={getSortParams("total_watts")}>Total Power</Th>
                <Th sort={getSortParams("cpu_watts")}>CPU</Th>
                <Th sort={getSortParams("memory_watts")}>Memory</Th>
                <Th sort={getSortParams("gpu_watts")}>GPU</Th>
                <Th sort={getSortParams("storage_watts")}>Storage</Th>
                <Th sort={getSortParams("io_watts")}>Network</Th>
                <Th sort={getSortParams("pod_count")}>Pods</Th>
              </Tr>
            </Thead>
            <Tbody>
              {sorted.map((g) => (
                <Tr key={g.group_key}>
                  <Td style={{ fontWeight: 600 }}>{g.group_name}</Td>
                  <Td><Label style={{ fontSize: "11px" }}>{g.group_kind}</Label></Td>
                  <Td><Label color={categoryColor(g.category)} style={{ fontSize: "11px" }}>{g.category}</Label></Td>
                  <Td style={{ fontSize: "0.9em", color: "var(--pf-v6-global--Color--200)" }}>{g.namespace || "multiple"}</Td>
                  <Td style={{ fontWeight: 600 }}>{formatWatts(g.total_watts)}</Td>
                  <Td>{formatWatts(g.cpu_watts)}</Td>
                  <Td>{formatWatts(g.memory_watts)}</Td>
                  <Td>{formatWatts(g.gpu_watts)}</Td>
                  <Td>{formatWatts(g.storage_watts)}</Td>
                  <Td>{formatWatts(g.io_watts)}</Td>
                  <Td>{g.pod_count}</Td>
                </Tr>
              ))}
            </Tbody>
          </Table>
        ) : (
          <EmptyState>
            <EmptyStateBody>No application power data available{category !== "all" ? ` for category "${category}"` : ""}.</EmptyStateBody>
          </EmptyState>
        )}
      </PageSection>
    </Page>
  );
};

export default ApplicationsView;
```

- [ ] **Step 2: Commit**

```bash
git add keck-ui/src/components/application/ApplicationsView.tsx
git commit -m "feat(ui): add Applications page with category tabs and group-by dropdown"
```

---

### Task 8: Register Applications page in console plugin

**Files:**
- Modify: `keck-ui/package.json`
- Modify: `keck-ui/console-extensions.json`

- [ ] **Step 1: Add exposed module to package.json**

In `package.json` under `consolePlugin.exposedModules`, add:

```json
"ApplicationsView": "./src/components/application/ApplicationsView"
```

- [ ] **Step 2: Add nav entry and route to console-extensions.json**

Add a nav entry after the "Nodes" entry (insert before the "power-namespaces" entry so Applications appears between Nodes and Namespaces):

```json
{
  "type": "console.navigation/href",
  "properties": {
    "id": "power-applications",
    "name": "Applications",
    "href": "/power-management/applications",
    "section": "power-management",
    "perspective": "admin"
  }
}
```

Add a route entry:

```json
{
  "type": "console.page/route",
  "properties": {
    "exact": true,
    "path": "/power-management/applications",
    "component": {
      "$codeRef": "ApplicationsView"
    }
  }
}
```

- [ ] **Step 3: Commit**

```bash
git add keck-ui/package.json keck-ui/console-extensions.json
git commit -m "feat(ui): register Applications page in OpenShift console plugin nav"
```

---

### Task 9: Add CapturedLabels to KeckCluster CRD

**Files:**
- Modify: `keck-operator/api/v1alpha1/types.go`

- [ ] **Step 1: Add CapturedLabels field to AgentSpec**

In `types.go`, add to the `AgentSpec` struct:

```go
// Labels to capture from pod metadata for application grouping.
// Entries ending in /* are treated as prefix matches.
// Default: app.kubernetes.io/name, app.kubernetes.io/part-of,
//   app.kubernetes.io/component, argocd.argoproj.io/instance,
//   operators.coreos.com/*, olm.owner
// +optional
CapturedLabels []string `json:"capturedLabels,omitempty"`
```

- [ ] **Step 2: Commit**

```bash
git add keck-operator/api/v1alpha1/types.go
git commit -m "feat(operator): add CapturedLabels field to KeckCluster CRD for configurable label capture"
```

---

### Task 10: Update cluster overview with category power split

**Files:**
- Modify: `keck-controller/src/api/mod.rs`
- Modify: `keck-ui/src/components/PowerManagementPage.tsx`

- [ ] **Step 1: Add category_power to cluster API response**

In `handle_cluster` in `api/mod.rs`, add a `category_power` section to the JSON response. After computing `power`, add:

```rust
let category_power = {
    let apps = agg.group_power(crate::aggregator::GroupBy::Workload, Some("application"));
    let ops = agg.group_power(crate::aggregator::GroupBy::Workload, Some("operator"));
    let platform = agg.group_power(crate::aggregator::GroupBy::Workload, Some("platform"));
    serde_json::json!({
        "application_watts": apps.iter().map(|g| g.total_uw).sum::<u64>() as f64 / 1e6,
        "operator_watts": ops.iter().map(|g| g.total_uw).sum::<u64>() as f64 / 1e6,
        "platform_watts": platform.iter().map(|g| g.total_uw).sum::<u64>() as f64 / 1e6,
    })
};
```

Add `"category_power": category_power,` to the JSON response object.

- [ ] **Step 2: Add category split to PowerManagementPage**

In `PowerManagementPage.tsx`, after the "Infrastructure" row in the summary table, add:

```tsx
{(data as any).category_power && (
  <>
    <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
      <td style={{ padding: "10px 8px", color: "#3e8635" }}>Applications</td>
      <td style={{ padding: "10px 8px", textAlign: "right", fontWeight: 600 }}>{formatWatts((data as any).category_power.application_watts)}</td>
      <td style={{ padding: "10px 8px", fontSize: "0.85em", color: "var(--pf-v6-global--Color--200)" }}>user workloads</td>
    </tr>
    <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
      <td style={{ padding: "10px 8px", color: "#0066cc" }}>Operators</td>
      <td style={{ padding: "10px 8px", textAlign: "right", fontWeight: 600 }}>{formatWatts((data as any).category_power.operator_watts)}</td>
      <td style={{ padding: "10px 8px", fontSize: "0.85em", color: "var(--pf-v6-global--Color--200)" }}>OLM-managed</td>
    </tr>
    <tr style={{ borderBottom: "1px solid var(--pf-v6-global--BorderColor--100)" }}>
      <td style={{ padding: "10px 8px", color: "#6753ac" }}>Platform</td>
      <td style={{ padding: "10px 8px", textAlign: "right", fontWeight: 600 }}>{formatWatts((data as any).category_power.platform_watts)}</td>
      <td style={{ padding: "10px 8px", fontSize: "0.85em", color: "var(--pf-v6-global--Color--200)" }}>openshift-* / kube-system</td>
    </tr>
  </>
)}
```

- [ ] **Step 3: Run controller tests**

Run: `cd /Users/avivgt/keck/keck && cargo test -p keck-controller 2>&1 | tail -20`
Expected: ALL pass

- [ ] **Step 4: Commit**

```bash
git add keck-controller/src/api/mod.rs keck-ui/src/components/PowerManagementPage.tsx
git commit -m "feat: add category power split (application/operator/platform) to cluster overview"
```
