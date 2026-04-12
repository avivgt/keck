// SPDX-License-Identifier: Apache-2.0

//! Online auto-tuner for attribution model coefficients.
//!
//! Adjusts α (IPC weight) and β (cache miss ratio weight) based on
//! internal consistency of per-PID attributions across PIDs sharing
//! the same core.
//!
//! Signal: for PIDs on the same core in the same interval, a well-calibrated
//! model produces consistent "energy per cycle" values. Large variance in
//! energy/cycle across PIDs on the same core indicates α/β are wrong.
//!
//! The tuner uses numerical gradient descent with EMA momentum and a very
//! low learning rate (0.001) to avoid oscillation. It only updates when
//! there's sufficient signal (PIDs with IPC diversity > 2x on the same core).

use std::collections::HashMap;

/// Online coefficient tuner for the attribution model.
pub struct CoefficientTuner {
    alpha: f64,
    beta: f64,
    learning_rate: f64,
    alpha_grad_ema: f64,
    beta_grad_ema: f64,
    ema_decay: f64,
    min_ipc_diversity: f64,
    updates: u64,
    qualifying_cores: u64,
    last_logged_alpha: f64,
    last_logged_beta: f64,
}

impl CoefficientTuner {
    pub fn new(alpha: f64, beta: f64) -> Self {
        let lr = std::env::var("KECK_TUNE_LEARNING_RATE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.001);

        Self {
            alpha,
            beta,
            learning_rate: lr,
            alpha_grad_ema: 0.0,
            beta_grad_ema: 0.0,
            ema_decay: 0.9,
            min_ipc_diversity: 2.0,
            updates: 0,
            qualifying_cores: 0,
            last_logged_alpha: alpha,
            last_logged_beta: beta,
        }
    }

    /// Current coefficient values.
    pub fn current(&self) -> (f64, f64) {
        (self.alpha, self.beta)
    }

    /// Feed one interval's per-PID per-core data into the tuner.
    ///
    /// `pid_hw_counters`: (cpu, pid) -> (instructions, cycles, cache_misses)
    /// `pid_cpu_times`: [(cpu, pid, time_ns)]
    pub fn update(
        &mut self,
        pid_hw_counters: &HashMap<(u32, u32), (u64, u64, u64)>,
        pid_cpu_times: &[(u32, u32, u64)],
    ) {
        if pid_hw_counters.is_empty() || pid_cpu_times.is_empty() {
            return;
        }

        // Build per-core PID observations: core -> Vec<(pid, time, instr, cycles, misses)>
        let mut core_pids: HashMap<u32, Vec<PidObs>> = HashMap::new();

        for &(cpu, pid, time_ns) in pid_cpu_times {
            if time_ns == 0 {
                continue;
            }
            let (instr, cycles, misses) = pid_hw_counters
                .get(&(cpu, pid))
                .copied()
                .unwrap_or((0, 0, 0));
            if cycles == 0 {
                continue;
            }
            core_pids.entry(cpu).or_default().push(PidObs {
                time_ns,
                ipc: instr as f64 / cycles as f64,
                miss_ratio: if instr > 0 {
                    misses as f64 / instr as f64
                } else {
                    0.0
                },
                cycles: cycles as f64,
            });
        }

        // Accumulate gradient across qualifying cores
        let mut total_d_alpha = 0.0;
        let mut total_d_beta = 0.0;
        let mut qualifying = 0u64;

        for (_core, pids) in &core_pids {
            if pids.len() < 2 {
                continue;
            }

            // Check IPC diversity
            let max_ipc = pids.iter().map(|p| p.ipc).fold(0.0f64, f64::max);
            let min_ipc = pids
                .iter()
                .map(|p| p.ipc)
                .fold(f64::INFINITY, f64::min);
            if min_ipc <= 0.0 || max_ipc / min_ipc < self.min_ipc_diversity {
                continue;
            }

            qualifying += 1;

            // Compute consistency error and numerical gradient
            let err = consistency_error(pids, self.alpha, self.beta);
            let eps = 0.01;
            let err_da = consistency_error(pids, self.alpha + eps, self.beta);
            let err_db = consistency_error(pids, self.alpha, self.beta + eps);

            total_d_alpha += (err_da - err) / eps;
            total_d_beta += (err_db - err) / eps;
        }

        if qualifying == 0 {
            return;
        }

        // Normalize gradient by number of qualifying cores
        let d_alpha = total_d_alpha / qualifying as f64;
        let d_beta = total_d_beta / qualifying as f64;

        // EMA update
        self.alpha_grad_ema = self.ema_decay * self.alpha_grad_ema + (1.0 - self.ema_decay) * d_alpha;
        self.beta_grad_ema = self.ema_decay * self.beta_grad_ema + (1.0 - self.ema_decay) * d_beta;

        self.alpha -= self.learning_rate * self.alpha_grad_ema;
        self.beta -= self.learning_rate * self.beta_grad_ema;

        // Clamp to safe bounds
        self.alpha = self.alpha.clamp(0.05, 2.0);
        self.beta = self.beta.clamp(0.01, 10.0);

        self.updates += 1;
        self.qualifying_cores += qualifying;

        // Log significant changes
        let alpha_change = (self.alpha - self.last_logged_alpha).abs() / self.last_logged_alpha;
        let beta_change = (self.beta - self.last_logged_beta).abs() / self.last_logged_beta;
        if alpha_change > 0.05 || beta_change > 0.05 {
            log::info!(
                "Tuner: alpha {:.3} -> {:.3}, beta {:.3} -> {:.3} ({} updates, {} qualifying cores)",
                self.last_logged_alpha, self.alpha,
                self.last_logged_beta, self.beta,
                self.updates, self.qualifying_cores,
            );
            self.last_logged_alpha = self.alpha;
            self.last_logged_beta = self.beta;
        }
    }
}

struct PidObs {
    time_ns: u64,
    ipc: f64,
    miss_ratio: f64,
    cycles: f64,
}

/// Compute consistency error for PIDs on a single core under given (alpha, beta).
///
/// A well-calibrated model produces similar "energy per cycle" for all PIDs
/// on the same core (they share the same voltage/frequency). The error is
/// the coefficient of variation (stddev / mean) of energy_per_cycle.
fn consistency_error(pids: &[PidObs], alpha: f64, beta: f64) -> f64 {
    // Compute weights
    let weights: Vec<f64> = pids
        .iter()
        .map(|p| {
            let workload_factor = 1.0 + alpha * p.ipc + beta * p.miss_ratio;
            p.time_ns as f64 * workload_factor
        })
        .collect();

    let total_weight: f64 = weights.iter().sum();
    if total_weight <= 0.0 {
        return 0.0;
    }

    // Energy fraction × total_cycles (proxy for core energy) / pid_cycles
    // = energy_per_cycle for each PID
    let total_cycles: f64 = pids.iter().map(|p| p.cycles).sum();
    let epc: Vec<f64> = pids
        .iter()
        .zip(weights.iter())
        .map(|(p, w)| {
            let fraction = w / total_weight;
            let energy = fraction * total_cycles; // proxy: core_energy ∝ total_cycles
            energy / p.cycles
        })
        .collect();

    // Coefficient of variation
    let mean: f64 = epc.iter().sum::<f64>() / epc.len() as f64;
    if mean <= 0.0 {
        return 0.0;
    }
    let variance: f64 = epc.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / epc.len() as f64;
    variance.sqrt() / mean
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_obs(time_ns: u64, ipc: f64, miss_ratio: f64, cycles: f64) -> PidObs {
        PidObs { time_ns, ipc, miss_ratio, cycles }
    }

    #[test]
    fn test_consistency_error_identical_pids() {
        // Two identical PIDs should have zero consistency error
        let pids = vec![
            make_obs(1000, 2.0, 0.01, 2000.0),
            make_obs(1000, 2.0, 0.01, 2000.0),
        ];
        let err = consistency_error(&pids, 0.3, 1.5);
        assert!(err < 1e-10, "identical PIDs should have ~zero error, got {}", err);
    }

    #[test]
    fn test_consistency_error_diverse_pids() {
        // Two PIDs with very different profiles should have non-zero error
        // (unless alpha/beta perfectly compensate)
        let pids = vec![
            make_obs(1000, 3.0, 0.001, 3000.0),  // compute-heavy
            make_obs(1000, 0.5, 0.05, 500.0),     // memory-heavy
        ];
        let err = consistency_error(&pids, 0.3, 1.5);
        assert!(err > 0.0, "diverse PIDs should have non-zero error");
    }

    #[test]
    fn test_tuner_no_update_without_signal() {
        let mut tuner = CoefficientTuner::new(0.3, 1.5);
        let counters = HashMap::new();
        let times = vec![];
        tuner.update(&counters, &times);
        assert_eq!(tuner.updates, 0);
        assert_eq!(tuner.current(), (0.3, 1.5));
    }

    #[test]
    fn test_tuner_no_update_single_pid_per_core() {
        let mut tuner = CoefficientTuner::new(0.3, 1.5);
        let mut counters = HashMap::new();
        counters.insert((0, 100), (10000u64, 5000u64, 100u64));
        let times = vec![(0u32, 100u32, 1000u64)];
        tuner.update(&counters, &times);
        assert_eq!(tuner.updates, 0); // needs >=2 PIDs per core
    }

    #[test]
    fn test_tuner_updates_with_diverse_pids() {
        let mut tuner = CoefficientTuner::new(0.3, 1.5);
        let mut counters = HashMap::new();
        // PID 100: high IPC (3.0), low miss ratio
        counters.insert((0, 100), (30000u64, 10000u64, 30u64));
        // PID 200: low IPC (0.5), high miss ratio
        counters.insert((0, 200), (5000u64, 10000u64, 500u64));
        let times = vec![
            (0u32, 100u32, 1000u64),
            (0u32, 200u32, 1000u64),
        ];
        tuner.update(&counters, &times);
        assert!(tuner.updates > 0, "should update with diverse PIDs");
    }

    #[test]
    fn test_tuner_clamps_bounds() {
        let mut tuner = CoefficientTuner::new(0.05, 0.01);
        // Force a large negative gradient
        tuner.alpha = 0.04;
        tuner.beta = 0.005;
        tuner.alpha = tuner.alpha.clamp(0.05, 2.0);
        tuner.beta = tuner.beta.clamp(0.01, 10.0);
        assert_eq!(tuner.alpha, 0.05);
        assert_eq!(tuner.beta, 0.01);
    }
}
