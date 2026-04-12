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
/// α and β are tunable coefficients. Override via KECK_ALPHA / KECK_BETA
/// environment variables.
///
/// Default values:
///   α = 0.3 — IPC typically ranges 0.5–4.0, giving contribution 0.15–1.2
///   β = 1.5 — cache miss ratio (misses/instructions) typically 0.001–0.05,
///             giving contribution 0.0015–0.075 (comparable to α's range)
///
/// Note: β was previously 0.5 with miss_rate = misses_per_1000_instructions,
/// which gave contributions of 0.5–25.0 and dominated the formula. The new
/// formulation uses the raw ratio (misses/instructions) which is dimensionless
/// and in the range [0, ~0.1], making β=1.5 well-scaled.
pub struct FullModel {
    /// Weight of IPC contribution to power
    alpha: f64,
    /// Weight of cache miss ratio contribution to power
    beta: f64,
}

impl FullModel {
    pub fn new() -> Self {
        Self {
            alpha: 0.3,
            beta: 1.5,
        }
    }

    pub fn with_coefficients(alpha: f64, beta: f64) -> Self {
        Self { alpha, beta }
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

                // Cache miss ratio: misses / instructions (dimensionless, 0–~0.1)
                let miss_ratio = if obs.instructions > 0 {
                    obs.cache_misses as f64 / obs.instructions as f64
                } else {
                    0.0
                };

                // Combined weight: base (time × freq²) × workload adjustment
                let workload_factor = 1.0 + self.alpha * ipc + self.beta * miss_ratio;
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

/// CPU time ratio model: simplest fallback.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(pid: u32, time_ns: u64, instructions: u64, cycles: u64, cache_misses: u64) -> PidCoreObservation {
        PidCoreObservation { pid, time_ns, instructions, cycles, cache_misses }
    }

    // ─── select_model() tests ────────────────────────────────────

    #[test]
    fn test_select_model_full() {
        let model = select_model(true, true);
        assert_eq!(model.method(), AttributionMethod::FullModel);
    }

    #[test]
    fn test_select_model_freq_weighted() {
        let model = select_model(true, false);
        assert_eq!(model.method(), AttributionMethod::FrequencyWeighted);
    }

    #[test]
    fn test_select_model_cpu_time_ratio_no_data() {
        let model = select_model(false, false);
        assert_eq!(model.method(), AttributionMethod::CpuTimeRatio);
    }

    #[test]
    fn test_select_model_counters_without_freq_falls_back() {
        // counters=true, freq=false => CpuTimeRatio (needs both for FullModel)
        let model = select_model(false, true);
        assert_eq!(model.method(), AttributionMethod::FrequencyWeighted);
    }

    // ─── CpuTimeRatioModel tests ─────────────────────────────────

    #[test]
    fn test_cpu_time_ratio_proportional() {
        let model = CpuTimeRatioModel;
        let observations = vec![
            obs(100, 1000, 0, 0, 0),
            obs(200, 3000, 0, 0, 0),
        ];

        let weights = model.compute_weights(&observations, 2_000_000);
        assert_eq!(weights.len(), 2);

        let total: f64 = weights.iter().map(|w| w.raw_weight).sum();
        let ratio_100 = weights[0].raw_weight / total;
        let ratio_200 = weights[1].raw_weight / total;

        assert!((ratio_100 - 0.25).abs() < 1e-10); // 1000/4000
        assert!((ratio_200 - 0.75).abs() < 1e-10); // 3000/4000
    }

    #[test]
    fn test_cpu_time_ratio_ignores_frequency() {
        let model = CpuTimeRatioModel;
        let observations = vec![obs(100, 1000, 0, 0, 0)];

        let w1 = model.compute_weights(&observations, 1_000_000);
        let w2 = model.compute_weights(&observations, 3_000_000);

        assert_eq!(w1[0].raw_weight, w2[0].raw_weight);
    }

    #[test]
    fn test_cpu_time_ratio_empty() {
        let model = CpuTimeRatioModel;
        let weights = model.compute_weights(&[], 2_000_000);
        assert!(weights.is_empty());
    }

    // ─── FrequencyWeightedModel tests ────────────────────────────

    #[test]
    fn test_freq_weighted_scales_with_frequency() {
        let model = FrequencyWeightedModel;
        let observations = vec![obs(100, 1000, 0, 0, 0)];

        let w_low = model.compute_weights(&observations, 1_000_000);  // 1 GHz
        let w_high = model.compute_weights(&observations, 2_000_000); // 2 GHz

        // At 2GHz, freq_factor = (2e6/1e6)^2 = 4.0
        // At 1GHz, freq_factor = (1e6/1e6)^2 = 1.0
        assert!((w_high[0].raw_weight / w_low[0].raw_weight - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_freq_weighted_proportional_by_time() {
        let model = FrequencyWeightedModel;
        let observations = vec![
            obs(100, 1000, 0, 0, 0),
            obs(200, 2000, 0, 0, 0),
        ];

        let weights = model.compute_weights(&observations, 2_000_000);
        let total: f64 = weights.iter().map(|w| w.raw_weight).sum();
        let ratio = weights[0].raw_weight / total;
        assert!((ratio - 1.0 / 3.0).abs() < 1e-10);
    }

    // ─── FullModel tests ─────────────────────────────────────────

    #[test]
    fn test_full_model_ipc_effect() {
        let model = FullModel::new();
        // Process A: high IPC (compute-intensive)
        let obs_high_ipc = vec![obs(100, 1000, 2000, 1000, 0)]; // IPC=2.0
        // Process B: low IPC (memory-bound)
        let obs_low_ipc = vec![obs(200, 1000, 500, 1000, 0)]; // IPC=0.5

        let w_high = model.compute_weights(&obs_high_ipc, 2_000_000);
        let w_low = model.compute_weights(&obs_low_ipc, 2_000_000);

        // Higher IPC should produce higher weight (more compute work done)
        assert!(w_high[0].raw_weight > w_low[0].raw_weight);
    }

    #[test]
    fn test_full_model_cache_miss_effect() {
        let model = FullModel::new();
        // Process A: lots of cache misses (memory-heavy)
        let obs_missy = vec![obs(100, 1000, 10000, 10000, 5000)];
        // Process B: few cache misses
        let obs_no_miss = vec![obs(200, 1000, 10000, 10000, 0)];

        let w_missy = model.compute_weights(&obs_missy, 2_000_000);
        let w_clean = model.compute_weights(&obs_no_miss, 2_000_000);

        // More cache misses => higher weight (more power consumed)
        assert!(w_missy[0].raw_weight > w_clean[0].raw_weight);
    }

    #[test]
    fn test_full_model_zero_cycles() {
        let model = FullModel::new();
        let observations = vec![obs(100, 1000, 500, 0, 10)]; // cycles=0
        let weights = model.compute_weights(&observations, 2_000_000);
        // Should not panic, IPC falls back to 0
        assert!(weights[0].raw_weight > 0.0);
    }

    #[test]
    fn test_full_model_zero_instructions() {
        let model = FullModel::new();
        let observations = vec![obs(100, 1000, 0, 1000, 10)]; // instructions=0
        let weights = model.compute_weights(&observations, 2_000_000);
        // miss_rate = 0 (guarded by instructions > 0)
        assert!(weights[0].raw_weight > 0.0);
    }

    // ─── Energy conservation property ────────────────────────────

    #[test]
    fn test_weights_sum_enables_conservation() {
        // After normalization: sum(pid_energy) = core_energy
        // Here we verify the weights are non-negative and sum > 0
        let model = select_model(true, true);
        let observations = vec![
            obs(100, 5000, 10000, 8000, 200),
            obs(200, 3000, 6000, 4000, 100),
            obs(300, 2000, 3000, 2000, 50),
        ];

        let weights = model.compute_weights(&observations, 2_500_000);
        let total: f64 = weights.iter().map(|w| w.raw_weight).sum();
        assert!(total > 0.0);
        for w in &weights {
            assert!(w.raw_weight >= 0.0);
        }

        // Simulate normalization with 100_000 uj core energy
        let core_energy = 100_000u64;
        let mut pid_energy_sum = 0u64;
        for w in &weights {
            let ratio = w.raw_weight / total;
            pid_energy_sum += (core_energy as f64 * ratio) as u64;
        }
        // Due to integer rounding, may differ by a few uj
        assert!((pid_energy_sum as i64 - core_energy as i64).unsigned_abs() <= weights.len() as u64);
    }
}
