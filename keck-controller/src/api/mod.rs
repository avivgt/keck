// SPDX-License-Identifier: Apache-2.0

//! REST API: serves live power data and accepts agent reports.
//!
//! Endpoints:
//!   POST /api/v1/report              — agent pushes AgentReport
//!   GET  /api/v1/cluster             — cluster power summary
//!   GET  /api/v1/namespaces          — per-namespace breakdown
//!   GET  /api/v1/namespaces/:ns      — pods in a namespace
//!   GET  /api/v1/nodes               — per-node summary
//!   GET  /api/v1/nodes/:name         — single node detail
//!   GET  /api/v1/pods/:uid           — single pod power
//!   GET  /healthz                    — health check

use std::sync::Arc;

use axum::{
    Router,
    Json,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use tower_http::cors::CorsLayer;
use tokio::sync::RwLock;

use crate::aggregator::{AgentReport, ClusterAggregator};

/// Shared state passed to all handlers.
type AppState = Arc<RwLock<ClusterAggregator>>;

/// Start the REST API server.
pub async fn start_rest_server(
    aggregator: AppState,
    bind_addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        // Agent report ingestion
        .route("/api/v1/report", post(handle_report))
        // Read endpoints
        .route("/api/v1/cluster", get(handle_cluster))
        .route("/api/v1/namespaces", get(handle_namespaces))
        .route("/api/v1/namespaces/{ns}", get(handle_namespace_pods))
        .route("/api/v1/nodes", get(handle_nodes))
        .route("/api/v1/nodes/{name}", get(handle_node))
        .route("/api/v1/pods/{uid}", get(handle_pod))
        // Health check
        .route("/healthz", get(handle_healthz))
        // CORS for console plugin
        .layer(CorsLayer::permissive())
        .with_state(aggregator);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    log::info!("REST API listening on {}", bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

/// POST /api/v1/report — agent pushes its power data.
async fn handle_report(
    State(aggregator): State<AppState>,
    Json(report): Json<AgentReport>,
) -> StatusCode {
    let node_name = report.node.node_name.clone();
    let pod_count = report.pods.len();

    let mut agg = aggregator.write().await;
    agg.ingest(report);

    log::debug!(
        "Ingested report from node '{}': {} pods",
        node_name,
        pod_count,
    );

    StatusCode::OK
}

/// GET /api/v1/cluster — cluster-wide power summary.
async fn handle_cluster(
    State(aggregator): State<AppState>,
) -> Json<serde_json::Value> {
    let agg = aggregator.read().await;
    let power = agg.cluster_power();

    Json(serde_json::json!({
        "cpu_watts": power.cpu_uw as f64 / 1e6,
        "memory_watts": power.memory_uw as f64 / 1e6,
        "gpu_watts": power.gpu_uw as f64 / 1e6,
        "platform_watts": power.platform_uw as f64 / 1e6,
        "idle_watts": power.idle_uw as f64 / 1e6,
        "total_attributed_watts": power.total_attributed_uw as f64 / 1e6,
        "node_count": power.node_count,
        "pod_count": power.pod_count,
        "avg_error_ratio": power.avg_error_ratio,
    }))
}

/// GET /api/v1/namespaces — per-namespace power breakdown.
async fn handle_namespaces(
    State(aggregator): State<AppState>,
) -> Json<serde_json::Value> {
    let agg = aggregator.read().await;
    let namespaces = agg.namespace_power();

    let ns_list: Vec<serde_json::Value> = namespaces
        .iter()
        .map(|ns| {
            serde_json::json!({
                "namespace": ns.namespace,
                "cpu_watts": ns.cpu_uw as f64 / 1e6,
                "memory_watts": ns.memory_uw as f64 / 1e6,
                "gpu_watts": ns.gpu_uw as f64 / 1e6,
                "total_watts": ns.total_uw as f64 / 1e6,
                "pod_count": ns.pod_count,
            })
        })
        .collect();

    Json(serde_json::Value::Array(ns_list))
}

/// GET /api/v1/namespaces/:ns — pods in a namespace.
async fn handle_namespace_pods(
    State(aggregator): State<AppState>,
    Path(ns): Path<String>,
) -> Json<serde_json::Value> {
    let agg = aggregator.read().await;
    let pods = agg.pods_in_namespace(&ns);

    let pod_list: Vec<serde_json::Value> = pods
        .iter()
        .map(|p| {
            serde_json::json!({
                "pod_uid": p.pod_uid,
                "pod_name": p.pod_name,
                "namespace": p.namespace,
                "node_name": p.node_name,
                "cpu_watts": p.cpu_uw as f64 / 1e6,
                "memory_watts": p.memory_uw as f64 / 1e6,
                "gpu_watts": p.gpu_uw as f64 / 1e6,
                "total_watts": p.total_uw as f64 / 1e6,
            })
        })
        .collect();

    Json(serde_json::Value::Array(pod_list))
}

/// GET /api/v1/nodes — all node summaries.
async fn handle_nodes(
    State(aggregator): State<AppState>,
) -> Json<serde_json::Value> {
    let agg = aggregator.read().await;
    let nodes = agg.node_summaries();

    let node_list: Vec<serde_json::Value> = nodes
        .iter()
        .map(|n| {
            serde_json::json!({
                "node_name": n.node_name,
                "cpu_watts": n.cpu_uw as f64 / 1e6,
                "memory_watts": n.memory_uw as f64 / 1e6,
                "gpu_watts": n.gpu_uw as f64 / 1e6,
                "platform_watts": n.platform_uw.map(|v| v as f64 / 1e6),
                "idle_watts": n.idle_uw as f64 / 1e6,
                "error_ratio": n.error_ratio,
                "pod_count": n.pod_count,
                "headroom_watts": n.headroom_uw.map(|v| v as f64 / 1e6),
                "last_seen_secs_ago": n.last_seen_secs_ago,
            })
        })
        .collect();

    Json(serde_json::Value::Array(node_list))
}

/// GET /api/v1/nodes/:name — single node.
async fn handle_node(
    State(aggregator): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let agg = aggregator.read().await;
    let node = agg.node_summary(&name).ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::json!({
        "node_name": node.node_name,
        "cpu_watts": node.cpu_uw as f64 / 1e6,
        "memory_watts": node.memory_uw as f64 / 1e6,
        "gpu_watts": node.gpu_uw as f64 / 1e6,
        "platform_watts": node.platform_uw.map(|v| v as f64 / 1e6),
        "idle_watts": node.idle_uw as f64 / 1e6,
        "error_ratio": node.error_ratio,
        "pod_count": node.pod_count,
    })))
}

/// GET /api/v1/pods/:uid — single pod.
async fn handle_pod(
    State(aggregator): State<AppState>,
    Path(uid): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let agg = aggregator.read().await;
    let pod = agg.pod_power(&uid).ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::json!({
        "pod_uid": pod.pod_uid,
        "pod_name": pod.pod_name,
        "namespace": pod.namespace,
        "node_name": pod.node_name,
        "cpu_watts": pod.cpu_uw as f64 / 1e6,
        "memory_watts": pod.memory_uw as f64 / 1e6,
        "gpu_watts": pod.gpu_uw as f64 / 1e6,
        "total_watts": pod.total_uw as f64 / 1e6,
    })))
}

/// GET /healthz
async fn handle_healthz() -> &'static str {
    "ok"
}
