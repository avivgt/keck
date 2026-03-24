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

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Router,
    Json,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use tower_http::cors::CorsLayer;
use tokio::sync::RwLock;

use crate::aggregator::{AgentReport, ClusterAggregator};

/// Shared state: aggregator + optional API key for report auth.
#[derive(Clone)]
pub struct ServerState {
    aggregator: Arc<RwLock<ClusterAggregator>>,
    api_key: Option<String>,
}

/// Start the REST API server.
pub async fn start_rest_server(
    aggregator: Arc<RwLock<ClusterAggregator>>,
    bind_addr: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("KECK_API_KEY").ok();
    if api_key.is_some() {
        log::info!("API key configured — report endpoint requires authentication");
    } else {
        log::warn!("KECK_API_KEY not set — report endpoint is unauthenticated");
    }

    let state = ServerState {
        aggregator,
        api_key,
    };
    let app = Router::new()
        // Agent report ingestion (authenticated)
        .route("/api/v1/report", post(handle_report))
        // Read endpoints
        .route("/api/v1/cluster", get(handle_cluster))
        .route("/api/v1/namespaces", get(handle_namespaces))
        .route("/api/v1/pods-by-namespace", get(handle_namespace_pods))
        .route("/api/v1/nodes", get(handle_nodes))
        .route("/api/v1/pods-by-node", get(handle_node))
        .route("/api/v1/pods-by-uid", get(handle_pod))
        // Health check
        .route("/healthz", get(handle_healthz))
        // CORS: allow specific origin if configured, deny cross-origin by default
        .layer(if let Ok(origin) = std::env::var("KECK_CORS_ORIGIN") {
            log::info!("CORS allowed for origin: {}", origin);
            CorsLayer::new()
                .allow_origin(origin.parse::<axum::http::HeaderValue>().expect("invalid KECK_CORS_ORIGIN"))
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers([axum::http::header::CONTENT_TYPE, axum::http::header::AUTHORIZATION])
        } else {
            CorsLayer::new()
        })
        // 1MB body limit (a 500-pod report is ~100KB)
        .layer(DefaultBodyLimit::max(1_048_576))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    log::info!("REST API listening on {}", bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Validate bearer token against configured API key.
fn check_auth(headers: &HeaderMap, api_key: &Option<String>) -> Result<(), StatusCode> {
    let expected = match api_key {
        Some(key) => key,
        None => return Ok(()), // No key configured — allow all
    };

    let header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if token != expected {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(())
}

/// Max pods per report — no real node has more than this.
const MAX_PODS_PER_REPORT: usize = 2000;
/// Max string length for names (K8s limit is 253 for DNS names).
const MAX_NAME_LEN: usize = 512;
/// Max power per component in microwatts (100kW — no single server draws more).
const MAX_POWER_UW: u64 = 100_000_000_000;

/// Validate an agent report before ingestion.
fn validate_report(report: &AgentReport) -> Result<(), String> {
    let node = &report.node;

    if node.node_name.is_empty() || node.node_name.len() > MAX_NAME_LEN {
        return Err(format!("invalid node_name length: {}", node.node_name.len()));
    }

    if node.cpu_uw > MAX_POWER_UW {
        return Err(format!("cpu_uw exceeds max: {}", node.cpu_uw));
    }
    if node.memory_uw > MAX_POWER_UW {
        return Err(format!("memory_uw exceeds max: {}", node.memory_uw));
    }
    if node.gpu_uw > MAX_POWER_UW {
        return Err(format!("gpu_uw exceeds max: {}", node.gpu_uw));
    }

    if report.pods.len() > MAX_PODS_PER_REPORT {
        return Err(format!("too many pods: {} (max {})", report.pods.len(), MAX_PODS_PER_REPORT));
    }

    for pod in &report.pods {
        if pod.pod_uid.is_empty() || pod.pod_uid.len() > MAX_NAME_LEN {
            return Err(format!("invalid pod_uid length: {}", pod.pod_uid.len()));
        }
        if pod.pod_name.len() > MAX_NAME_LEN {
            return Err(format!("pod_name too long: {}", pod.pod_name.len()));
        }
        if pod.namespace.len() > MAX_NAME_LEN {
            return Err(format!("namespace too long: {}", pod.namespace.len()));
        }
        if pod.total_uw > MAX_POWER_UW {
            return Err(format!("pod {} total_uw exceeds max: {}", pod.pod_uid, pod.total_uw));
        }
    }

    Ok(())
}

/// POST /api/v1/report — agent pushes its power data (authenticated + validated).
async fn handle_report(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(report): Json<AgentReport>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Err(status) = check_auth(&headers, &state.api_key) {
        log::warn!("Rejected unauthenticated report from '{}'", report.node.node_name);
        return Err((status, "unauthorized".into()));
    }

    if let Err(reason) = validate_report(&report) {
        log::warn!("Rejected invalid report from '{}': {}", report.node.node_name, reason);
        return Err((StatusCode::BAD_REQUEST, reason));
    }

    let node_name = report.node.node_name.clone();
    let pod_count = report.pods.len();

    let mut agg = state.aggregator.write().await;
    agg.ingest(report);

    log::debug!(
        "Ingested report from node '{}': {} pods",
        node_name,
        pod_count,
    );

    Ok(StatusCode::OK)
}

/// GET /api/v1/cluster — cluster-wide power summary.
async fn handle_cluster(
    State(state): State<ServerState>,
) -> Json<serde_json::Value> {
    let agg = state.aggregator.read().await;
    let power = agg.cluster_power();

    let has_platform = power.platform_uw > 0;
    let has_gpu = power.gpu_uw > 0;

    // Source info from agents
    let cpu_info = agg.cpu_source_info();
    let mem_info = agg.memory_source_info();

    // Per-node breakdown
    let nodes = agg.node_summaries();
    let nodes_json: Vec<serde_json::Value> = nodes.iter().map(|n| {
        serde_json::json!({
            "node_name": n.node_name,
            "cpu_watts": n.cpu_uw as f64 / 1e6,
            "memory_watts": n.memory_uw as f64 / 1e6,
            "gpu_watts": n.gpu_uw as f64 / 1e6,
            "platform_watts": n.platform_uw.map(|v| v as f64 / 1e6),
            "idle_watts": n.idle_uw as f64 / 1e6,
            "pod_count": n.pod_count,
            "error_ratio": n.error_ratio,
            "cpu_source": n.cpu_source,
            "cpu_reading_type": n.cpu_reading_type,
        })
    }).collect();

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
        "nodes": nodes_json,
        "sources": agg.all_sources().iter().map(|s| {
            serde_json::json!({
                "name": s.name,
                "node_name": s.node_name,
                "component": s.component,
                "reading_type": s.reading_type,
                "available": s.available,
                "selected": s.selected,
                "watts": s.power_uw as f64 / 1e6,
            })
        }).collect::<Vec<_>>(),
        "data_quality": {
            "cpu": {
                "source": &cpu_info.0,
                "type": &cpu_info.1,
                "available": power.cpu_uw > 0,
            },
            "memory": {
                "source": &mem_info,
                "type": if mem_info.contains("Redfish") { "measured" } else if power.memory_uw > 0 { "estimated" } else { "unavailable" },
                "available": power.memory_uw > 0,
            },
            "gpu": {
                "source": if has_gpu { "NVML" } else { "none" },
                "type": if has_gpu { "measured" } else { "unavailable" },
                "available": has_gpu,
            },
            "platform": {
                "source": if has_platform { "Redfish PSU" } else { "none" },
                "type": if has_platform { "measured" } else { "unavailable" },
                "available": has_platform,
                "note": if has_platform {
                    "PSU measured power (ground truth)"
                } else {
                    "No PSU power source — configure Redfish/IPMI for ground truth."
                }
            },
            "attribution": {
                "method": "ebpf+proc",
                "note": "CPU time per-process + RSS for memory. eBPF sched_switch for active pods."
            },
            "alerts": {
                "missing_ground_truth": !has_platform,
                "missing_gpu": !has_gpu,
                "message": if !has_platform {
                    "No platform power source. Configure iDRAC to get measured power."
                } else {
                    "All data sources available."
                }
            }
        }
    }))
}

/// GET /api/v1/namespaces — per-namespace power breakdown.
async fn handle_namespaces(
    State(state): State<ServerState>,
) -> Json<serde_json::Value> {
    let agg = state.aggregator.read().await;
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

/// GET /api/v1/pods-by-namespace?ns=<namespace> — pods in a namespace.
async fn handle_namespace_pods(
    State(state): State<ServerState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let ns = params.get("ns").cloned().unwrap_or_default();
    let agg = state.aggregator.read().await;
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
    State(state): State<ServerState>,
) -> Json<serde_json::Value> {
    let agg = state.aggregator.read().await;
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

/// GET /api/v1/pods-by-node?name=<node> — single node.
async fn handle_node(
    State(state): State<ServerState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let name = params.get("name").cloned().unwrap_or_default();
    let agg = state.aggregator.read().await;
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

/// GET /api/v1/pods-by-uid?uid=<uid> — single pod.
async fn handle_pod(
    State(state): State<ServerState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let uid = params.get("uid").cloned().unwrap_or_default();
    let agg = state.aggregator.read().await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregator::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn build_app(state: ServerState) -> Router {
        Router::new()
            .route("/api/v1/report", post(handle_report))
            .route("/api/v1/cluster", get(handle_cluster))
            .route("/api/v1/namespaces", get(handle_namespaces))
            .route("/api/v1/pods-by-namespace", get(handle_namespace_pods))
            .route("/api/v1/nodes", get(handle_nodes))
            .route("/api/v1/pods-by-node", get(handle_node))
            .route("/api/v1/pods-by-uid", get(handle_pod))
            .route("/healthz", get(handle_healthz))
            .with_state(state)
    }

    fn make_state() -> ServerState {
        ServerState {
            aggregator: Arc::new(RwLock::new(ClusterAggregator::new())),
            api_key: None,
        }
    }

    fn make_agent_report() -> AgentReport {
        AgentReport {
            node: NodePowerReport {
                node_name: "test-node".into(),
                cpu_uw: 5_000_000,
                memory_uw: 2_000_000,
                gpu_uw: 0,
                platform_uw: Some(10_000_000),
                idle_uw: 3_000_000,
                error_ratio: 0.05,
                pod_count: 1,
                process_count: 50,
                timestamp: std::time::SystemTime::now(),
                cpu_source: "rapl".into(),
                memory_source: "estimated".into(),
                cpu_reading_type: "estimated".into(),
                sources: vec![],
            },
            pods: vec![PodPowerReport {
                node_name: "test-node".into(),
                pod_uid: "test-uid-123".into(),
                pod_name: "web-app-1".into(),
                namespace: "production".into(),
                cpu_uw: 1_000_000,
                memory_uw: 500_000,
                gpu_uw: 0,
                total_uw: 1_500_000,
                timestamp: std::time::SystemTime::now(),
            }],
        }
    }

    #[tokio::test]
    async fn test_healthz() {
        let app = build_app(make_state());
        let req = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn test_post_report() {
        let state = make_state();
        let app = build_app(state.clone());
        let report = make_agent_report();

        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/report")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&report).unwrap()))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify the data was ingested
        let agg = state.aggregator.read().await;
        assert_eq!(agg.node_count(), 1);
    }

    #[tokio::test]
    async fn test_get_cluster_empty() {
        let app = build_app(make_state());
        let req = Request::builder()
            .uri("/api/v1/cluster")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["node_count"], 0);
        assert_eq!(json["pod_count"], 0);
        assert_eq!(json["cpu_watts"], 0.0);
    }

    #[tokio::test]
    async fn test_get_cluster_with_data() {
        let state = make_state();
        {
            let mut agg = state.aggregator.write().await;
            agg.ingest(make_agent_report());
        }
        let app = build_app(state);

        let req = Request::builder()
            .uri("/api/v1/cluster")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["node_count"], 1);
        assert_eq!(json["pod_count"], 1);
        assert_eq!(json["cpu_watts"], 5.0); // 5_000_000 uw / 1e6
    }

    #[tokio::test]
    async fn test_get_namespaces() {
        let state = make_state();
        {
            let mut agg = state.aggregator.write().await;
            agg.ingest(make_agent_report());
        }
        let app = build_app(state);

        let req = Request::builder()
            .uri("/api/v1/namespaces")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["namespace"], "production");
        assert_eq!(arr[0]["pod_count"], 1);
    }

    #[tokio::test]
    async fn test_get_namespace_pods() {
        let state = make_state();
        {
            let mut agg = state.aggregator.write().await;
            agg.ingest(make_agent_report());
        }
        let app = build_app(state);

        let req = Request::builder()
            .uri("/api/v1/pods-by-namespace?ns=production")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["pod_name"], "web-app-1");
    }

    #[tokio::test]
    async fn test_get_namespace_pods_empty_ns() {
        let state = make_state();
        {
            let mut agg = state.aggregator.write().await;
            agg.ingest(make_agent_report());
        }
        let app = build_app(state);

        let req = Request::builder()
            .uri("/api/v1/pods-by-namespace?ns=nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_get_nodes() {
        let state = make_state();
        {
            let mut agg = state.aggregator.write().await;
            agg.ingest(make_agent_report());
        }
        let app = build_app(state);

        let req = Request::builder()
            .uri("/api/v1/nodes")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["node_name"], "test-node");
    }

    #[tokio::test]
    async fn test_get_node_found() {
        let state = make_state();
        {
            let mut agg = state.aggregator.write().await;
            agg.ingest(make_agent_report());
        }
        let app = build_app(state);

        let req = Request::builder()
            .uri("/api/v1/pods-by-node?name=test-node")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["node_name"], "test-node");
        assert_eq!(json["cpu_watts"], 5.0);
    }

    #[tokio::test]
    async fn test_get_node_not_found() {
        let app = build_app(make_state());
        let req = Request::builder()
            .uri("/api/v1/pods-by-node?name=nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_pod_found() {
        let state = make_state();
        {
            let mut agg = state.aggregator.write().await;
            agg.ingest(make_agent_report());
        }
        let app = build_app(state);

        let req = Request::builder()
            .uri("/api/v1/pods-by-uid?uid=test-uid-123")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["pod_uid"], "test-uid-123");
        assert_eq!(json["pod_name"], "web-app-1");
        assert_eq!(json["namespace"], "production");
        assert_eq!(json["total_watts"], 1.5); // 1_500_000 / 1e6
    }

    #[tokio::test]
    async fn test_get_pod_not_found() {
        let app = build_app(make_state());
        let req = Request::builder()
            .uri("/api/v1/pods-by-uid?uid=nonexistent")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_cluster_json_has_data_quality() {
        let state = make_state();
        {
            let mut agg = state.aggregator.write().await;
            agg.ingest(make_agent_report());
        }
        let app = build_app(state);

        let req = Request::builder()
            .uri("/api/v1/cluster")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 8192).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Verify data_quality section exists
        assert!(json["data_quality"].is_object());
        assert!(json["data_quality"]["cpu"].is_object());
        assert!(json["data_quality"]["memory"].is_object());
        assert!(json["data_quality"]["gpu"].is_object());
        assert!(json["data_quality"]["platform"].is_object());
    }
}
