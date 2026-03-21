// SPDX-License-Identifier: Apache-2.0

//! Attribution models: convert raw per-core observations into energy weights.
//!
//! The model takes per-PID per-core observations and produces a "weight"
//! for each PID on each core. These weights are then normalized so that
//! they sum to the core's total energy (from RAPL/MSR).
//!
//! Three models, selected based on available data:
//!
//! 1. FullModel: time × freq² × counter_weights (best accuracy)
//!    Requires: eBPF sched_switch + cpu_frequency + perf counters
//!
//! 2. FrequencyWeighted: time × freq² (no counter data)
//!    Requires: eBPF sched_switch + cpu_frequency
//!
//! 3. CpuTimeRatio: time only (fallback)
//!    Requires: eBPF sched_switch only (or even /proc fallback)
//!
//! All models guarantee energy conservation via normalization:
//!   Σ(process_energy_on_core) = core_energy

use super::types::AttributionMethod;

/// Input: what we know about one PID on one core during one interval.
#[derive(Clone, Debug)]
pub struct PidCoreObservation {
    pub pid: u32,
    pub time_ns: u64,
    pub instructions: u64,
    pub cycles: u64,
    pub cache_misses: u64,
}

/// Output: raw weight for one PID on one core.
/// Normalization happens in the engine, not here.
#[derive(Clone, Debug)]
pub struct PidCoreWeight {
    pub pid: u32,
    pub raw_weight: f64,
}

/// Attribution model trait.
pub trait AttributionModel: Send + Sync {
    /// Compute raw weights for all PIDs on a given core.
    ///
    /// The weights are relative — they will be normalized by the engine
    /// so that they sum to the core's measured energy.
    ///
    /// `avg_freq_khz`: weighted average frequency of this core during the interval
    fn compute_weights(
        &self,
        observations: &[PidCoreObservation],
        avg_freq_khz: u32,
    ) -> Vec<PidCoreWeight>;

    /// Which method this model uses (for metadata tagging).
    fn method(&self) -> AttributionMethod;
}

/// Full model: uses time, frequency, and hardware counters.
///
/// Weight = time_ns × (freq_khz / 1e6)² × (1 + α·IPC + β·cache_miss_rate)
///
/// Where:
/// - freq² approximates the voltage²×frequency power relationship
/// - IPC (instructions per cycle) indicates compute intensity
/// - cache_miss_rate indicates memory-bound behavior (higher power per cycle)
///
/// α and β are tunable coefficients, defaulting to values from
/// academic power modeling literature.
pub struct FullModel {
    /// Weight of IPC contribution to power
    alpha: f64,
    /// Weight of cache miss rate contribution to power
    beta: f64,
}

impl FullModel {
    pub fn new() -> Self {
        Self {
            // Default coefficients from power modeling literature.
            // These can be refined via online training against RAPL.
            alpha: 0.3, // IPC contribution
            beta: 0.5,  // Cache miss contribution (memory is power-hungry)
        }
    }
}

impl AttributionModel for FullModel {
    fn compute_weights(
        &self,
        observations: &[PidCoreObservation],
        avg_freq_khz: u32,
    ) -> Vec<PidCoreWeight> {
        let freq_factor = (avg_freq_khz as f64 / 1_000_000.0).powi(2);

        observations
            .iter()
            .map(|obs| {
                let time_factor = obs.time_ns as f64;

                // IPC: instructions per cycle (higher = more compute-dense)
                let ipc = if obs.cycles > 0 {
                    obs.instructions as f64 / obs.cycles as f64
                } else {
                    0.0
                };

                // Cache miss rate: misses per 1000 instructions
                let miss_rate = if obs.instructions > 0 {
                    (obs.cache_misses as f64 / obs.instructions as f64) * 1000.0
                } else {
                    0.0
                };

                // Combined weight: base (time × freq²) × workload adjustment
                let workload_factor = 1.0 + self.alpha * ipc + self.beta * miss_rate;
                let raw_weight = time_factor * freq_factor * workload_factor;

                PidCoreWeight {
                    pid: obs.pid,
                    raw_weight,
                }
            })
            .collect()
    }

    fn method(&self) -> AttributionMethod {
        AttributionMethod::FullModel
    }
}

/// Frequency-weighted model: uses time and frequency, no counters.
///
/// Weight = time_ns × (freq_khz / 1e6)²
///
/// This captures the fact that a process running at 3.5GHz
/// consumes ~3x the power of one at 1.2GHz for the same duration.
pub struct FrequencyWeightedModel;

impl AttributionModel for FrequencyWeightedModel {
    fn compute_weights(
        &self,
        observations: &[PidCoreObservation],
        avg_freq_khz: u32,
    ) -> Vec<PidCoreWeight> {
        let freq_factor = (avg_freq_khz as f64 / 1_000_000.0).powi(2);

        observations
            .iter()
            .map(|obs| PidCoreWeight {
                pid: obs.pid,
                raw_weight: obs.time_ns as f64 * freq_factor,
            })
            .collect()
    }

    fn method(&self) -> AttributionMethod {
        AttributionMethod::FrequencyWeighted
    }
}

/// CPU time ratio model: simplest fallback, same as Kepler.
///
/// Weight = time_ns (frequency and counters ignored)
pub struct CpuTimeRatioModel;

impl AttributionModel for CpuTimeRatioModel {
    fn compute_weights(
        &self,
        observations: &[PidCoreObservation],
        _avg_freq_khz: u32,
    ) -> Vec<PidCoreWeight> {
        observations
            .iter()
            .map(|obs| PidCoreWeight {
                pid: obs.pid,
                raw_weight: obs.time_ns as f64,
            })
            .collect()
    }

    fn method(&self) -> AttributionMethod {
        AttributionMethod::CpuTimeRatio
    }
}

/// Select the best model based on available data.
pub fn select_model(
    has_freq_data: bool,
    has_perf_counters: bool,
) -> Box<dyn AttributionModel> {
    match (has_perf_counters, has_freq_data) {
        (true, true) => Box::new(FullModel::new()),
        (false, true) => Box::new(FrequencyWeightedModel),
        _ => Box::new(CpuTimeRatioModel),
    }
}
