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
mod perf_counters;

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
    /// Source used for CPU power
    cpu_source: String,
    /// Source used for memory power
    memory_source: String,
    /// Reading type for CPU: "measured", "estimated", or "none"
    cpu_reading_type: String,
    /// All discovered sources with their status
    sources: Vec<SourceStatus>,
}

#[derive(Serialize)]
struct SourceStatus {
    name: String,
    node_name: String,
    component: String,
    reading_type: String,
    available: bool,
    selected: bool,
    power_uw: u64,
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

    // Open per-core LLC miss counters for memory bandwidth attribution
    let mut llc_reader = match perf_counters::LlcMissReader::new() {
        Ok(reader) => {
            info!("LLC miss counters enabled — memory attribution uses PSS + LLC misses");
            Some(reader)
        }
        Err(e) => {
            info!("LLC miss counters unavailable ({}), memory attribution uses PSS only", e);
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
    let api_key = std::env::var("KECK_API_KEY").ok();
    if api_key.is_some() {
        info!("API key configured for controller authentication");
    }

    // Previous readings for delta computation
    let mut prev_readings: HashMap<String, u64> = HashMap::new();

    // Per-process caches for computing deltas
    let mut prev_cpu_ticks: HashMap<u32, u64> = HashMap::new();
    let mut prev_page_faults: HashMap<u32, (u64, u64)> = HashMap::new(); // (minflt, majflt)
    let mut pss_cache: HashMap<u32, u64> = HashMap::new(); // pid → PSS in KB (refreshed every 5 cycles)
    let mut tick_count: u64 = 0;

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
        struct ComponentReading {
            power_uw: u64,
            source: String,
            reading_type: hardware::ReadingType,
        }

        // Track ALL source readings for the sources list
        let mut all_source_readings: Vec<(String, String, String, bool, u64)> = Vec::new(); // (name, component, type, available, power)

        let mut cpu_best: Option<ComponentReading> = None;
        let mut mem_best: Option<ComponentReading> = None;
        let mut gpu_best: Option<ComponentReading> = None;
        let mut platform_reading: Option<ComponentReading> = None;
        let mut io_best: Option<ComponentReading> = None;
        let mut storage_best: Option<ComponentReading> = None;
        let mut fan_best: Option<ComponentReading> = None;

        for source in &sources {
            let reading = match source.read() {
                Ok(r) => r,
                Err(e) => {
                    log::debug!("Failed to read {}: {}", source.name(), e);
                    let type_str = match source.reading_type() {
                        hardware::ReadingType::Measured => "measured",
                        hardware::ReadingType::Estimated => "estimated",
                        hardware::ReadingType::Derived => "derived",
                    };
                    all_source_readings.push((
                        source.name().to_string(),
                        format!("{}", source.component()),
                        type_str.to_string(),
                        false,
                        0,
                    ));
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

            let type_str = match reading.reading_type {
                hardware::ReadingType::Measured => "measured",
                hardware::ReadingType::Estimated => "estimated",
                hardware::ReadingType::Derived => "derived",
            };
            let comp_str = format!("{}", source.component());
            all_source_readings.push((
                source.name().to_string(),
                comp_str,
                type_str.to_string(),
                power_uw > 0,
                power_uw,
            ));

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
                    // For Platform, prefer PSU Total (highest value = full server power)
                    // over subsystem readings (Platform Subsystem = chipset only)
                    match &platform_reading {
                        None => platform_reading = Some(cr),
                        Some(existing) => {
                            if cr.power_uw > existing.power_uw {
                                platform_reading = Some(cr);
                            }
                        }
                    }
                }
                Component::Nic => {
                    if is_better(&io_best, &cr) { io_best = Some(cr); }
                }
                Component::Storage => {
                    if is_better(&storage_best, &cr) { storage_best = Some(cr); }
                }
                Component::Fan => {
                    if is_better(&fan_best, &cr) { fan_best = Some(cr); }
                }
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

        let io_uw = io_best.as_ref().map(|r| r.power_uw).unwrap_or(0);
        let storage_uw = storage_best.as_ref().map(|r| r.power_uw).unwrap_or(0);
        let fan_uw = fan_best.as_ref().map(|r| r.power_uw).unwrap_or(0);

        let total_attributed = cpu_uw + mem_uw + gpu_uw + io_uw + storage_uw + fan_uw;
        let idle_uw = platform_uw.map(|p| p.saturating_sub(total_attributed)).unwrap_or(0);

        let error_ratio = if let Some(p) = platform_uw {
            if p > 0 {
                (p as i64 - total_attributed as i64).unsigned_abs() as f64 / p as f64
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Read LLC miss deltas for memory bandwidth attribution
        let total_llc_misses = llc_reader.as_mut().map(|r| r.total_deltas()).unwrap_or(0);

        // Drain eBPF maps
        let ebpf_snapshot = ebpf_observer.as_mut().and_then(|obs| obs.drain().ok());

        // When CPU source is RAPL (estimated), use eBPF per-core data for
        // frequency-weighted attribution. When CPU source is Redfish (measured),
        // eBPF per-core data doesn't add value — use /proc CPU time ratios.
        // Always use /proc for full pod list + PSS + memory attribution.
        // When CPU source is RAPL (estimated) and eBPF has data, override
        // CPU attribution with frequency-weighted per-core data from eBPF.
        let mut pods = enumerate_pods(&node_name, cpu_uw, mem_uw, total_llc_misses,
            &pod_cache, &mut prev_cpu_ticks, &mut prev_page_faults,
            &mut pss_cache, tick_count);

        if cpu_type == "estimated" {
            if let Some(ref snapshot) = ebpf_snapshot {
                if !snapshot.pid_cpu_times.is_empty() {
                    // Compute frequency-weighted CPU attribution from eBPF
                    let ebpf_pods = enumerate_pods_ebpf_weighted(
                        &node_name, cpu_uw, mem_uw, total_llc_misses,
                        &pod_cache, snapshot,
                    );

                    // Override CPU power for pods that eBPF saw (active pods)
                    if !ebpf_pods.is_empty() {
                        let ebpf_map: HashMap<String, u64> = ebpf_pods.iter()
                            .map(|p| (p.pod_uid.clone(), p.cpu_uw))
                            .collect();

                        for pod in &mut pods {
                            if let Some(&ebpf_cpu) = ebpf_map.get(&pod.pod_uid) {
                                pod.cpu_uw = ebpf_cpu;
                                pod.total_uw = pod.cpu_uw + pod.memory_uw + pod.gpu_uw;
                            }
                        }
                    }
                }
            }
        }

        // Add per-pod GPU power from DCGM exporter metrics
        if gpu_uw > 0 {
            add_gpu_power_to_pods(&node_name, &mut pods, &http_client, &pod_cache).await;
        }

        // Periodically clean up stale PIDs from caches and BPF maps
        tick_count += 1;
        if tick_count % 30 == 0 {
            cleanup_stale_pids(&mut prev_cpu_ticks, &mut prev_page_faults, &mut pss_cache);
            if let Some(ref mut observer) = ebpf_observer {
                match observer.cleanup_dead_pids() {
                    Ok(n) if n > 0 => info!("Cleaned {} dead PIDs from BPF PID_CGROUP map", n),
                    Err(e) => warn!("BPF PID_CGROUP cleanup failed: {}", e),
                    _ => {}
                }
            }
        }
        let pod_count = pods.len() as u32;

        // Build source status list with selection markers
        let sources_status: Vec<SourceStatus> = all_source_readings.iter().map(|(name, comp, rtype, available, power)| {
            let selected = match comp.as_str() {
                "cpu" => cpu_source == name,
                "memory" => mem_source == name,
                "gpu" => gpu_uw > 0 && *available,
                "platform" => platform_uw.is_some() && *available,
                _ => false,
            };
            SourceStatus {
                name: name.clone(),
                node_name: node_name.clone(),
                component: comp.clone(),
                reading_type: rtype.clone(),
                available: *available,
                selected,
                power_uw: *power,
            }
        }).collect();

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
                sources: sources_status,
            },
            pods,
        };

        let mut req = http_client.post(&report_url).json(&report);
        if let Some(ref key) = api_key {
            req = req.bearer_auth(key);
        }
        match req.send().await {
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

// ─── Pod enumeration via eBPF with frequency weighting (RAPL path) ──

/// Frequency-weighted per-core attribution using eBPF data.
///
/// Only used when CPU source is RAPL (estimated). When Redfish is available,
/// this function is not called.
///
/// Algorithm:
/// 1. Compute per-core weighted busy time: freq² × time_at_freq
/// 2. Split RAPL socket energy to cores proportionally
/// 3. Attribute per-core energy to PIDs by their time on each core
/// 4. Sum across cores for per-PID total
///
/// This is more accurate than flat CPU time ratios because:
/// - A process on a core at 3.5GHz gets ~3× more power than one at 1.2GHz
/// - Cross-socket attribution is correct (socket 0 energy goes only to
///   processes that ran on socket 0 cores)
fn enumerate_pods_ebpf_weighted(
    node_name: &str,
    total_cpu_uw: u64,
    total_mem_uw: u64,
    total_llc_misses: u64,
    pod_cache: &HashMap<String, PodInfo>,
    snapshot: &ebpf::EbpfSnapshot,
) -> Vec<PodPowerReport> {
    // Step 1: Compute per-core weighted busy time from frequency data
    // Weight = freq_khz² × time_ns (power ∝ V²f, V scales with f)
    let mut core_weight: HashMap<u32, f64> = HashMap::new();
    for &(cpu, freq_khz, time_ns) in &snapshot.cpu_freq_times {
        let freq_factor = (freq_khz as f64 / 1_000_000.0).powi(2);
        *core_weight.entry(cpu).or_default() += freq_factor * time_ns as f64;
    }

    // If no frequency data, fall back to equal weighting using pid_cpu_times
    if core_weight.is_empty() {
        for &(cpu, _, time_ns) in &snapshot.pid_cpu_times {
            *core_weight.entry(cpu).or_default() += time_ns as f64;
        }
    }

    let total_weight: f64 = core_weight.values().sum();
    if total_weight <= 0.0 {
        return Vec::new();
    }

    // Step 2: Compute per-core energy (split total proportionally)
    let mut core_energy: HashMap<u32, f64> = HashMap::new();
    for (&cpu, &weight) in &core_weight {
        let ratio = weight / total_weight;
        core_energy.insert(cpu, total_cpu_uw as f64 * ratio);
    }

    // Step 3: Build per-PID per-core time and map to pods
    let mut pid_pod: HashMap<u32, String> = HashMap::new();
    let mut core_pid_time: HashMap<u32, Vec<(u32, u64)>> = HashMap::new(); // core → [(pid, ns)]

    for &(cpu, pid, time_ns) in &snapshot.pid_cpu_times {
        // Resolve PID to pod UID
        if !pid_pod.contains_key(&pid) {
            let cgroup_path = format!("{}/{}/cgroup", procfs_root(), pid);
            if let Ok(content) = fs::read_to_string(&cgroup_path) {
                if let Some(uid) = extract_pod_uid(&content) {
                    pid_pod.insert(pid, uid);
                }
            }
        }

        core_pid_time.entry(cpu).or_default().push((pid, time_ns));
    }

    // Step 4: Attribute per-core energy to PIDs
    let mut pod_cpu_uw: HashMap<String, f64> = HashMap::new();

    for (&cpu, pids) in &core_pid_time {
        let ce = core_energy.get(&cpu).copied().unwrap_or(0.0);
        if ce <= 0.0 { continue; }

        let total_time: u64 = pids.iter().map(|&(_, t)| t).sum();
        if total_time == 0 { continue; }

        for &(pid, time_ns) in pids {
            if let Some(pod_uid) = pid_pod.get(&pid) {
                let ratio = time_ns as f64 / total_time as f64;
                *pod_cpu_uw.entry(pod_uid.clone()).or_default() += ce * ratio;
            }
        }
    }

    // Step 5: Get PSS for memory attribution
    // Read PSS for all pod processes
    let mut pod_pss: HashMap<String, u64> = HashMap::new();
    let mut total_pss: u64 = 0;
    for (&pid, pod_uid) in &pid_pod {
        let pss = read_pss_kb(&pid.to_string()).unwrap_or(0);
        *pod_pss.entry(pod_uid.clone()).or_default() += pss;
        total_pss += pss;
    }

    // Step 6: Build reports
    let total_pod_cpu: f64 = pod_cpu_uw.values().sum();

    // Collect all pod UIDs
    let all_uids: std::collections::HashSet<&String> = pod_cpu_uw.keys()
        .chain(pod_pss.keys())
        .collect();

    all_uids.iter().map(|pod_uid| {
        let cpu = pod_cpu_uw.get(*pod_uid).copied().unwrap_or(0.0) as u64;

        let pss = pod_pss.get(*pod_uid).copied().unwrap_or(0);
        let pss_ratio = if total_pss > 0 { pss as f64 / total_pss as f64 } else { 0.0 };
        let llc_ratio = if total_pod_cpu > 0.0 {
            pod_cpu_uw.get(*pod_uid).copied().unwrap_or(0.0) / total_pod_cpu
        } else { 0.0 };

        let has_llc = total_llc_misses > 0;
        let mem_ratio = if has_llc {
            0.6 * pss_ratio + 0.4 * llc_ratio
        } else {
            pss_ratio
        };
        let mem = (total_mem_uw as f64 * mem_ratio) as u64;

        let (pod_name, namespace) = match pod_cache.get(*pod_uid) {
            Some(info) => (info.name.clone(), info.namespace.clone()),
            None => ((*pod_uid).clone(), "unknown".into()),
        };

        PodPowerReport {
            node_name: node_name.to_string(),
            pod_uid: (*pod_uid).clone(),
            pod_name,
            namespace,
            cpu_uw: cpu,
            memory_uw: mem,
            gpu_uw: 0,
            total_uw: cpu + mem,
            timestamp: SystemTime::now(),
        }
    }).collect()
}

// ─── Pod enumeration via /proc (fallback) ────────────────────────

/// Per-process metrics read from /proc/[pid]/stat and /proc/[pid]/statm.
struct ProcessMetrics {
    pod_uid: String,
    /// CPU time in clock ticks (utime + stime)
    cpu_ticks: u64,
    /// Resident set size in pages
    rss_pages: u64,
    /// Minor page faults (memory accesses served from page cache)
    minflt_delta: u64,
    /// Major page faults (actual disk reads → memory)
    majflt_delta: u64,
}

/// Fields parsed from /proc/[pid]/stat.
struct ProcStat {
    cpu_ticks: u64,  // utime + stime
    minflt: u64,     // minor page faults (memory access from page cache)
    majflt: u64,     // major page faults (disk → memory)
}

/// Read CPU time and page fault counters from /proc/[pid]/stat.
///
/// Fields in /proc/[pid]/stat (after the closing ')'):
///   index 7:  minflt  — minor page faults
///   index 9:  majflt  — major page faults
///   index 11: utime   — user mode CPU time in clock ticks
///   index 12: stime   — kernel mode CPU time in clock ticks
fn read_proc_stat(pid_str: &str) -> Option<ProcStat> {
    let stat_path = format!("{}/{}/stat", procfs_root(), pid_str);
    let content = fs::read_to_string(&stat_path).ok()?;

    let after_comm = content.rsplit_once(')')?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();

    if fields.len() < 13 {
        return None;
    }

    let minflt: u64 = fields[7].parse().ok()?;
    let majflt: u64 = fields[9].parse().ok()?;
    let utime: u64 = fields[11].parse().ok()?;
    let stime: u64 = fields[12].parse().ok()?;

    Some(ProcStat {
        cpu_ticks: utime + stime,
        minflt,
        majflt,
    })
}

/// Backward-compatible wrapper.
fn read_cpu_ticks(pid_str: &str) -> Option<u64> {
    read_proc_stat(pid_str).map(|s| s.cpu_ticks)
}

/// Read PSS (Proportional Set Size) from /proc/[pid]/smaps_rollup.
///
/// PSS splits shared pages proportionally among all processes using them,
/// avoiding the double-counting problem of RSS.
/// Returns PSS in KB.
fn read_pss_kb(pid_str: &str) -> Option<u64> {
    let path = format!("{}/{}/smaps_rollup", procfs_root(), pid_str);
    let content = fs::read_to_string(&path).ok()?;
    for line in content.lines() {
        if line.starts_with("Pss:") {
            let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
            return Some(kb);
        }
    }
    None
}

/// Read RSS (resident set size) from /proc/[pid]/statm (fallback for PSS).
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
///   Memory power → weighted combination of:
///     - PSS (Proportional Set Size) — static DRAM power (60% weight)
///       PSS splits shared pages proportionally, avoiding double-counting
///     - Page fault delta (minflt + majflt) — dynamic DRAM access power (40% weight)
///       Processes actively accessing memory cause more DRAM power
///
/// This is significantly more accurate than process-count-based attribution
/// because a pod running at 100% CPU gets proportionally more power than
/// an idle pod, and a pod with 4GB RSS gets more memory power than one
/// with 100MB.
fn enumerate_pods(
    node_name: &str,
    total_cpu_uw: u64,
    total_mem_uw: u64,
    total_llc_misses: u64,
    pod_cache: &HashMap<String, PodInfo>,
    prev_cpu_ticks: &mut HashMap<u32, u64>,
    prev_page_faults: &mut HashMap<u32, (u64, u64)>,
    pss_cache: &mut HashMap<u32, u64>,
    tick_count: u64,
) -> Vec<PodPowerReport> {
    // Only refresh PSS every 5 cycles (~50s) — smaps_rollup is expensive
    let refresh_pss = tick_count % 5 == 0;
    // Phase 1: Scan ALL processes for CPU time and PSS.
    // Normalize against ALL processes (not just pods) for proper attribution.
    let mut process_metrics: Vec<ProcessMetrics> = Vec::new();
    let mut total_all_cpu_ticks: u64 = 0;
    let mut total_all_pss: u64 = 0;
    let mut total_all_pgfaults: u64 = 0;

    let proc_entries = match fs::read_dir(procfs_root()) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    for entry in proc_entries.filter_map(|e| e.ok()) {
        let pid_str = entry.file_name().to_string_lossy().to_string();
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        // Skip threads — only process leaders (Tgid == PID)
        let tgid_path = format!("{}/{}/status", procfs_root(), pid_str);
        if let Ok(status) = fs::read_to_string(&tgid_path) {
            if let Some(tgid_line) = status.lines().find(|l| l.starts_with("Tgid:")) {
                let tgid = tgid_line.split_whitespace().nth(1).unwrap_or("");
                if tgid != pid_str {
                    continue;
                }
            }
        }

        // Read CPU time + page faults from /proc/[pid]/stat
        let proc_stat = match read_proc_stat(&pid_str) {
            Some(s) => s,
            None => continue,
        };

        let pid: u32 = pid_str.parse().unwrap_or(0);

        // CPU time delta
        let prev_ticks = prev_cpu_ticks.get(&pid).copied().unwrap_or(proc_stat.cpu_ticks);
        let cpu_delta = proc_stat.cpu_ticks.saturating_sub(prev_ticks);
        prev_cpu_ticks.insert(pid, proc_stat.cpu_ticks);
        total_all_cpu_ticks += cpu_delta;

        // Page fault deltas
        let (prev_min, prev_maj) = prev_page_faults.get(&pid).copied()
            .unwrap_or((proc_stat.minflt, proc_stat.majflt));
        let minflt_delta = proc_stat.minflt.saturating_sub(prev_min);
        let majflt_delta = proc_stat.majflt.saturating_sub(prev_maj);
        prev_page_faults.insert(pid, (proc_stat.minflt, proc_stat.majflt));
        let pgfaults = minflt_delta + majflt_delta * 10; // Major faults weighted 10× (disk I/O)
        total_all_pgfaults += pgfaults;

        // Read PSS — only refresh from /proc every 5 cycles (smaps_rollup is expensive)
        let pss = if refresh_pss {
            let val = read_pss_kb(&pid_str).unwrap_or_else(|| {
                read_rss_pages(&pid_str).unwrap_or(0) * 4 // pages → KB
            });
            pss_cache.insert(pid, val);
            val
        } else {
            pss_cache.get(&pid).copied().unwrap_or_else(|| {
                // First time seeing this PID — must read
                let val = read_pss_kb(&pid_str).unwrap_or_else(|| {
                    read_rss_pages(&pid_str).unwrap_or(0) * 4
                });
                pss_cache.insert(pid, val);
                val
            })
        };
        total_all_pss += pss;

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
                rss_pages: pss, // Now PSS (KB), not RSS pages
                minflt_delta,
                majflt_delta,
            });
        }
    }

    if process_metrics.is_empty() {
        return Vec::new();
    }

    // Phase 2: Aggregate per-process metrics to per-pod
    struct PodMetrics {
        cpu_ticks: u64,
        pss_kb: u64,
        pgfaults: u64, // weighted: minflt + 10*majflt
    }

    let mut pod_metrics: HashMap<String, PodMetrics> = HashMap::new();

    for pm in &process_metrics {
        let entry = pod_metrics.entry(pm.pod_uid.clone()).or_insert(PodMetrics {
            cpu_ticks: 0,
            pss_kb: 0,
            pgfaults: 0,
        });
        entry.cpu_ticks += pm.cpu_ticks;
        entry.pss_kb += pm.rss_pages; // This is now PSS in KB
        entry.pgfaults += pm.minflt_delta + pm.majflt_delta * 10;
    }

    // Phase 3: Normalize against ALL processes
    let total_cpu_ticks = total_all_cpu_ticks;
    let total_pss = total_all_pss;
    let total_pgfaults = total_all_pgfaults;

    // Phase 4: Attribute power proportionally
    //
    // Memory power model:
    //   When LLC miss counters are available:
    //     60% PSS (static DRAM refresh — proportional to capacity)
    //     40% LLC misses (dynamic DRAM access — proportional to bandwidth)
    //   When LLC miss counters unavailable:
    //     100% PSS
    //
    // LLC misses are per-core totals. We attribute to processes proportionally
    // by their CPU time on each core (processes using more CPU cause more
    // cache misses). This is an approximation — true per-process LLC misses
    // would require per-PID perf events.
    let has_llc = total_llc_misses > 0;
    let pss_weight = if has_llc { 0.6 } else { 1.0 };
    let llc_weight = if has_llc { 0.4 } else { 0.0 };

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

            // Memory power: PSS (static) + LLC misses (dynamic)
            let pss_ratio = if total_pss > 0 {
                metrics.pss_kb as f64 / total_pss as f64
            } else {
                0.0
            };
            // Approximate per-pod LLC misses by CPU time ratio
            // (processes using more CPU generally cause more LLC misses)
            let llc_ratio = cpu_ratio; // proxy: LLC misses ∝ CPU activity
            let mem_ratio = pss_weight * pss_ratio + llc_weight * llc_ratio;
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
fn cleanup_stale_pids(
    prev_cpu_ticks: &mut HashMap<u32, u64>,
    prev_page_faults: &mut HashMap<u32, (u64, u64)>,
    pss_cache: &mut HashMap<u32, u64>,
) {
    let active_pids: std::collections::HashSet<u32> = fs::read_dir(procfs_root())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default();

    prev_cpu_ticks.retain(|pid, _| active_pids.contains(pid));
    prev_page_faults.retain(|pid, _| active_pids.contains(pid));
    pss_cache.retain(|pid, _| active_pids.contains(pid));
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

/// Add per-pod GPU power from DCGM exporter metrics.
///
/// DCGM metrics include pod name and namespace directly:
///   DCGM_FI_DEV_POWER_USAGE{...,namespace="default",pod="gpu-workload-xxx",...} 49.157
async fn add_gpu_power_to_pods(node_name: &str, pods: &mut Vec<PodPowerReport>, http_client: &reqwest::Client, pod_cache: &HashMap<String, PodInfo>) {
    let dcgm_url = if let Ok(url) = std::env::var("DCGM_EXPORTER_URL") {
        if url.contains(".svc") {
            hardware::gpu::discover_dcgm_url_pub().unwrap_or(url)
        } else {
            url
        }
    } else {
        match hardware::gpu::discover_dcgm_url_pub() {
            Some(url) => url,
            None => return,
        }
    };

    let resp = match http_client.get(&dcgm_url).send().await {
        Ok(r) => r,
        Err(_) => return,
    };
    let body = match resp.text().await {
        Ok(b) => b,
        Err(_) => return,
    };

    for line in body.lines() {
        if !line.starts_with("DCGM_FI_DEV_POWER_USAGE{") {
            continue;
        }

        // Filter by this node's hostname
        if let Some(h) = extract_dcgm_label(line, "Hostname") {
            if h != node_name { continue; }
        }

        let pod_name = match extract_dcgm_label(line, "pod") {
            Some(p) if !p.is_empty() => p,
            _ => continue,
        };
        let namespace = extract_dcgm_label(line, "namespace").unwrap_or_default();

        let watts: f64 = line.rsplit_once(' ')
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0.0);
        if watts <= 0.0 { continue; }

        let gpu_uw = (watts * 1_000_000.0) as u64;

        // Find matching pod and add GPU power
        if let Some(pod) = pods.iter_mut().find(|p| p.pod_name == pod_name && p.namespace == namespace) {
            pod.gpu_uw += gpu_uw;
            pod.total_uw = pod.cpu_uw + pod.memory_uw + pod.gpu_uw;
        } else {
            // Pod not in cgroup scan — look up real UID from K8s API cache
            let real_uid = pod_cache.iter()
                .find(|(_, info)| info.name == pod_name && info.namespace == namespace)
                .map(|(uid, _)| uid.clone());

            if let Some(uid) = real_uid {
                pods.push(PodPowerReport {
                    node_name: node_name.to_string(),
                    pod_uid: uid,
                    pod_name,
                    namespace,
                    cpu_uw: 0,
                    memory_uw: 0,
                    gpu_uw: gpu_uw,
                    total_uw: gpu_uw,
                    timestamp: SystemTime::now(),
                });
            } else {
                log::debug!("GPU pod {}/{} not in K8s cache — skipping", namespace, pod_name);
            }
        }
    }
}

/// Extract a label from a Prometheus DCGM metric line.
fn extract_dcgm_label(line: &str, label: &str) -> Option<String> {
    let pattern = format!("{}=\"", label);
    let start = line.find(&pattern)? + pattern.len();
    let end = start + line[start..].find('"')?;
    Some(line[start..end].to_string())
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
