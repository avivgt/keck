// SPDX-License-Identifier: Apache-2.0

//! Keck node agent — reads real hardware power data and reports to the controller.
//!
//! Pipeline:
//! 1. Discover hardware sources (RAPL, hwmon)
//! 2. Query K8s API for pods on this node (name, namespace, UID)
//! 3. Read energy counters every interval
//! 4. Compute deltas (energy consumed since last read)
//! 5. Map processes to pods via /proc cgroup → pod UID → K8s metadata
//! 6. POST report to keck-controller

mod hardware;

use std::collections::HashMap;
use std::fs;
use std::time::{Duration, SystemTime};

use log::{info, warn};
use serde::{Deserialize, Serialize};

use hardware::{Component, PowerSource, discover_sources, procfs_root};

// ─── Wire types ──────────────────────────────────────────────────

#[derive(Serialize)]
struct AgentReport {
    node: NodePowerReport,
    pods: Vec<PodPowerReport>,
}

#[derive(Serialize)]
struct NodePowerReport {
    node_name: String,
    cpu_uw: u64,
    memory_uw: u64,
    gpu_uw: u64,
    platform_uw: Option<u64>,
    idle_uw: u64,
    error_ratio: f64,
    pod_count: u32,
    process_count: u32,
    timestamp: SystemTime,
}

#[derive(Serialize)]
struct PodPowerReport {
    node_name: String,
    pod_uid: String,
    pod_name: String,
    namespace: String,
    cpu_uw: u64,
    memory_uw: u64,
    gpu_uw: u64,
    total_uw: u64,
    timestamp: SystemTime,
}

// ─── K8s API types (partial) ─────────────────────────────────────

#[derive(Deserialize)]
struct PodList {
    items: Vec<Pod>,
}

#[derive(Deserialize)]
struct Pod {
    metadata: PodMetadata,
}

#[derive(Deserialize)]
struct PodMetadata {
    name: String,
    namespace: String,
    uid: String,
}

/// Pod info resolved from the K8s API.
struct PodInfo {
    name: String,
    namespace: String,
}

// ─── Main ────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    let node_name = get_node_name();
    let controller_url = std::env::var("KECK_CONTROLLER_URL")
        .unwrap_or_else(|_| "http://keck-controller.keck-system.svc:8080".into());
    let interval = Duration::from_secs(
        std::env::var("KECK_INTERVAL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10),
    );

    info!("Keck agent starting");
    info!("  Node: {}", node_name);
    info!("  Controller: {}", controller_url);
    info!("  Interval: {:?}", interval);

    // Discover hardware power sources
    let sources = discover_sources();
    info!("Discovered {} power source(s)", sources.len());

    if sources.is_empty() {
        warn!("No power sources found — agent will report zero power");
    }

    // Build K8s API client using in-cluster service account
    let k8s_client = build_k8s_client();

    // HTTP client for reporting to controller
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let report_url = format!("{}/api/v1/report", controller_url);

    // Previous readings for delta computation
    let mut prev_readings: HashMap<String, u64> = HashMap::new();

    // Pod metadata cache: uid → (name, namespace)
    let mut pod_cache: HashMap<String, PodInfo> = HashMap::new();
    let mut last_pod_refresh = std::time::Instant::now();
    let pod_refresh_interval = Duration::from_secs(30);

    // Initial read to populate prev_readings
    for source in &sources {
        if let Ok(reading) = source.read() {
            if let Some(energy) = reading.energy_uj {
                prev_readings.insert(source.id().0.clone(), energy);
            }
        }
    }

    // Initial pod cache
    refresh_pod_cache(&k8s_client, &node_name, &mut pod_cache).await;
    info!("Cached {} pods from K8s API", pod_cache.len());

    info!("Initial hardware read complete, entering collection loop");

    loop {
        tokio::time::sleep(interval).await;

        // Refresh pod cache periodically
        if last_pod_refresh.elapsed() >= pod_refresh_interval {
            refresh_pod_cache(&k8s_client, &node_name, &mut pod_cache).await;
            last_pod_refresh = std::time::Instant::now();
        }

        // Read all sources and compute deltas
        let mut cpu_energy_uj: u64 = 0;
        let mut mem_energy_uj: u64 = 0;
        let mut gpu_energy_uj: u64 = 0;
        let mut platform_energy_uj: Option<u64> = None;

        for source in &sources {
            let reading = match source.read() {
                Ok(r) => r,
                Err(e) => {
                    log::debug!("Failed to read {}: {}", source.name(), e);
                    continue;
                }
            };

            let delta = if let Some(current_energy) = reading.energy_uj {
                let prev = prev_readings
                    .get(&source.id().0)
                    .copied()
                    .unwrap_or(current_energy);

                let delta = if current_energy >= prev {
                    current_energy - prev
                } else if reading.max_energy_uj > 0 {
                    (reading.max_energy_uj - prev) + current_energy
                } else {
                    current_energy
                };

                prev_readings.insert(source.id().0.clone(), current_energy);
                delta
            } else if let Some(power_uw) = reading.power_uw {
                (power_uw as u128 * interval.as_nanos() / 1_000_000_000) as u64
            } else {
                0
            };

            match source.component() {
                Component::Cpu => cpu_energy_uj += delta,
                Component::Memory => mem_energy_uj += delta,
                Component::Gpu => gpu_energy_uj += delta,
                Component::Platform => platform_energy_uj = Some(delta),
                _ => {}
            }
        }

        // Convert to power
        let interval_ns = interval.as_nanos() as u64;
        let cpu_uw = energy_to_power(cpu_energy_uj, interval_ns);
        let mem_uw = energy_to_power(mem_energy_uj, interval_ns);
        let gpu_uw = energy_to_power(gpu_energy_uj, interval_ns);
        let platform_uw = platform_energy_uj.map(|e| energy_to_power(e, interval_ns));

        let total = cpu_uw + mem_uw + gpu_uw;
        let idle_uw = platform_uw.map(|p| p.saturating_sub(total)).unwrap_or(0);

        let error_ratio = if let Some(p) = platform_uw {
            if p > 0 {
                (p as i64 - total as i64).unsigned_abs() as f64 / p as f64
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Enumerate pods with real K8s metadata
        let pods = enumerate_pods(&node_name, cpu_uw, mem_uw, &pod_cache);
        let pod_count = pods.len() as u32;

        let report = AgentReport {
            node: NodePowerReport {
                node_name: node_name.clone(),
                cpu_uw,
                memory_uw: mem_uw,
                gpu_uw,
                platform_uw,
                idle_uw,
                error_ratio,
                pod_count,
                process_count: count_processes(),
                timestamp: SystemTime::now(),
            },
            pods,
        };

        match http_client.post(&report_url).json(&report).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!(
                    "Reported: cpu={:.1}W mem={:.1}W platform={} pods={} namespaces={}",
                    cpu_uw as f64 / 1e6,
                    mem_uw as f64 / 1e6,
                    platform_uw
                        .map(|p| format!("{:.1}W", p as f64 / 1e6))
                        .unwrap_or("N/A".into()),
                    pod_count,
                    count_unique_namespaces(&report.node.node_name, &pod_cache),
                );
            }
            Ok(resp) => warn!("Controller returned {}", resp.status()),
            Err(e) => warn!("Failed to report to controller: {}", e),
        }
    }
}

// ─── K8s API ─────────────────────────────────────────────────────

/// Build an HTTP client for in-cluster K8s API access.
fn build_k8s_client() -> reqwest::Client {
    // Read the service account CA cert
    let ca_cert = fs::read("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt")
        .ok()
        .and_then(|pem| reqwest::Certificate::from_pem(&pem).ok());

    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(10));

    if let Some(cert) = ca_cert {
        builder = builder.add_root_certificate(cert);
    } else {
        // Fallback: skip TLS verification (dev/test only)
        builder = builder.danger_accept_invalid_certs(true);
    }

    builder.build().unwrap_or_else(|_| reqwest::Client::new())
}

/// Get the service account token for K8s API auth.
fn k8s_token() -> Option<String> {
    fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/token").ok()
}

/// Refresh the pod metadata cache from the K8s API.
async fn refresh_pod_cache(
    client: &reqwest::Client,
    node_name: &str,
    cache: &mut HashMap<String, PodInfo>,
) {
    let token = match k8s_token() {
        Some(t) => t,
        None => {
            log::debug!("No K8s service account token found");
            return;
        }
    };

    let url = format!(
        "https://kubernetes.default.svc/api/v1/pods?fieldSelector=spec.nodeName={}",
        node_name,
    );

    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to query K8s API for pods: {}", e);
            return;
        }
    };

    if !resp.status().is_success() {
        warn!("K8s API returned {}", resp.status());
        return;
    }

    let pod_list: PodList = match resp.json().await {
        Ok(pl) => pl,
        Err(e) => {
            warn!("Failed to parse K8s pod list: {}", e);
            return;
        }
    };

    cache.clear();
    for pod in pod_list.items {
        cache.insert(
            pod.metadata.uid.clone(),
            PodInfo {
                name: pod.metadata.name,
                namespace: pod.metadata.namespace,
            },
        );
    }

    log::debug!("Refreshed pod cache: {} pods on node {}", cache.len(), node_name);
}

fn count_unique_namespaces(_node: &str, cache: &HashMap<String, PodInfo>) -> usize {
    let ns: std::collections::HashSet<&str> = cache.values().map(|p| p.namespace.as_str()).collect();
    ns.len()
}

// ─── Pod enumeration ─────────────────────────────────────────────

/// Enumerate pods by scanning /proc cgroups and resolving via K8s API cache.
fn enumerate_pods(
    node_name: &str,
    total_cpu_uw: u64,
    total_mem_uw: u64,
    pod_cache: &HashMap<String, PodInfo>,
) -> Vec<PodPowerReport> {
    let mut pod_process_count: HashMap<String, u32> = HashMap::new();

    let proc_entries = match fs::read_dir(procfs_root()) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    for entry in proc_entries.filter_map(|e| e.ok()) {
        let pid_str = entry.file_name().to_string_lossy().to_string();
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let cgroup_path = format!("{}/{}/cgroup", procfs_root(), pid_str);
        let cgroup_content = match fs::read_to_string(&cgroup_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if let Some(pod_uid) = extract_pod_uid(&cgroup_content) {
            *pod_process_count.entry(pod_uid).or_default() += 1;
        }
    }

    if pod_process_count.is_empty() {
        return Vec::new();
    }

    let total_processes: u32 = pod_process_count.values().sum();

    pod_process_count
        .iter()
        .map(|(pod_uid, count)| {
            let ratio = if total_processes > 0 {
                *count as f64 / total_processes as f64
            } else {
                0.0
            };
            let pod_cpu_uw = (total_cpu_uw as f64 * ratio) as u64;
            let pod_mem_uw = (total_mem_uw as f64 * ratio) as u64;

            // Resolve pod name and namespace from K8s API cache
            let (pod_name, namespace) = match pod_cache.get(pod_uid) {
                Some(info) => (info.name.clone(), info.namespace.clone()),
                None => (pod_uid.clone(), "unknown".into()),
            };

            PodPowerReport {
                node_name: node_name.to_string(),
                pod_uid: pod_uid.clone(),
                pod_name,
                namespace,
                cpu_uw: pod_cpu_uw,
                memory_uw: pod_mem_uw,
                gpu_uw: 0,
                total_uw: pod_cpu_uw + pod_mem_uw,
                timestamp: SystemTime::now(),
            }
        })
        .collect()
}

// ─── Helpers ─────────────────────────────────────────────────────

fn energy_to_power(energy_uj: u64, interval_ns: u64) -> u64 {
    if interval_ns == 0 { return 0; }
    ((energy_uj as u128 * 1_000_000_000) / interval_ns as u128) as u64
}

fn get_node_name() -> String {
    std::env::var("NODE_NAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()))
        .unwrap_or_else(|_| "unknown".into())
}

fn count_processes() -> u32 {
    fs::read_dir(procfs_root())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .map(|s| s.chars().all(|c| c.is_ascii_digit()))
                        .unwrap_or(false)
                })
                .count() as u32
        })
        .unwrap_or(0)
}

/// Extract pod UID from cgroup v2 content.
///
/// OpenShift format:
///   0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod<UID>.slice/crio-...
fn extract_pod_uid(cgroup_content: &str) -> Option<String> {
    for line in cgroup_content.lines() {
        let path = line.rsplit(':').next().unwrap_or("");
        if let Some(pos) = path.find("-pod") {
            let after = &path[pos + 4..];
            let uid: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if uid.len() >= 8 {
                return Some(uid.replace('_', "-"));
            }
        }
    }
    None
}
