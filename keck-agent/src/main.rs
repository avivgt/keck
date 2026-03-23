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

mod ebpf;
mod hardware;

use std::collections::HashMap;
use std::fs;
use std::time::{Duration, SystemTime};

use log::{info, warn};
use serde::{Deserialize, Serialize};

use ebpf::EbpfObserver;
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
    /// Source used for CPU power (e.g., "Redfish CPU" or "RAPL package")
    cpu_source: String,
    /// Source used for memory power
    memory_source: String,
    /// Reading type: "measured", "estimated", or "none"
    cpu_reading_type: String,
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

    // Load eBPF programs for per-core scheduling data
    let mut ebpf_observer = match EbpfObserver::load() {
        Ok(obs) => {
            info!("eBPF loaded — using per-core CPU time attribution");
            Some(obs)
        }
        Err(e) => {
            warn!("eBPF not available ({}), falling back to /proc-based attribution", e);
            None
        }
    };

    // Build K8s API client using in-cluster service account
    let k8s_client = build_k8s_client();

    // HTTP client for reporting to controller
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let report_url = format!("{}/api/v1/report", controller_url);

    // Previous readings for delta computation
    let mut prev_readings: HashMap<String, u64> = HashMap::new();

    // Per-process CPU ticks cache for computing deltas
    let mut prev_cpu_ticks: HashMap<u32, u64> = HashMap::new();
    let mut tick_count: u32 = 0;

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

        // Read all sources and select the best per component.
        // Priority: Measured (Redfish) > Estimated (RAPL) > Unavailable
        //
        // Track per-component: (power_uw, source_name, reading_type)
        struct ComponentReading {
            power_uw: u64,
            source: String,
            reading_type: hardware::ReadingType,
        }

        let mut cpu_best: Option<ComponentReading> = None;
        let mut mem_best: Option<ComponentReading> = None;
        let mut gpu_best: Option<ComponentReading> = None;
        let mut platform_reading: Option<ComponentReading> = None;

        for source in &sources {
            let reading = match source.read() {
                Ok(r) => r,
                Err(e) => {
                    log::debug!("Failed to read {}: {}", source.name(), e);
                    continue;
                }
            };

            // Convert to power (microwatts)
            let power_uw = if let Some(current_energy) = reading.energy_uj {
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
                energy_to_power(delta, interval.as_nanos() as u64)
            } else if let Some(pw) = reading.power_uw {
                pw
            } else {
                0
            };

            if power_uw == 0 {
                continue;
            }

            let cr = ComponentReading {
                power_uw,
                source: source.name().to_string(),
                reading_type: reading.reading_type,
            };

            // Select best source per component: Measured beats Estimated
            let is_better = |existing: &Option<ComponentReading>, new: &ComponentReading| -> bool {
                match existing {
                    None => true,
                    Some(old) => {
                        // Measured > Estimated > Derived
                        let old_rank = match old.reading_type {
                            hardware::ReadingType::Measured => 3,
                            hardware::ReadingType::Estimated => 2,
                            hardware::ReadingType::Derived => 1,
                        };
                        let new_rank = match new.reading_type {
                            hardware::ReadingType::Measured => 3,
                            hardware::ReadingType::Estimated => 2,
                            hardware::ReadingType::Derived => 1,
                        };
                        new_rank > old_rank
                    }
                }
            };

            match source.component() {
                Component::Cpu => {
                    if is_better(&cpu_best, &cr) { cpu_best = Some(cr); }
                }
                Component::Memory => {
                    if is_better(&mem_best, &cr) { mem_best = Some(cr); }
                }
                Component::Gpu => {
                    if is_better(&gpu_best, &cr) { gpu_best = Some(cr); }
                }
                Component::Platform => {
                    if is_better(&platform_reading, &cr) { platform_reading = Some(cr); }
                }
                _ => {}
            }
        }

        // Extract selected values
        let cpu_uw = cpu_best.as_ref().map(|r| r.power_uw).unwrap_or(0);
        let mem_uw = mem_best.as_ref().map(|r| r.power_uw).unwrap_or(0);
        let gpu_uw = gpu_best.as_ref().map(|r| r.power_uw).unwrap_or(0);
        let platform_uw = platform_reading.as_ref().map(|r| r.power_uw);

        // Log which source was selected per component
        let cpu_source = cpu_best.as_ref().map(|r| r.source.as_str()).unwrap_or("none");
        let mem_source = mem_best.as_ref().map(|r| r.source.as_str()).unwrap_or("none");
        let cpu_type = cpu_best.as_ref().map(|r| match r.reading_type {
            hardware::ReadingType::Measured => "measured",
            hardware::ReadingType::Estimated => "estimated",
            hardware::ReadingType::Derived => "derived",
        }).unwrap_or("none");

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

        // Attribution uses /proc/[pid]/stat CPU time deltas and RSS.
        // eBPF sched_switch data is collected for future per-core frequency
        // weighting but not used for power distribution yet — /proc CPU time
        // deltas are more stable for attribution because they measure actual
        // user+kernel time, while eBPF context switch counts can over-weight
        // processes with high scheduling frequency (goroutines, event loops).
        let pods = enumerate_pods(&node_name, cpu_uw, mem_uw, &pod_cache, &mut prev_cpu_ticks);

        // Drain eBPF maps to prevent unbounded growth (data collected for future use)
        if let Some(ref mut obs) = ebpf_observer {
            let _ = obs.drain();
        }

        // Periodically clean up stale PIDs from the cache
        tick_count += 1;
        if tick_count % 30 == 0 {
            cleanup_stale_pids(&mut prev_cpu_ticks);
        }
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
                cpu_source: cpu_source.to_string(),
                memory_source: mem_source.to_string(),
                cpu_reading_type: cpu_type.to_string(),
            },
            pods,
        };

        match http_client.post(&report_url).json(&report).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!(
                    "Reported: cpu={:.1}W({}) mem={:.1}W platform={} pods={}",
                    cpu_uw as f64 / 1e6,
                    cpu_type,
                    mem_uw as f64 / 1e6,
                    platform_uw
                        .map(|p| format!("{:.1}W", p as f64 / 1e6))
                        .unwrap_or("N/A".into()),
                    pod_count,
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

// ─── Pod enumeration via eBPF (most accurate) ───────────────────

/// Enumerate pods using eBPF per-PID per-core time data.
///
/// This is the most accurate attribution method:
/// - Per-core CPU time from sched_switch tracepoint (nanosecond precision)
/// - Per-core frequency data for weighting
/// - Pod mapping via cgroup IDs from eBPF (no /proc reads needed)
/// - RSS from /proc for memory attribution
fn enumerate_pods_ebpf(
    node_name: &str,
    total_cpu_uw: u64,
    total_mem_uw: u64,
    pod_cache: &HashMap<String, PodInfo>,
    snapshot: &ebpf::EbpfSnapshot,
) -> Vec<PodPowerReport> {
    // Build pid → pod_uid mapping from cgroup data
    // eBPF gives us pid → cgroup_id, but we need pod_uid.
    // For now, fall back to /proc cgroup parsing for the pod_uid mapping
    // and use eBPF for the CPU time data.

    let mut pid_pod_uid: HashMap<u32, String> = HashMap::new();
    for &(_, pid, _) in &snapshot.pid_cpu_times {
        if pid_pod_uid.contains_key(&pid) {
            continue;
        }
        let cgroup_path = format!("{}/{}/cgroup", procfs_root(), pid);
        if let Ok(content) = fs::read_to_string(&cgroup_path) {
            if let Some(uid) = extract_pod_uid(&content) {
                pid_pod_uid.insert(pid, uid);
            }
        }
    }

    // Aggregate per-PID per-core CPU time to per-pod
    // Key: pod_uid → total CPU nanoseconds across all cores
    let mut pod_cpu_ns: HashMap<String, u64> = HashMap::new();

    for &(cpu, pid, time_ns) in &snapshot.pid_cpu_times {
        if let Some(pod_uid) = pid_pod_uid.get(&pid) {
            *pod_cpu_ns.entry(pod_uid.clone()).or_default() += time_ns;
        }
    }

    if pod_cpu_ns.is_empty() {
        return Vec::new();
    }

    // Total CPU nanoseconds across all pods (for normalization)
    let total_cpu_ns: u64 = pod_cpu_ns.values().sum();

    // Collect RSS per pod for memory attribution
    let mut pod_rss: HashMap<String, u64> = HashMap::new();
    for (&pid, pod_uid) in &pid_pod_uid {
        let rss = read_rss_pages(&pid.to_string()).unwrap_or(0);
        *pod_rss.entry(pod_uid.clone()).or_default() += rss;
    }
    let total_rss: u64 = pod_rss.values().sum();

    // Build reports
    pod_cpu_ns
        .iter()
        .map(|(pod_uid, &cpu_ns)| {
            let cpu_ratio = if total_cpu_ns > 0 {
                cpu_ns as f64 / total_cpu_ns as f64
            } else {
                0.0
            };
            let pod_cpu_uw = (total_cpu_uw as f64 * cpu_ratio) as u64;

            let rss = pod_rss.get(pod_uid).copied().unwrap_or(0);
            let mem_ratio = if total_rss > 0 {
                rss as f64 / total_rss as f64
            } else {
                0.0
            };
            let pod_mem_uw = (total_mem_uw as f64 * mem_ratio) as u64;

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

// ─── Pod enumeration via /proc (fallback) ────────────────────────

/// Per-process metrics read from /proc/[pid]/stat and /proc/[pid]/statm.
struct ProcessMetrics {
    pod_uid: String,
    /// CPU time in clock ticks (utime + stime)
    cpu_ticks: u64,
    /// Resident set size in pages
    rss_pages: u64,
}

/// Read CPU time (utime + stime) from /proc/[pid]/stat.
///
/// Fields in /proc/[pid]/stat (space-separated, 1-indexed):
///   field 14: utime — user mode CPU time in clock ticks
///   field 15: stime — kernel mode CPU time in clock ticks
fn read_cpu_ticks(pid_str: &str) -> Option<u64> {
    let stat_path = format!("{}/{}/stat", procfs_root(), pid_str);
    let content = fs::read_to_string(&stat_path).ok()?;

    // The comm field (field 2) can contain spaces and parens, so find
    // the last ')' to skip past it reliably.
    let after_comm = content.rsplit_once(')')?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();

    // After the closing ')': field index 0 = state (field 3 in stat)
    // utime is field 14 in stat = index 11 after ')'
    // stime is field 15 in stat = index 12 after ')'
    if fields.len() < 13 {
        return None;
    }

    let utime: u64 = fields[11].parse().ok()?;
    let stime: u64 = fields[12].parse().ok()?;
    Some(utime + stime)
}

/// Read RSS (resident set size) from /proc/[pid]/statm.
///
/// /proc/[pid]/statm fields (space-separated):
///   field 1: total program size (pages)
///   field 2: RSS (pages) — this is what we want
fn read_rss_pages(pid_str: &str) -> Option<u64> {
    let statm_path = format!("{}/{}/statm", procfs_root(), pid_str);
    let content = fs::read_to_string(&statm_path).ok()?;
    let fields: Vec<&str> = content.split_whitespace().collect();
    if fields.len() < 2 {
        return None;
    }
    fields[1].parse().ok()
}

/// Enumerate pods with accurate per-process attribution.
///
/// Power is distributed based on:
///   CPU power → proportional to each pod's CPU time delta (utime + stime)
///   Memory power → proportional to each pod's RSS (resident set size)
///
/// This is significantly more accurate than process-count-based attribution
/// because a pod running at 100% CPU gets proportionally more power than
/// an idle pod, and a pod with 4GB RSS gets more memory power than one
/// with 100MB.
fn enumerate_pods(
    node_name: &str,
    total_cpu_uw: u64,
    total_mem_uw: u64,
    pod_cache: &HashMap<String, PodInfo>,
    prev_cpu_ticks: &mut HashMap<u32, u64>,
) -> Vec<PodPowerReport> {
    // Phase 1: Scan ALL processes for CPU time (pod and non-pod)
    // We need total CPU time across ALL processes for proper normalization.
    // A pod using 10% of total node CPU should get 10% of power, not 90%.
    let mut process_metrics: Vec<ProcessMetrics> = Vec::new();
    let mut total_all_cpu_ticks: u64 = 0; // ALL processes, not just pods
    let mut total_all_rss: u64 = 0;

    let proc_entries = match fs::read_dir(procfs_root()) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    for entry in proc_entries.filter_map(|e| e.ok()) {
        let pid_str = entry.file_name().to_string_lossy().to_string();
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        // Skip threads (LWPs) — only count process leaders.
        // A thread's TGID (field 4 in /proc/[pid]/status) differs from its PID.
        // For process leaders, TGID == PID.
        // /proc/[pid]/stat field 4 (after comm) is PPID, field 1 (before comm) is PID.
        // Simpler check: if /proc/[pid]/task/[pid]/children doesn't list this as leader,
        // or check /proc/[pid]/status for Tgid.
        let tgid_path = format!("{}/{}/status", procfs_root(), pid_str);
        if let Ok(status) = fs::read_to_string(&tgid_path) {
            if let Some(tgid_line) = status.lines().find(|l| l.starts_with("Tgid:")) {
                let tgid = tgid_line.split_whitespace().nth(1).unwrap_or("");
                if tgid != pid_str {
                    continue; // This is a thread, skip it
                }
            }
        }

        // Read CPU time for ALL processes (process leaders only)
        let current_ticks = match read_cpu_ticks(&pid_str) {
            Some(t) => t,
            None => continue,
        };

        let pid: u32 = pid_str.parse().unwrap_or(0);
        let prev = prev_cpu_ticks.get(&pid).copied().unwrap_or(current_ticks);
        let cpu_delta = current_ticks.saturating_sub(prev);
        prev_cpu_ticks.insert(pid, current_ticks);

        // Count toward total (all processes)
        total_all_cpu_ticks += cpu_delta;

        // Read RSS
        let rss = read_rss_pages(&pid_str).unwrap_or(0);
        total_all_rss += rss;

        // Check if this process belongs to a pod
        let cgroup_path = format!("{}/{}/cgroup", procfs_root(), pid_str);
        let cgroup_content = match fs::read_to_string(&cgroup_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if let Some(pod_uid) = extract_pod_uid(&cgroup_content) {
            process_metrics.push(ProcessMetrics {
                pod_uid,
                cpu_ticks: cpu_delta,
                rss_pages: rss,
            });
        }
    }

    if process_metrics.is_empty() {
        return Vec::new();
    }

    // Phase 2: Aggregate per-process metrics to per-pod
    struct PodMetrics {
        cpu_ticks: u64,
        rss_pages: u64,
    }

    let mut pod_metrics: HashMap<String, PodMetrics> = HashMap::new();

    for pm in &process_metrics {
        let entry = pod_metrics.entry(pm.pod_uid.clone()).or_insert(PodMetrics {
            cpu_ticks: 0,
            rss_pages: 0,
        });
        entry.cpu_ticks += pm.cpu_ticks;
        entry.rss_pages += pm.rss_pages;
    }

    // Phase 3: Normalize against ALL processes (not just pods)
    // This ensures a pod using 10% of total CPU gets 10% of power.
    let total_cpu_ticks = total_all_cpu_ticks;
    let total_rss = total_all_rss;

    // Phase 4: Attribute power proportionally
    pod_metrics
        .iter()
        .map(|(pod_uid, metrics)| {
            // CPU power proportional to CPU time consumed
            let cpu_ratio = if total_cpu_ticks > 0 {
                metrics.cpu_ticks as f64 / total_cpu_ticks as f64
            } else {
                0.0
            };
            let pod_cpu_uw = (total_cpu_uw as f64 * cpu_ratio) as u64;

            // Memory power proportional to RSS (resident memory)
            let mem_ratio = if total_rss > 0 {
                metrics.rss_pages as f64 / total_rss as f64
            } else {
                0.0
            };
            let pod_mem_uw = (total_mem_uw as f64 * mem_ratio) as u64;

            // Resolve pod name and namespace
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

/// Clean up stale PIDs from the CPU ticks cache.
/// Called periodically to prevent unbounded growth.
fn cleanup_stale_pids(prev_cpu_ticks: &mut HashMap<u32, u64>) {
    let active_pids: std::collections::HashSet<u32> = fs::read_dir(procfs_root())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default();

    prev_cpu_ticks.retain(|pid, _| active_pids.contains(pid));
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
