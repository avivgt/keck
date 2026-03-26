# Deploying Keck on OpenShift

This guide covers the complete deployment of Keck on OpenShift Container Platform (OCP),
including building all images on the cluster, installing the operator via OLM,
and deploying the agent, controller, and UI.

## Prerequisites

- OpenShift 4.14+ cluster with admin access
- `oc` CLI logged in to the cluster
- Keck source code available locally (or via git)

## Architecture on OpenShift

```
┌─────────────────────────────────────────────────────────────────┐
│                    OpenShift Cluster                             │
│                                                                  │
│  Namespace: keck-system                                         │
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
│  │ keck-operator│  │keck-controller│ │   keck-ui    │          │
│  │ (Deployment) │  │ (Deployment) │  │ (Deployment) │          │
│  │              │  │              │  │              │          │
│  │ Manages CRDs │  │ Aggregation  │  │ Dashboard    │          │
│  │ and workloads│  │ Carbon/Sched │  │ (nginx)      │          │
│  └──────────────┘  └──────────────┘  └──────────────┘          │
│                                                                  │
│  ┌──────────────────────────────────────────────────────┐      │
│  │ keck-agent (DaemonSet) — one per node                │      │
│  │ Privileged, hostPID, /host/proc + /host/sys mounts  │      │
│  │ Probes: Redfish Telemetry → Sensors → RAPL (auto)   │      │
│  │ GPU: DCGM exporter (per-pod measured power)         │      │
│  │ eBPF: sched_switch + cpu_frequency tracepoints      │      │
│  └──────────────────────────────────────────────────────┘      │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Step 1: Create the Project

```bash
oc new-project keck-system
```

## Step 2: Build Images on OpenShift

All images are built directly on the cluster using OpenShift's built-in
build system. No local container runtime (podman/docker) required.

### 2.1 Build the Operator

```bash
oc new-build --name=keck-operator --binary --strategy=docker -n keck-system

oc start-build keck-operator \
  --from-dir=keck-operator \
  --follow \
  -n keck-system
```

### 2.2 Build the Controller

The controller uses `keck-controller/Dockerfile` which builds from UBI9 base images.
The build context is the repo root (Dockerfile references `keck-common/` and `keck-controller/`):

```bash
oc new-build --name=keck-controller --binary --strategy=docker -n keck-system
oc patch bc keck-controller -n keck-system --type=json \
  -p '[{"op":"add","path":"/spec/strategy/dockerStrategy/dockerfilePath","value":"keck-controller/Dockerfile"}]'
oc start-build keck-controller --from-dir=. --follow -n keck-system
```

### 2.3 Build the Agent

The agent uses `keck-agent/Dockerfile` which builds from UBI9 base images
with Rust nightly and eBPF toolchain. The build context is the repo root:

```bash
oc new-build --name=keck-agent --binary --strategy=docker -n keck-system
oc patch bc keck-agent -n keck-system --type=json \
  -p '[{"op":"add","path":"/spec/strategy/dockerStrategy/dockerfilePath","value":"keck-agent/Dockerfile"}]'
oc start-build keck-agent --from-dir=. --follow -n keck-system
```

### 2.4 Build the UI (OpenShift Console Plugin)

The Keck UI runs as an OpenShift Dynamic Console Plugin called
"Power Management". It integrates directly into the OpenShift console
navigation — no separate URL needed.

```bash
oc new-build --name=keck-power-management --binary --strategy=docker -n keck-system

oc start-build keck-power-management \
  --from-dir=keck-ui \
  --follow \
  -n keck-system
```

### 2.5 Verify All Images

```bash
oc get imagestreams -n keck-system

# Expected:
# NAME              IMAGE REPOSITORY                                                         TAGS
# keck-agent        image-registry.openshift-image-registry.svc:5000/keck-system/keck-agent        latest
# keck-controller   image-registry.openshift-image-registry.svc:5000/keck-system/keck-controller   latest
# keck-operator     image-registry.openshift-image-registry.svc:5000/keck-system/keck-operator     latest
# keck-ui           image-registry.openshift-image-registry.svc:5000/keck-system/keck-ui           latest
```

## Step 3: Install the Operator

You have two options:

- **Option A (Recommended):** Install via the OpenShift UI (OperatorHub) — see Step 3A
- **Option B:** Install manually via CLI — see Step 3B

### Step 3A: Install via OperatorHub (UI)

This makes the Keck Operator appear in **Operators → OperatorHub** so you
can install it by clicking "Install" in the console.

Run the automated script:

```bash
bash keck-operator/config/olm/install-via-ui.sh
```

This will:
1. Build the operator, bundle, and catalog images on OCP
2. Create a CatalogSource in `openshift-marketplace`
3. Make "Keck Operator" available in OperatorHub

Then in the OpenShift console:
1. Go to **Operators → OperatorHub**
2. Search for **"Keck"**
3. Click **Keck Operator** → **Install**
4. Choose the namespace and click **Install**
5. After installation, go to **Operators → Installed Operators → Keck Operator**
6. Click **Create KeckCluster** to deploy agents and controller

Skip to **Step 4** after installing.

### Step 3B: Install Manually (CLI)

#### 3B.1 Install CRDs

```bash
oc apply -f keck-operator/config/crd/bases/
```

#### 3B.2 Install RBAC

```bash
oc apply -f keck-operator/config/rbac/role.yaml

# Add nodes/pods read permission (required for agent RBAC creation)
oc patch clusterrole keck-operator --type=json -p='[
  {"op": "add", "path": "/rules/-", "value": {
    "apiGroups":[""], "resources":["nodes","pods"], "verbs":["get","list","watch"]
  }}
]'
```

#### 3B.3 Deploy the Operator

Update the manager manifest to use the internal registry image:

```bash
# Get the operator image reference
OPERATOR_IMAGE=$(oc get istag keck-operator:latest -n keck-system \
  -o jsonpath='{.image.dockerImageReference}')

# Apply manager manifest
oc apply -f keck-operator/config/manager/manager.yaml

# Point to the internally built image
oc set image deployment/keck-operator \
  manager="$OPERATOR_IMAGE" \
  -n keck-system

# Grant image pull access
oc policy add-role-to-user system:image-puller \
  system:serviceaccount:keck-system:keck-operator \
  -n keck-system
```

#### 3B.4 Register with OLM (Installed Operators)

To make Keck appear in the OpenShift console under Installed Operators:

```bash
# Create OperatorGroup
cat <<'EOF' | oc apply -f -
apiVersion: operators.coreos.com/v1
kind: OperatorGroup
metadata:
  name: keck-operator-group
  namespace: keck-system
spec:
  targetNamespaces: []
EOF

# Apply ClusterServiceVersion (update the image reference)
OPERATOR_IMAGE=$(oc get istag keck-operator:latest -n keck-system \
  -o jsonpath='{.image.dockerImageReference}')

sed "s|namespace: placeholder|namespace: keck-system|; \
     s|quay.io/aguetta/keck-operator:0.1.0|${OPERATOR_IMAGE}|g" \
  keck-operator/bundle/manifests/keck-operator.clusterserviceversion.yaml | \
  oc apply -n keck-system -f -
```

#### 3B.5 Verify Operator

```bash
oc get csv -n keck-system | grep keck
# keck-operator.v0.1.0   Keck Operator   0.1.0   Succeeded

oc get pods -n keck-system -l control-plane=keck-operator
# keck-operator-xxxx   1/1   Running
```

The operator is now visible in **Operators → Installed Operators** in
the OpenShift web console.

## Step 4: Deploy Keck

### 4.1 Grant Privileged SCC to Agent

The agent requires privileged access for eBPF, /proc, and /sys:

```bash
oc adm policy add-scc-to-user privileged \
  -z keck-agent \
  -n keck-system
```

### 4.2 Create KeckCluster

From the OpenShift console: **Operators → Installed Operators → Keck Operator
→ KeckCluster → Create KeckCluster**

Or from CLI:

```bash
oc apply -f keck-operator/config/samples/keckcluster.yaml
```

### 4.3 Point Agent and Controller to Internal Images

The operator creates the DaemonSet and Deployment with default image
references. Update them to use the images built on the cluster:

```bash
# Controller
CONTROLLER_IMAGE=$(oc get istag keck-controller:latest -n keck-system \
  -o jsonpath='{.image.dockerImageReference}')
oc set image deployment/keck-controller \
  keck-controller="$CONTROLLER_IMAGE" \
  -n keck-system

# Agent
AGENT_IMAGE=$(oc get istag keck-agent:latest -n keck-system \
  -o jsonpath='{.image.dockerImageReference}')
oc set image daemonset/keck-agent \
  keck-agent="$AGENT_IMAGE" \
  -n keck-system

# Restart agent to pick up SCC + image
oc rollout restart daemonset/keck-agent -n keck-system
```

### 4.4 Deploy the Power Management Console Plugin

The UI integrates directly into the OpenShift console as "Power Management"
in the left navigation. No separate URL or route needed.

```bash
# Get the plugin image reference
UI_IMAGE=$(oc get istag keck-power-management:latest -n keck-system \
  -o jsonpath='{.image.dockerImageReference}')

# Deploy the plugin (Deployment + Service + ConsolePlugin CR)
oc apply -f keck-ui/openshift/console-plugin.yaml

# Point to the internally built image
oc set image deployment/keck-power-management \
  keck-power-management="$UI_IMAGE" \
  -n keck-system

# Enable the plugin in the OpenShift console
bash keck-ui/openshift/enable-plugin.sh
```

### 4.5 Verify Everything

```bash
oc get pods -n keck-system
# NAME                                     READY   STATUS    AGE
# keck-agent-xxxxx                         1/1     Running   ...
# keck-agent-yyyyy                         1/1     Running   ...
# keck-controller-xxxxx-yyyyy              1/1     Running   ...
# keck-operator-xxxxx-yyyyy                1/1     Running   ...
# keck-power-management-xxxxx-yyyyy        1/1     Running   ...

oc get keckclusters
# NAME   AGENTS   CONTROLLER   PHASE     AGE
# keck   2        true         Running   ...

oc get consoleplugins
# NAME                     AGE
# keck-power-management    ...
```

## Step 5: Access Power Management

After enabling the plugin, refresh the OpenShift console. "Power Management"
appears in the left navigation under the admin perspective, after Monitoring.

```
OpenShift Console
├── Home
├── Operators
├── Workloads
├── Networking
├── Storage
├── Monitoring
├── Power Management        ← Keck
│   ├── Overview            — Cluster power, carbon, cost
│   ├── Namespaces          — Per-namespace breakdown (click to drill down)
│   ├── Nodes               — Per-node power and headroom
│   ├── Power Budgets       — Budget status and enforcement
│   └── Carbon & Cost       — Carbon intensity and cost tracking
├── Compute
└── ...
```

No separate URL needed. The plugin uses the console's existing auth,
styling (PatternFly), and routing.

To verify the plugin is loaded:

```bash
# Check plugin is registered
oc get consoleplugins keck-power-management

# Check console operator has it enabled
oc get console.operator.openshift.io cluster -o jsonpath='{.spec.plugins}'
# Should include "keck-power-management"
```

## Step 6: Configure Power Budgets (Optional)

### Per-Namespace Budget

From the console: **Operators → Installed Operators → Keck Operator
→ PowerBudget → Create PowerBudget**

Or from CLI:

```bash
oc apply -f keck-operator/config/samples/powerbudget.yaml

oc get powerbudgets -A
# NAMESPACE      NAME                 BUDGET (W)   CURRENT (W)   USAGE   EXCEEDED
# ml-training    ml-training-budget   10000        7234          72%     false
```

### Per-Node Profiles

```bash
oc apply -f keck-operator/config/samples/powerprofile.yaml

oc get powerprofiles
# NAME              PROFILE   NODES   AGE
# gpu-nodes-full    full      4       1m
# edge-minimal      minimal   2       1m
```

## Step 7: Prometheus Integration (Optional)

If you have the OpenShift monitoring stack (Prometheus/Thanos):

```bash
cat <<'EOF' | oc apply -f -
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: keck-agent
  namespace: keck-system
spec:
  selector:
    matchLabels:
      app.kubernetes.io/name: keck-agent
  endpoints:
    - port: metrics
      interval: 15s
EOF
```

## Rebuilding After Code Changes

When you update the Keck source code, rebuild the affected image:

```bash
# Rebuild agent
oc start-build keck-agent --from-dir=<build-context> --follow -n keck-system
oc rollout restart daemonset/keck-agent -n keck-system

# Rebuild controller
oc start-build keck-controller --from-dir=<build-context> --follow -n keck-system
oc rollout restart deployment/keck-controller -n keck-system

# Rebuild UI
oc start-build keck-ui --from-dir=keck-ui --follow -n keck-system
oc rollout restart deployment/keck-ui -n keck-system

# Rebuild operator
oc start-build keck-operator --from-dir=keck-operator --follow -n keck-system
oc rollout restart deployment/keck-operator -n keck-system
```

## Uninstalling

```bash
# Delete KeckCluster (removes agent DaemonSet and controller Deployment)
oc delete keckclusters --all

# Delete UI
oc delete deployment keck-ui -n keck-system
oc delete service keck-ui -n keck-system
oc delete route keck-ui -n keck-system

# Delete operator
oc delete csv keck-operator.v0.1.0 -n keck-system
oc delete operatorgroup keck-operator-group -n keck-system
oc delete deployment keck-operator -n keck-system

# Delete RBAC
oc delete -f keck-operator/config/rbac/role.yaml
oc adm policy remove-scc-from-user privileged -z keck-agent -n keck-system

# Delete CRDs
oc delete -f keck-operator/config/crd/bases/

# Delete builds and images
oc delete buildconfigs --all -n keck-system
oc delete imagestreams --all -n keck-system

# Delete project
oc delete project keck-system
```

## Troubleshooting

### Agent pods not starting

```bash
# Check SCC
oc get scc privileged -o yaml | grep keck

# If missing:
oc adm policy add-scc-to-user privileged -z keck-agent -n keck-system
oc rollout restart daemonset/keck-agent -n keck-system
```

### ImagePullBackOff

```bash
# Check if image exists
oc get istag <image-name>:latest -n keck-system

# If missing, rebuild:
oc start-build <build-name> --from-dir=<context> --follow -n keck-system

# Grant pull access
oc policy add-role-to-user system:image-puller \
  system:serviceaccount:keck-system:<service-account> \
  -n keck-system
```

### Operator CSV stuck in Pending

```bash
# Check requirements
oc get csv keck-operator.v0.1.0 -n keck-system -o jsonpath='{.status.message}'

# Usually missing RBAC — check:
oc get csv keck-operator.v0.1.0 -n keck-system \
  -o jsonpath='{.status.requirementStatus}' | python3 -m json.tool
```

### Build failures

```bash
# Check build logs
oc logs build/keck-agent-N -n keck-system

# List builds
oc get builds -n keck-system
```
