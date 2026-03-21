// SPDX-License-Identifier: Apache-2.0

//! Tiered polling collector with heartbeat reconciliation.
//!
//! Manages all discovered power sources with configurable polling tiers:
//! - Fast tier (100ms): RAPL, cpufreq — high-resolution energy counters
//! - Medium tier (500ms): GPU — moderate latency APIs
//! - Slow tier (2-5s): Redfish/IPMI — HTTP/IPMI roundtrip
//!
//! At every heartbeat interval, ALL sources are read simultaneously
//! for reconciliation: PSU input vs sum of components.
//!
//! Between heartbeats, fast-tier data provides high-resolution energy
//! deltas to the attribution engine.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::{Component, PowerReading, PowerSource, SourceId};

/// Configuration for the hardware collector.
pub struct CollectorConfig {
    /// Fast tier interval (RAPL, cpufreq)
    pub fast_interval: Duration,
    /// Medium tier interval (GPU)
    pub medium_interval: Duration,
    /// Slow tier interval (Redfish, IPMI)
    pub slow_interval: Duration,
    /// Heartbeat: all sources read together for reconciliation.
    /// Must be >= slow_interval.
    pub heartbeat_interval: Duration,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            fast_interval: Duration::from_millis(100),
            medium_interval: Duration::from_millis(500),
            slow_interval: Duration::from_secs(3),
            heartbeat_interval: Duration::from_secs(5),
        }
    }
}

/// Polling tier assignment for a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    Fast,
    Medium,
    Slow,
}

/// A registered source with its tier and last reading.
struct RegisteredSource {
    source: Box<dyn PowerSource>,
    tier: Tier,
    last_reading: Option<PowerReading>,
    prev_reading: Option<PowerReading>,
}

/// Reconciliation result: compares component sum against platform measurement.
#[derive(Clone, Debug)]
pub struct Reconciliation {
    /// Platform (PSU) measured power in microwatts
    pub platform_uw: Option<u64>,
    /// Sum of all component power readings in microwatts
    pub component_sum_uw: u64,
    /// Per-component power breakdown in microwatts
    pub per_component: HashMap<Component, u64>,
    /// Unaccounted: platform - component_sum (positive = underreporting)
    pub unaccounted_uw: i64,
    /// Error ratio: |unaccounted| / platform
    pub error_ratio: f64,
    /// Timestamp of the reconciliation
    pub timestamp: Instant,
}

/// The hardware collector manages all power sources and provides
/// energy deltas to the attribution engine.
pub struct HardwareCollector {
    sources: Vec<RegisteredSource>,
    config: CollectorConfig,
    last_heartbeat: Instant,
    last_fast: Instant,
    last_medium: Instant,
    last_slow: Instant,
    last_reconciliation: Option<Reconciliation>,
    num_sockets: u32,
}

impl HardwareCollector {
    /// Create a new collector with discovered sources.
    pub fn new(
        sources: Vec<Box<dyn PowerSource>>,
        config: CollectorConfig,
        num_sockets: u32,
    ) -> Self {
        let now = Instant::now();

        let registered: Vec<RegisteredSource> = sources
            .into_iter()
            .map(|source| {
                let tier = classify_tier(&source, &config);
                RegisteredSource {
                    source,
                    tier,
                    last_reading: None,
                    prev_reading: None,
                }
            })
            .collect();

        log::info!(
            "Hardware collector: {} sources ({} fast, {} medium, {} slow)",
            registered.len(),
            registered.iter().filter(|s| s.tier == Tier::Fast).count(),
            registered.iter().filter(|s| s.tier == Tier::Medium).count(),
            registered.iter().filter(|s| s.tier == Tier::Slow).count(),
        );

        Self {
            sources: registered,
            config,
            last_heartbeat: now,
            last_fast: now,
            last_medium: now,
            last_slow: now,
            last_reconciliation: None,
            num_sockets,
        }
    }

    /// Tick the collector: read sources that are due based on their tier.
    ///
    /// Call this from the main loop at the fast-tier interval.
    /// Returns true if new data is available.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let mut any_read = false;

        // Check if heartbeat is due (all sources)
        if now.duration_since(self.last_heartbeat) >= self.config.heartbeat_interval {
            self.read_all();
            self.reconcile();
            self.last_heartbeat = now;
            self.last_fast = now;
            self.last_medium = now;
            self.last_slow = now;
            return true;
        }

        // Fast tier
        if now.duration_since(self.last_fast) >= self.config.fast_interval {
            any_read |= self.read_tier(Tier::Fast);
            self.last_fast = now;
        }

        // Medium tier
        if now.duration_since(self.last_medium) >= self.config.medium_interval {
            any_read |= self.read_tier(Tier::Medium);
            self.last_medium = now;
        }

        // Slow tier
        if now.duration_since(self.last_slow) >= self.config.slow_interval {
            any_read |= self.read_tier(Tier::Slow);
            self.last_slow = now;
        }

        any_read
    }

    /// Build an EnergyInput for the attribution engine from the latest readings.
    ///
    /// Computes energy deltas from consecutive readings (handling wraparound).
    pub fn energy_input(&self, interval_ns: u64) -> EnergyInput {
        let mut socket_energy: Vec<u64> = vec![0; self.num_sockets as usize];
        let mut dram_energy: Vec<u64> = vec![0; self.num_sockets as usize];
        let mut gpu_energy: Vec<u64> = Vec::new();
        let mut platform_energy: Option<u64> = None;

        for reg in &self.sources {
            let (current, previous) = match (&reg.last_reading, &reg.prev_reading) {
                (Some(curr), Some(prev)) => (curr, prev),
                _ => continue,
            };

            let delta = compute_energy_delta(current, previous);
            if delta == 0 {
                continue;
            }

            match (current.component, current.granularity) {
                (Component::Cpu, super::Granularity::Socket(s)) => {
                    if (s as usize) < socket_energy.len() {
                        socket_energy[s as usize] = delta;
                    }
                }
                (Component::Memory, super::Granularity::Socket(s)) => {
                    if (s as usize) < dram_energy.len() {
                        dram_energy[s as usize] = delta;
                    }
                }
                (Component::Gpu, _) => {
                    gpu_energy.push(delta);
                }
                (Component::Platform, _) => {
                    platform_energy = Some(delta);
                }
                _ => {} // NIC, Storage, Fan — TODO
            }
        }

        EnergyInput {
            socket_energy_uj: socket_energy,
            dram_energy_uj: dram_energy,
            gpu_energy_uj: gpu_energy,
            platform_energy_uj: platform_energy,
            interval_ns,
        }
    }

    /// Get the latest reconciliation result.
    pub fn last_reconciliation(&self) -> Option<&Reconciliation> {
        self.last_reconciliation.as_ref()
    }

    /// Read all sources (heartbeat).
    fn read_all(&mut self) {
        for reg in &mut self.sources {
            read_source(reg);
        }
    }

    /// Read sources in a specific tier.
    fn read_tier(&mut self, tier: Tier) -> bool {
        let mut any = false;
        for reg in &mut self.sources {
            if reg.tier == tier {
                if read_source(reg) {
                    any = true;
                }
            }
        }
        any
    }

    /// Run reconciliation: compare component sum against platform reading.
    fn reconcile(&mut self) {
        let mut per_component: HashMap<Component, u64> = HashMap::new();
        let mut platform_uw: Option<u64> = None;

        for reg in &self.sources {
            let reading = match &reg.last_reading {
                Some(r) => r,
                None => continue,
            };

            // Get power in microwatts (either direct or derived from energy delta)
            let power = if let Some(pw) = reading.power_uw {
                pw
            } else if let (Some(curr), Some(prev)) = (&reg.last_reading, &reg.prev_reading) {
                let delta = compute_energy_delta(curr, prev);
                let interval = curr
                    .timestamp
                    .duration_since(prev.timestamp)
                    .as_nanos() as u64;
                if interval > 0 {
                    ((delta as u128 * 1_000_000_000) / interval as u128) as u64
                } else {
                    0
                }
            } else {
                continue;
            };

            if reading.component == Component::Platform {
                platform_uw = Some(power);
            } else {
                *per_component.entry(reading.component).or_default() += power;
            }
        }

        let component_sum: u64 = per_component.values().sum();

        let (unaccounted, error_ratio) = if let Some(platform) = platform_uw {
            let unaccounted = platform as i64 - component_sum as i64;
            let ratio = if platform > 0 {
                (unaccounted.unsigned_abs() as f64) / (platform as f64)
            } else {
                0.0
            };
            (unaccounted, ratio)
        } else {
            (0, 0.0)
        };

        let recon = Reconciliation {
            platform_uw,
            component_sum_uw: component_sum,
            per_component,
            unaccounted_uw: unaccounted,
            error_ratio,
            timestamp: Instant::now(),
        };

        if platform_uw.is_some() {
            log::info!(
                "Reconciliation: platform={}W, components={}W, unaccounted={}W ({:.1}%)",
                platform_uw.unwrap_or(0) as f64 / 1e6,
                component_sum as f64 / 1e6,
                unaccounted as f64 / 1e6,
                error_ratio * 100.0,
            );
        }

        self.last_reconciliation = Some(recon);
    }
}

/// Read a single source, rotating previous reading.
fn read_source(reg: &mut RegisteredSource) -> bool {
    match reg.source.read() {
        Ok(reading) => {
            reg.prev_reading = reg.last_reading.take();
            reg.last_reading = Some(reading);
            true
        }
        Err(e) => {
            log::debug!("Failed to read {}: {}", reg.source.name(), e);
            false
        }
    }
}

/// Compute energy delta between two readings, handling wraparound.
fn compute_energy_delta(current: &PowerReading, previous: &PowerReading) -> u64 {
    match (current.energy_uj, previous.energy_uj) {
        (Some(curr), Some(prev)) => {
            if curr >= prev {
                curr - prev
            } else if current.max_energy_uj > 0 {
                // Counter wrapped around
                (current.max_energy_uj - prev) + curr
            } else {
                // No max known, can't handle wraparound — assume reset
                curr
            }
        }
        // For power-only sources, convert power × time to energy
        (None, None) => {
            match (current.power_uw, previous.power_uw) {
                (Some(curr_pw), Some(_prev_pw)) => {
                    let interval_ns = current
                        .timestamp
                        .duration_since(previous.timestamp)
                        .as_nanos() as u64;
                    // energy_uj = power_uw × time_s = power_uw × time_ns / 1e9
                    // But we want microjoules: energy_uj = power_uw × time_ns / 1e9
                    // = power_uw × time_ns / 1_000_000_000
                    ((curr_pw as u128 * interval_ns as u128) / 1_000_000_000) as u64
                }
                _ => 0,
            }
        }
        _ => 0,
    }
}

/// Assign a source to a polling tier based on its default interval.
fn classify_tier(source: &Box<dyn PowerSource>, config: &CollectorConfig) -> Tier {
    let interval = source.default_poll_interval();

    if interval <= config.fast_interval {
        Tier::Fast
    } else if interval <= config.medium_interval {
        Tier::Medium
    } else {
        Tier::Slow
    }
}
