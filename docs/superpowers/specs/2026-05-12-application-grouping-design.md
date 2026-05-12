# Application-Level Power Grouping

Date: 2026-05-12

## Problem

Keck currently groups pods only by namespace. Users see individual pod entries and have no way to answer "how much power does my API service consume?" without manually summing pods. On OpenShift clusters, platform operators and user workloads are mixed in the same flat view.

## Goals

- Group pods by their owning application (Deployment, StatefulSet, DaemonSet, Job, CronJob)
- Support label-based grouping (app.kubernetes.io/name, ArgoCD instance, custom labels)
- Distinguish platform operators from user applications on OpenShift
- Server-side aggregation with a flexible group_by API
- Minimal resource overhead on the cluster

## Non-Goals

- Historical time-series per application (no storage backend yet)
- Prometheus metrics export for application-level data
- ACM propagation of label configuration (future, with keck-fleet)

## Design

### 1. Data Model

#### New fields on PodPowerReport (agent -> controller wire type)

```rust
struct PodPowerReport {
    // existing fields unchanged
    node_name: String,
    pod_uid: String,
    pod_name: String,
    namespace: String,
    cpu_uw: u64,
    memory_uw: u64,
    gpu_uw: u64,
    storage_uw: u64,
    io_uw: u64,
    total_uw: u64,
    timestamp: SystemTime,

    // new fields
    workload_uid: String,
    workload_name: String,
    workload_kind: String,              // "Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob", "Pod"
    workload_category: String,          // "platform", "operator", "application"
    labels: HashMap<String, String>,    // configured label subset
}
```

#### Workload resolution

The agent walks ownerReferences to find the top-level controller:

- Pod -> ReplicaSet -> Deployment (2 hops)
- Pod -> StatefulSet (1 hop)
- Pod -> DaemonSet (1 hop)
- Pod -> Job -> CronJob (2 hops)
- Pod with no ownerReferences -> bare Pod

Identity uses UIDs, not names. The ownerReference on a Pod contains the owner's UID. The agent caches `rs_uid -> (owner_uid, owner_name, owner_kind)` for the ReplicaSet -> Deployment hop. This cache entry is created by a single GET request on first encounter and never refreshed (owner relationships are immutable in K8s).

Bare pods: `workload_uid = pod_uid`, `workload_name = pod_name`, `workload_kind = "Pod"`.

Deleted owner (RS garbage collected while pod still runs): fall back to `workload_kind = "Unknown"`, use the ownerReference name field from the pod (still present even if the object is gone).

Multiple ownerReferences: use the one with `controller: true`, or the first one.

#### Workload category

Three categories, determined by the agent:

- **platform**: namespace starts with `openshift-` or is `kube-system`
- **operator**: pod has `operators.coreos.com/` label prefix, or lives in an OLM-managed operator namespace
- **application**: everything else

#### Label configuration

Default captured labels (shipped with Keck):
- `app.kubernetes.io/name`
- `app.kubernetes.io/part-of`
- `app.kubernetes.io/component`
- `argocd.argoproj.io/instance`
- `operators.coreos.com/*` (prefix match -- any label starting with `operators.coreos.com/` is captured)
- `olm.owner`

Label entries ending in `/*` are treated as prefix matches. All others are exact key matches.

Configurable via `KeckCluster` CRD:

```yaml
spec:
  capturedLabels:
    - app.kubernetes.io/name
    - app.kubernetes.io/part-of
    - app.kubernetes.io/component
    - argocd.argoproj.io/instance
    - custom/team
    - custom/cost-center
```

The operator passes this to the agent as `KECK_CAPTURED_LABELS` env var. In future ACM multi-cluster, a Policy on the hub pushes the same KeckCluster spec to all spokes.

### 2. Agent Changes

#### Replace raw reqwest pod fetching with kube-rs

The current `refresh_pod_cache` uses raw reqwest to call the K8s API and deserializes a minimal PodList (name, namespace, uid only). This is replaced with a kube-rs API client that deserializes the full Pod spec including `ownerReferences` and `metadata.labels`.

The agent does NOT use kube-rs informers/watchers for workload resources. Instead:

1. On pod cache refresh (every 30s), fetch Pod objects with full metadata (ownerReferences, labels).
2. For each pod, read `ownerReferences[0]` to get the immediate owner UID and kind.
3. If the owner is a ReplicaSet (or Job), check the local cache for the RS/Job UID -> top-level owner mapping.
4. On cache miss, do a single GET by UID to fetch the RS/Job object, read its ownerReferences to find the Deployment/CronJob, cache the result.
5. The RS -> Deployment cache is append-only. Entries are evicted only when the corresponding pod disappears from the node.

Resource cost: zero additional watch connections, one GET per unique ReplicaSet/Job (once, cached forever), same pod list fetch just deserializing 2 more fields.

#### PodIdentity struct

The existing `PodInfo` struct (name, namespace) becomes `PodIdentity`:

```rust
struct PodIdentity {
    name: String,
    namespace: String,
    workload_uid: String,
    workload_name: String,
    workload_kind: String,
    workload_category: String,
    labels: HashMap<String, String>,
}
```

The main loop's pod resolution is unchanged -- it reads from the enriched cache.

### 3. Controller Aggregation

New method on `ClusterAggregator`:

```rust
fn group_power(&self, group_by: GroupBy, category: Option<&str>) -> Vec<GroupPower>
```

GroupBy variants:
- `Workload` -- group by `workload_uid` (default)
- `Label(String)` -- group by a label value (e.g. `Label("argocd.argoproj.io/instance")`)
- `Namespace` -- existing behavior

GroupPower result:

```rust
struct GroupPower {
    group_key: String,          // workload UID or label value
    group_name: String,         // display name
    group_kind: String,         // "Deployment", label key, "Namespace"
    namespace: String,          // empty if group spans namespaces
    category: String,           // "platform", "operator", "application"
    cpu_uw: u64,
    memory_uw: u64,
    gpu_uw: u64,
    storage_uw: u64,
    io_uw: u64,
    total_uw: u64,
    pod_count: usize,
    pods: Vec<PodPowerSummary>,
}
```

The optional `category` filter restricts results to one category before grouping.

### 4. API

Single flexible endpoint:

```
GET /api/v1/applications?group_by=workload
GET /api/v1/applications?group_by=workload&category=application
GET /api/v1/applications?group_by=label:argocd.argoproj.io/instance
GET /api/v1/applications?group_by=namespace
GET /api/v1/applications?category=platform
```

Returns `Vec<GroupPower>` sorted by total power descending.

### 5. UI

#### New Applications page

Top-level page in the nav (alongside Nodes, Namespaces):

- Category tabs: All | Applications | Operators | Platform
- Group by dropdown: Workload (default), or any captured label key
- Table: group name, kind, namespace, CPU W, Memory W, GPU W, Total W, pod count
- Sorting: by total power descending (default), clickable column headers
- Drill-down: click a row to expand/navigate to individual pods in that group

#### Cluster overview update

The existing PowerManagementPage summary table gets a new row showing the category power split: Platform / Operators / Applications watts.

### 6. Edge Cases

- **Bare pods**: `workload_kind = "Pod"`, categorized as `application`
- **CronJob -> Job -> Pod**: full chain walked, `workload_kind = "CronJob"`
- **Pod with deleted owner**: `workload_kind = "Unknown"`, name from ownerReference field
- **Empty label map**: pod has no labels matching configured set. Grouped by workload normally.
- **Multiple ownerReferences**: use the one with `controller: true`, or first

### 7. Testing

- **keck-agent**: unit tests for owner chain resolution (Pod->RS->Deployment, Pod->StatefulSet, bare pod, deleted owner, CronJob chain). Mock K8s API responses.
- **keck-controller**: unit tests for `group_power()` with all GroupBy variants and category filtering. Test aggregation correctness (same workload_uid pods sum correctly).
- **keck-controller API**: integration tests for `/api/v1/applications` with query param combinations.
- **Existing tests**: update `make_pod` / `make_agent_report` helpers to include new fields with sensible defaults. All existing tests must continue passing.
