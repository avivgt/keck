// SPDX-License-Identifier: Apache-2.0

//! API layer: gRPC (agent reports), REST (dashboards), K8s custom metrics.
//!
//! Three API surfaces:
//!
//! 1. gRPC server (port 9090): receives AgentReport streams from node agents.
//!    Bidirectional: controller can also send drill-down queries back.
//!
//! 2. REST server (port 8080): serves JSON for dashboards and CLI tools.
//!    Endpoints:
//!      GET /api/v1/cluster          — cluster power summary
//!      GET /api/v1/namespaces       — per-namespace breakdown
//!      GET /api/v1/namespaces/{ns}  — pods in a namespace
//!      GET /api/v1/nodes            — per-node summary
//!      GET /api/v1/nodes/{name}     — node detail
//!      GET /api/v1/pods/{uid}       — pod power (redirects to node agent for drill-down)
//!      GET /metrics                 — Prometheus metrics
//!
//! 3. K8s Custom Metrics API: registers as an APIService so HPA can
//!    scale based on power metrics (e.g., "scale down if namespace > 10kW").

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::aggregator::ClusterAggregator;
use crate::carbon::CarbonTracker;

/// Start the gRPC server that receives agent reports.
///
/// Each node agent opens a streaming connection and sends
/// AgentReport messages every report_interval (default 10s).
pub async fn start_grpc_server(
    _aggregator: Arc<RwLock<ClusterAggregator>>,
    bind_addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Implement tonic gRPC server
    //
    // service PowerMeteringService {
    //   // Agent pushes reports (streaming)
    //   rpc ReportPower(stream AgentReport) returns (Ack);
    //
    //   // Controller queries agent for drill-down (bidirectional)
    //   rpc DrillDown(DrillDownRequest) returns (DrillDownResponse);
    // }
    //
    // On each incoming AgentReport:
    //   aggregator.write().await.ingest(report);

    log::info!("gRPC server would listen on {}", bind_addr);

    // Placeholder: keep alive
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
    }
}

/// Start the REST API server for dashboards and CLI.
pub async fn start_rest_server(
    _aggregator: Arc<RwLock<ClusterAggregator>>,
    _carbon: Arc<RwLock<CarbonTracker>>,
    bind_addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Implement with axum or actix-web
    //
    // Routes:
    //   GET /api/v1/cluster          → aggregator.read().cluster_power()
    //   GET /api/v1/namespaces       → aggregator.read().namespace_power()
    //   GET /api/v1/namespaces/:ns   → aggregator.read().pods_in_namespace(ns)
    //   GET /api/v1/nodes            → aggregator.read().node_summaries()
    //   GET /api/v1/nodes/:name      → aggregator.read().node_summary(name)
    //   GET /api/v1/pods/:uid        → aggregator.read().pod_power(uid)
    //   GET /api/v1/carbon           → carbon.read().current()
    //   GET /metrics                 → prometheus metrics
    //
    // Response format: JSON with power in watts (not microwatts)
    // {
    //   "cluster": {
    //     "cpu_watts": 1234.5,
    //     "memory_watts": 234.5,
    //     "gpu_watts": 890.0,
    //     "platform_watts": 2800.0,
    //     "idle_watts": 441.0,
    //     "node_count": 12,
    //     "pod_count": 187,
    //     "error_ratio": 0.034,
    //     "carbon_grams_per_hour": 523.4
    //   }
    // }

    log::info!("REST server would listen on {}", bind_addr);

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
    }
}

/// Register as a K8s Custom Metrics API provider.
///
/// This allows HPA to use power metrics for autoscaling:
///   kubectl autoscale deployment myapp --custom-metric power_watts --target 500
///
/// Also enables the power-aware scheduler to query pod power metrics
/// via the standard K8s metrics API.
pub async fn start_custom_metrics_api(
    _aggregator: Arc<RwLock<ClusterAggregator>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Implement K8s custom metrics API
    //
    // Register APIService:
    //   apiVersion: apiregistration.k8s.io/v1
    //   kind: APIService
    //   metadata:
    //     name: v1beta1.custom.metrics.k8s.io
    //   spec:
    //     service:
    //       name: power-controller
    //       namespace: power-system
    //     group: custom.metrics.k8s.io
    //     version: v1beta1
    //
    // Serve endpoints:
    //   /apis/custom.metrics.k8s.io/v1beta1/namespaces/{ns}/pods/{pod}/power_watts
    //   /apis/custom.metrics.k8s.io/v1beta1/namespaces/{ns}/metrics/power_watts
    //
    // This makes power metrics available to:
    //   - HPA (Horizontal Pod Autoscaler)
    //   - kubectl top pods --custom-metrics
    //   - Our scheduler extender
    //   - Any K8s controller that reads custom metrics

    log::info!("Custom metrics API would register with K8s API server");

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
    }
}
