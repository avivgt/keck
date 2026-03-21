// SPDX-License-Identifier: Apache-2.0

//! Power metering agent — full pipeline.
//!
//! Layer 0: Hardware energy sources (RAPL, hwmon, GPU, Redfish)
//! Layer 1: eBPF kernel observation (sched_switch, cpu_frequency, perf)
//! Layer 2: Attribution engine + K8s enrichment + store + outputs
//!
//! Main loop:
//!   1. Tick hardware collector — reads sources on their tier schedule (Layer 0)
//!   2. Drain eBPF maps (Layer 1)
//!   3. Build energy input from hardware deltas (Layer 0 → Layer 2)
//!   4. Attribute energy to processes (Layer 2 engine)
//!   5. Enrich with K8s metadata (Layer 2 k8s)
//!   6. Store snapshot locally (Layer 2 store)
//!   7. Update Prometheus metrics (Layer 2 output)
//!   8. Send pod summaries upstream (Layer 2 output)

mod attribution;
mod ebpf;
mod hardware;
mod k8s;
mod output;
mod store;

use std::time::Duration;

use attribution::engine::AttributionEngine;
use ebpf::{EbpfObserver, ObserverConfig};
use hardware::{CollectorConfig, HardwareCollector};
use k8s::CgroupResolver;
use log::{error, info, warn};
use output::PrometheusExporter;
use store::{LocalStore, StoreProfile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // --- Configuration ---
    let drain_interval = Duration::from_millis(500);
    let cgroup_refresh_interval = Duration::from_secs(15);

    // --- Layer 0: Hardware Sources ---
    info!("Discovering hardware power sources...");
    let sources = hardware::discover_sources();

    if sources.is_empty() {
        error!("No power sources discovered — cannot operate without energy data");
        return Err("No power sources available".into());
    }

    let num_sockets = num_sockets();
    let num_cores = num_cpus();

    let hw_config = CollectorConfig {
        fast_interval: Duration::from_millis(100),
        medium_interval: Duration::from_millis(500),
        slow_interval: Duration::from_secs(3),
        heartbeat_interval: Duration::from_secs(5),
    };

    let mut hw_collector = HardwareCollector::new(sources, hw_config, num_sockets);
    info!(
        "Layer 0: Hardware collector initialized ({} sockets, {} cores)",
        num_sockets, num_cores
    );

    // --- Layer 1: eBPF Observer ---
    let ebpf_config = ObserverConfig {
        drain_interval,
        max_pid_entries: 65536,
    };

    let mut observer = EbpfObserver::load(ebpf_config)?;
    info!("Layer 1: eBPF programs loaded and attached");

    // --- Layer 2: Attribution Engine ---
    let mut engine = AttributionEngine::new(num_cores, num_sockets);
    info!("Layer 2: Attribution engine initialized");

    // --- Layer 2: K8s Enrichment ---
    let mut cgroup_resolver = CgroupResolver::new();
    let _ = cgroup_resolver.refresh();
    let mut last_cgroup_refresh = std::time::Instant::now();

    // --- Layer 2: Local Store ---
    let mut store = LocalStore::new(StoreProfile::standard());

    // --- Layer 2: Output ---
    let prometheus = PrometheusExporter::new(false);
    // TODO: Start HTTP server for /metrics on configurable port
    // TODO: Start gRPC client for upstream reporting to cluster controller
    // TODO: Start query server for drill-down requests

    info!(
        "Agent running — drain interval: {:?}, heartbeat: {:?}",
        drain_interval,
        Duration::from_secs(5),
    );

    // --- Main Collection Loop ---
    //
    // The loop runs at the drain_interval (500ms by default).
    // On each iteration:
    //   - Hardware collector ticks (reads sources on their tier schedule)
    //   - eBPF maps are drained
    //   - Attribution engine combines both into per-process power
    //
    // The hardware collector internally manages fast/medium/slow tiers
    // and runs reconciliation on heartbeat.
    loop {
        std::thread::sleep(drain_interval);

        // Step 1: Tick hardware collector (Layer 0)
        // This reads whichever sources are due based on their tier.
        // On heartbeat intervals, ALL sources are read and reconciled.
        hw_collector.tick();

        // Step 2: Drain eBPF maps (Layer 1)
        let observation = match observer.drain() {
            Ok(obs) => obs,
            Err(e) => {
                error!("Failed to drain eBPF maps: {}", e);
                continue;
            }
        };

        // Step 3: Build energy input from hardware deltas (Layer 0 → Layer 2)
        let energy = hw_collector.energy_input(drain_interval.as_nanos() as u64);

        // Step 4: Attribution (Layer 2 engine)
        let mut snapshot = engine.attribute(&observation, &energy);

        // Step 5: K8s enrichment
        if last_cgroup_refresh.elapsed() >= cgroup_refresh_interval {
            if let Err(e) = cgroup_resolver.refresh() {
                warn!("Failed to refresh cgroup map: {}", e);
            }
            last_cgroup_refresh = std::time::Instant::now();
        }
        k8s::enrich(&mut snapshot, &cgroup_resolver);

        // Step 6: Store locally
        store.push(snapshot.clone());

        // Step 7: Update Prometheus metrics
        prometheus.update(&snapshot);

        // Step 8: Send pod summaries upstream
        let outbox = store.drain_outbox();
        if !outbox.is_empty() {
            // TODO: Send via gRPC to cluster controller
            // On failure: store.requeue(outbox);
        }

        // Log reconciliation on heartbeats
        if let Some(recon) = hw_collector.last_reconciliation() {
            if recon.error_ratio > 0.15 {
                warn!(
                    "High reconciliation error: {:.1}% unaccounted",
                    recon.error_ratio * 100.0
                );
            }
        }

        // Self-monitoring
        let mem = store.estimated_memory();
        if mem > 100 * 1024 * 1024 {
            warn!("Store memory usage high: {}MB", mem / (1024 * 1024));
        }
    }
}

fn num_cpus() -> u32 {
    std::fs::read_to_string("/sys/devices/system/cpu/online")
        .ok()
        .and_then(|s| parse_cpu_range(&s))
        .unwrap_or(1)
}

fn num_sockets() -> u32 {
    let mut packages = std::collections::HashSet::new();
    for cpu in 0..1024u32 {
        let path = format!(
            "/sys/devices/system/cpu/cpu{}/topology/physical_package_id",
            cpu
        );
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(id) = content.trim().parse::<u32>() {
                packages.insert(id);
            }
        } else {
            break;
        }
    }
    packages.len().max(1) as u32
}

fn parse_cpu_range(s: &str) -> Option<u32> {
    let mut count = 0u32;
    for range in s.trim().split(',') {
        let parts: Vec<&str> = range.split('-').collect();
        match parts.len() {
            1 => count += 1,
            2 => {
                let start: u32 = parts[0].parse().ok()?;
                let end: u32 = parts[1].parse().ok()?;
                count += end - start + 1;
            }
            _ => return None,
        }
    }
    Some(count)
}
