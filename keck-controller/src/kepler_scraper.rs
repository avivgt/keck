// SPDX-License-Identifier: Apache-2.0

//! Scrapes upstream Kepler's Prometheus /metrics endpoint and converts
//! per-pod power readings into Keck's internal format for side-by-side
//! comparison with Keck's native metering.
//!
//! Kepler runs as a separate DaemonSet and exposes:
//!   kepler_pod_cpu_watts{pod_id, pod_name, pod_namespace, state, zone, node_name}
//!   kepler_pod_gpu_watts{pod_id, pod_name, pod_namespace, state, node_name}
//!   kepler_node_cpu_watts{zone, path, node_name}

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::RwLock;

use crate::aggregator::{AgentReport, ClusterAggregator, NodePowerReport, PodPowerReport};

const KEPLER_URL_DEFAULT: &str = "http://kepler.kepler.svc:28282/metrics";
const SCRAPE_INTERVAL: Duration = Duration::from_secs(10);

pub async fn run_kepler_scraper(aggregator: Arc<RwLock<ClusterAggregator>>) {
    let url = std::env::var("KEPLER_METRICS_URL").unwrap_or_else(|_| KEPLER_URL_DEFAULT.into());
    let enabled = std::env::var("KEPLER_ENABLED").unwrap_or_default();
    if enabled != "true" {
        log::info!("Kepler scraper disabled (set KEPLER_ENABLED=true to enable)");
        return;
    }

    log::info!("Kepler scraper starting, target: {}", url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to create HTTP client for Kepler scraper");

    // Wait for Kepler to start
    tokio::time::sleep(Duration::from_secs(15)).await;

    loop {
        match scrape_and_ingest(&client, &url, &aggregator).await {
            Ok(count) => log::info!("Kepler: scraped {} pods", count),
            Err(e) => log::warn!("Kepler scrape failed: {}", e),
        }
        tokio::time::sleep(SCRAPE_INTERVAL).await;
    }
}

async fn scrape_and_ingest(
    client: &reqwest::Client,
    url: &str,
    aggregator: &Arc<RwLock<ClusterAggregator>>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let body = client.get(url).send().await?.text().await?;

    let mut pod_data: HashMap<PodKey, PodAccum> = HashMap::new();
    let mut node_cpu_watts: HashMap<String, f64> = HashMap::new();
    let mut node_gpu_watts: HashMap<String, f64> = HashMap::new();
    let mut node_idle_watts: HashMap<String, f64> = HashMap::new();
    let mut node_platform_watts: HashMap<String, f64> = HashMap::new();

    for line in body.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }

        if line.starts_with("kepler_pod_cpu_watts{") {
            if let Some((key, watts)) = parse_pod_metric(line) {
                if key.state != "running" {
                    continue;
                }
                let entry = pod_data.entry(PodKey {
                    pod_id: key.pod_id,
                    pod_name: key.pod_name,
                    pod_namespace: key.pod_namespace,
                    node_name: key.node_name,
                    state: key.state,
                }).or_default();
                entry.cpu_watts += watts;
            }
        } else if line.starts_with("kepler_pod_gpu_watts{") {
            if let Some((key, watts)) = parse_pod_metric(line) {
                if key.state != "running" {
                    continue;
                }
                let entry = pod_data.entry(PodKey {
                    pod_id: key.pod_id,
                    pod_name: key.pod_name,
                    pod_namespace: key.pod_namespace,
                    node_name: key.node_name,
                    state: key.state,
                }).or_default();
                entry.gpu_watts += watts;
            }
        } else if line.starts_with("kepler_node_cpu_watts{") {
            if let Some((node, watts)) = parse_node_metric(line) {
                *node_cpu_watts.entry(node).or_default() += watts;
            }
        } else if line.starts_with("kepler_node_cpu_idle_watts{") {
            if let Some((node, watts)) = parse_node_metric(line) {
                *node_idle_watts.entry(node).or_default() += watts;
            }
        } else if line.starts_with("kepler_node_gpu_watts{") {
            if let Some((node, watts)) = parse_node_metric(line) {
                *node_gpu_watts.entry(node).or_default() += watts;
            }
        } else if line.starts_with("kepler_platform_watts{") {
            if let Some((node, watts)) = parse_node_metric(line) {
                *node_platform_watts.entry(node).or_default() += watts;
            }
        }
    }

    // Group pods by node
    let mut nodes: HashMap<String, Vec<PodPowerReport>> = HashMap::new();
    for (key, accum) in &pod_data {
        let cpu_uw = (accum.cpu_watts * 1_000_000.0) as u64;
        let gpu_uw = (accum.gpu_watts * 1_000_000.0) as u64;
        let total_uw = cpu_uw + gpu_uw;

        let pod = PodPowerReport {
            node_name: key.node_name.clone(),
            pod_uid: key.pod_id.clone(),
            pod_name: key.pod_name.clone(),
            namespace: key.pod_namespace.clone(),
            cpu_uw,
            memory_uw: 0,
            gpu_uw,
            storage_uw: 0,
            io_uw: 0,
            total_uw,
            timestamp: SystemTime::now(),
            workload_uid: String::new(),
            workload_name: String::new(),
            workload_kind: String::new(),
            workload_category: String::new(),
            labels: HashMap::new(),
            metering_method: "kepler".into(),
        };

        nodes.entry(key.node_name.clone()).or_default().push(pod);
    }

    let total_pods = pod_data.len();

    // Ingest one AgentReport per node
    let mut agg = aggregator.write().await;
    for (node_name, pods) in nodes {
        let node_cpu = node_cpu_watts.get(&node_name).copied().unwrap_or(0.0);
        let cpu_uw = (node_cpu * 1_000_000.0) as u64;
        let gpu_uw = (node_gpu_watts.get(&node_name).copied().unwrap_or(0.0) * 1_000_000.0) as u64;
        let idle_uw = (node_idle_watts.get(&node_name).copied().unwrap_or(0.0) * 1_000_000.0) as u64;
        let platform = node_platform_watts.get(&node_name).copied();
        let platform_uw = platform.map(|w| (w * 1_000_000.0) as u64);

        let has_platform = platform.is_some();
        let total_attributed = cpu_uw + gpu_uw;
        let error_ratio = if let Some(p) = platform_uw {
            if p > 0 { (p as i64 - total_attributed as i64).unsigned_abs() as f64 / p as f64 } else { 0.0 }
        } else { 0.0 };

        let report = AgentReport {
            node: NodePowerReport {
                node_name: node_name.clone(),
                cpu_uw,
                memory_uw: 0,
                gpu_uw,
                platform_uw,
                psu_output_uw: None,
                idle_uw,
                error_ratio,
                pod_count: pods.len() as u32,
                process_count: 0,
                timestamp: SystemTime::now(),
                cpu_source: if has_platform { "kepler (RAPL + Redfish)" } else { "kepler (RAPL)" }.into(),
                memory_source: "kepler".into(),
                cpu_reading_type: "estimated".into(),
                sources: vec![],
                metering_method: "kepler".into(),
            },
            pods,
        };
        agg.ingest(report);
    }

    Ok(total_pods)
}

#[derive(Hash, Eq, PartialEq, Clone)]
struct PodKey {
    pod_id: String,
    pod_name: String,
    pod_namespace: String,
    node_name: String,
    state: String,
}

#[derive(Default)]
struct PodAccum {
    cpu_watts: f64,
    gpu_watts: f64,
}

fn parse_pod_metric(line: &str) -> Option<(PodKey, f64)> {
    let brace_start = line.find('{')?;
    let brace_end = line.find('}')?;
    let labels_str = &line[brace_start + 1..brace_end];
    let value_str = line[brace_end + 1..].trim();
    let watts: f64 = value_str.parse().ok()?;

    let labels = parse_labels(labels_str);
    let key = PodKey {
        pod_id: labels.get("pod_id")?.clone(),
        pod_name: labels.get("pod_name")?.clone(),
        pod_namespace: labels.get("pod_namespace")?.clone(),
        node_name: labels.get("node_name")?.clone(),
        state: labels.get("state")?.clone(),
    };

    Some((key, watts))
}

fn parse_node_metric(line: &str) -> Option<(String, f64)> {
    let brace_start = line.find('{')?;
    let brace_end = line.find('}')?;
    let labels_str = &line[brace_start + 1..brace_end];
    let value_str = line[brace_end + 1..].trim();
    let watts: f64 = value_str.parse().ok()?;

    let labels = parse_labels(labels_str);
    let node = labels.get("node_name")?.clone();
    Some((node, watts))
}

fn parse_labels(s: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let mut remaining = s;

    while !remaining.is_empty() {
        let eq = match remaining.find('=') {
            Some(i) => i,
            None => break,
        };
        let key = remaining[..eq].trim().trim_start_matches(',').trim();
        remaining = &remaining[eq + 1..];

        if !remaining.starts_with('"') {
            break;
        }
        remaining = &remaining[1..];

        let end_quote = match remaining.find('"') {
            Some(i) => i,
            None => break,
        };
        let value = &remaining[..end_quote];
        remaining = &remaining[end_quote + 1..];

        if remaining.starts_with(',') {
            remaining = &remaining[1..];
        }

        result.insert(key.to_string(), value.to_string());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pod_metric() {
        let line = r#"kepler_pod_cpu_watts{node_name="node-1",pod_id="abc-123",pod_name="web-1",pod_namespace="default",state="running",zone="package"} 2.5"#;
        let (key, watts) = parse_pod_metric(line).unwrap();
        assert_eq!(key.pod_id, "abc-123");
        assert_eq!(key.pod_name, "web-1");
        assert_eq!(key.pod_namespace, "default");
        assert_eq!(key.node_name, "node-1");
        assert_eq!(key.state, "running");
        assert!((watts - 2.5).abs() < 0.001);
    }

    #[test]
    fn test_parse_node_metric() {
        let line = r#"kepler_node_cpu_watts{node_name="node-1",path="/sys/class/powercap/intel-rapl:0",zone="package"} 105.3"#;
        let (node, watts) = parse_node_metric(line).unwrap();
        assert_eq!(node, "node-1");
        assert!((watts - 105.3).abs() < 0.001);
    }

    #[test]
    fn test_parse_labels() {
        let s = r#"pod_id="abc",pod_name="web",state="running""#;
        let labels = parse_labels(s);
        assert_eq!(labels.get("pod_id").unwrap(), "abc");
        assert_eq!(labels.get("pod_name").unwrap(), "web");
        assert_eq!(labels.get("state").unwrap(), "running");
    }

    #[test]
    fn test_parse_zero_value() {
        let line = r#"kepler_pod_cpu_watts{node_name="n",pod_id="p",pod_name="x",pod_namespace="ns",state="running",zone="dram"} 0"#;
        let (key, watts) = parse_pod_metric(line).unwrap();
        assert_eq!(watts, 0.0);
        assert_eq!(key.pod_namespace, "ns");
    }
}
