// SPDX-License-Identifier: Apache-2.0

//! eBPF observer: loads BPF programs, attaches to tracepoints,
//! and drains BPF maps to produce observation snapshots.

mod maps;
mod perf;

use std::time::Duration;

use aya::programs::TracePoint;
use aya::{Ebpf, EbpfLoader};
use log::info;
use thiserror::Error;

pub use maps::{ObservationSnapshot, MapReader};
pub use perf::PerfCounterReader;

#[derive(Debug, Error)]
pub enum ObserverError {
    #[error("failed to load eBPF programs: {0}")]
    Load(#[from] aya::EbpfError),

    #[error("failed to attach tracepoint {name}: {source}")]
    Attach {
        name: String,
        source: aya::programs::ProgramError,
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

/// The eBPF observer: owns the loaded BPF programs and provides
/// a `drain()` method that returns a snapshot of all kernel observations.
pub struct EbpfObserver {
    #[allow(dead_code)] // Must keep alive — dropping unloads BPF programs
    bpf: Ebpf,
    map_reader: MapReader,
    perf_reader: Option<PerfCounterReader>,
    config: ObserverConfig,
}

impl EbpfObserver {
    /// Load eBPF programs and attach to tracepoints.
    ///
    /// This is the only place that requires elevated privileges
    /// (CAP_BPF + CAP_PERFMON, or root).
    pub fn load(config: ObserverConfig) -> Result<Self, ObserverError> {
        // Load the compiled eBPF object file (embedded at build time by aya-build)
        let mut bpf = EbpfLoader::new()
            .load(aya::include_bytes_aligned!(concat!(
                env!("OUT_DIR"),
                "/ebpf"
            )))?;

        // Initialize aya-log for BPF-side logging (if enabled)
        if let Err(e) = aya_log::EbpfLogger::init(&mut bpf) {
            log::warn!("Failed to init eBPF logger (non-fatal): {}", e);
        }

        // Attach sched_switch tracepoint
        let sched_switch: &mut TracePoint =
            bpf.program_mut("sched_switch")
                .unwrap()
                .try_into()
                .map_err(|e| ObserverError::Attach {
                    name: "sched_switch".into(),
                    source: e,
                })?;

        sched_switch
            .load()
            .map_err(|e| ObserverError::Attach {
                name: "sched_switch".into(),
                source: e,
            })?;

        sched_switch
            .attach("sched", "sched_switch")
            .map_err(|e| ObserverError::Attach {
                name: "sched_switch".into(),
                source: e,
            })?;

        info!("Attached sched_switch tracepoint");

        // Attach cpu_frequency tracepoint
        let cpu_frequency: &mut TracePoint =
            bpf.program_mut("cpu_frequency")
                .unwrap()
                .try_into()
                .map_err(|e| ObserverError::Attach {
                    name: "cpu_frequency".into(),
                    source: e,
                })?;

        cpu_frequency
            .load()
            .map_err(|e| ObserverError::Attach {
                name: "cpu_frequency".into(),
                source: e,
            })?;

        cpu_frequency
            .attach("power", "cpu_frequency")
            .map_err(|e| ObserverError::Attach {
                name: "cpu_frequency".into(),
                source: e,
            })?;

        info!("Attached cpu_frequency tracepoint");

        // Initialize the map reader (holds references to BPF maps)
        let map_reader = MapReader::new(&bpf)?;

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
                log::warn!(
                    "Perf counters unavailable (falling back to time-only attribution): {}",
                    e
                );
                None
            }
        };

        Ok(Self {
            bpf,
            map_reader,
            perf_reader,
            config,
        })
    }

    /// Drain all BPF maps and return an observation snapshot.
    ///
    /// This is called once per collection interval from the main loop.
    /// It reads and resets the accumulated data in BPF maps, and reads
    /// current perf counter values.
    ///
    /// The returned snapshot contains all raw data needed by the
    /// attribution engine (Layer 2).
    pub fn drain(&mut self) -> Result<ObservationSnapshot, ObserverError> {
        let mut snapshot = self.map_reader.drain()?;

        // Read perf counters if available
        if let Some(ref mut perf) = self.perf_reader {
            match perf.read_all_cores() {
                Ok(counters) => {
                    snapshot.core_counters = counters;
                }
                Err(e) => {
                    log::warn!("Failed to read perf counters: {}", e);
                    // Non-fatal: attribution engine falls back to time-only
                }
            }
        }

        Ok(snapshot)
    }

    pub fn config(&self) -> &ObserverConfig {
        &self.config
    }
}
