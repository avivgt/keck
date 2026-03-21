// SPDX-License-Identifier: Apache-2.0

//! eBPF observer: loads BPF programs, attaches to tracepoints,
//! and drains BPF maps to produce observation snapshots.
//!
//! When eBPF programs are not available (e.g., built without eBPF,
//! running in a VM without BPF support), the observer returns
//! empty snapshots and the attribution engine falls back to
//! CPU-time-ratio mode.

mod maps;
mod perf;

use std::collections::HashMap;
use std::time::Duration;

use log::{info, warn};
use thiserror::Error;

pub use maps::ObservationSnapshot;
pub use perf::PerfCounterReader;

#[derive(Debug, Error)]
pub enum ObserverError {
    #[error("failed to load eBPF programs: {0}")]
    Load(String),

    #[error("failed to attach tracepoint {name}: {reason}")]
    Attach {
        name: String,
        reason: String,
    },

    #[error("failed to read BPF map: {0}")]
    MapRead(String),

    #[error("perf counter error: {0}")]
    PerfCounter(String),
}

pub struct ObserverConfig {
    /// How often to drain BPF maps (collection interval)
    pub drain_interval: Duration,

    /// Maximum PID entries in BPF maps (determines kernel memory usage)
    pub max_pid_entries: u32,
}

/// The eBPF observer: provides a `drain()` method that returns
/// a snapshot of all kernel observations.
///
/// When eBPF is not available, returns empty snapshots.
pub struct EbpfObserver {
    perf_reader: Option<PerfCounterReader>,
    config: ObserverConfig,
    ebpf_available: bool,
}

impl EbpfObserver {
    /// Load eBPF programs and attach to tracepoints.
    ///
    /// If eBPF is not available (no programs compiled, insufficient
    /// permissions), logs a warning and falls back to perf-only or
    /// empty observation mode.
    pub fn load(config: ObserverConfig) -> Result<Self, ObserverError> {
        // TODO: When eBPF programs are compiled (via aya-build), load them here.
        // For now, we operate without eBPF and rely on hardware sources only.
        warn!("eBPF programs not compiled — running without kernel-level observation");
        warn!("Attribution will use hardware sources only (RAPL, hwmon)");

        // Try to initialize perf counter reader (optional, needs CAP_PERFMON)
        let perf_reader = match PerfCounterReader::new() {
            Ok(reader) => {
                info!(
                    "Perf counters enabled: instructions, cycles, cache_misses on {} cores",
                    reader.num_cores()
                );
                Some(reader)
            }
            Err(e) => {
                warn!("Perf counters unavailable: {}", e);
                None
            }
        };

        Ok(Self {
            perf_reader,
            config,
            ebpf_available: false,
        })
    }

    /// Drain all BPF maps and return an observation snapshot.
    pub fn drain(&mut self) -> Result<ObservationSnapshot, ObserverError> {
        let mut snapshot = ObservationSnapshot {
            pid_cpu_times: Vec::new(),
            cpu_freq_times: Vec::new(),
            pid_cgroups: HashMap::new(),
            core_counters: Vec::new(),
        };

        // Read perf counters if available
        if let Some(ref mut perf) = self.perf_reader {
            match perf.read_all_cores() {
                Ok(counters) => {
                    snapshot.core_counters = counters;
                }
                Err(e) => {
                    log::debug!("Failed to read perf counters: {}", e);
                }
            }
        }

        Ok(snapshot)
    }

    pub fn config(&self) -> &ObserverConfig {
        &self.config
    }
}
