// SPDX-License-Identifier: Apache-2.0

//! Keck node agent — reads real hardware power data and reports to the controller.
//!
//! Pipeline:
//! 1. Discover hardware sources (RAPL, hwmon)
//! 2. Read energy counters every interval
//! 3. Compute deltas (energy consumed since last read)
//! 4. Enumerate running pods via /proc cgroup mapping
//! 5. POST report to keck-controller

mod hardware;

use std::collections::HashMap;
use std::fs;
use std::time::{Duration, SystemTime};

use log::{info, warn};
use serde::Serialize;

use hardware::{Component, PowerSource, discover_sources, procfs_root};

/// Report sent to the controller.
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

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let report_url = format!("{}/api/v1/report", controller_url);

    // Previous readings for delta computation
    let mut prev_readings: HashMap<String, u64> = HashMap::new();

    // Initial read to populate prev_readings
    for source in &sources {
        if let Ok(reading) = source.read() {
            if let Some(energy) = reading.energy_uj {
                prev_readings.insert(source.id().0.clone(), energy);
            }
        }
    }

    info!("Initial hardware read complete, entering collection loop");

    loop {
        tokio::time::sleep(interval).await;

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

            // Compute energy delta
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
                // Convert instantaneous power to energy over interval
                (power_uw as u128 * interval.as_nanos() / 1_000_000_000) as u64
            } else {
                0
            };

            // Accumulate by component
            match source.component() {
                Component::Cpu => cpu_energy_uj += delta,
                Component::Memory => mem_energy_uj += delta,
                Component::Gpu => gpu_energy_uj += delta,
                Component::Platform => platform_energy_uj = Some(delta),
                _ => {}
            }
        }

        // Convert energy (microjoules over interval) to power (microwatts)
        let interval_ns = interval.as_nanos() as u64;
        let cpu_uw = energy_to_power(cpu_energy_uj, interval_ns);
        let mem_uw = energy_to_power(mem_energy_uj, interval_ns);
        let gpu_uw = energy_to_power(gpu_energy_uj, interval_ns);
        let platform_uw = platform_energy_uj.map(|e| energy_to_power(e, interval_ns));

        // Idle estimation
        let total = cpu_uw + mem_uw + gpu_uw;
        let idle_uw = platform_uw.map(|p| p.saturating_sub(total)).unwrap_or(0);

        // Error ratio
        let error_ratio = if let Some(p) = platform_uw {
            if p > 0 {
                (p as i64 - total as i64).unsigned_abs() as f64 / p as f64
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Enumerate pods from /proc cgroups
        let pods = enumerate_pods(&node_name, cpu_uw);
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

        // POST to controller
        match http_client.post(&report_url).json(&report).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!(
                    "Reported: cpu={:.1}W mem={:.1}W gpu={:.1}W platform={} pods={}",
                    cpu_uw as f64 / 1e6,
                    mem_uw as f64 / 1e6,
                    gpu_uw as f64 / 1e6,
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

/// Enumerate pods by scanning /proc cgroups.
fn enumerate_pods(node_name: &str, total_cpu_uw: u64) -> Vec<PodPowerReport> {
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

            PodPowerReport {
                node_name: node_name.to_string(),
                pod_uid: pod_uid.clone(),
                pod_name: pod_uid.clone(),
                namespace: "default".into(),
                cpu_uw: pod_cpu_uw,
                memory_uw: 0,
                gpu_uw: 0,
                total_uw: pod_cpu_uw,
                timestamp: SystemTime::now(),
            }
        })
        .collect()
}

/// Extract pod UID from cgroup content.
fn extract_pod_uid(cgroup_content: &str) -> Option<String> {
    for line in cgroup_content.lines() {
        let path = line.rsplit(':').next().unwrap_or("");
        if let Some(pod_pos) = path.find("pod") {
            let after_pod = &path[pod_pos + 3..];
            let uid: String = after_pod
                .chars()
                .take_while(|c| *c != '/' && *c != '.')
                .collect();
            if uid.len() >= 8 {
                return Some(uid.replace('_', "-"));
            }
        }
    }
    None
}
