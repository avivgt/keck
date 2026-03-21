// SPDX-License-Identifier: Apache-2.0

//! Attribution engine: the core computation that turns raw observations
//! into per-workload power numbers.
//!
//! Input:
//!   - ObservationSnapshot from Layer 1 (eBPF: per-PID per-core time,
//!     per-core frequency, hardware counters, cgroup mappings)
//!   - Energy readings from Layer 0 (RAPL per-socket, GPU, NIC, etc.)
//!
//! Output:
//!   - AttributionSnapshot with per-process, per-container, per-pod power
//!
//! Algorithm:
//!   1. Estimate per-core energy from per-socket RAPL
//!      (using frequency-weighted splitting)
//!   2. For each core, compute per-PID weights using the attribution model
//!   3. Normalize weights so they sum to core energy
//!   4. Sum across cores to get per-process CPU energy
//!   5. Attribute memory energy via LLC miss ratio
//!   6. Attribute GPU energy via compute utilization (from GPU API)
//!   7. Aggregate: process → container → pod → namespace
//!   8. Reconcile against PSU ground truth

use std::collections::HashMap;
use std::time::Instant;

use keck_common::CoreCounters;

use super::model::{select_model, AttributionModel, PidCoreObservation};
use super::types::*;
use crate::ebpf::ObservationSnapshot;

/// Energy readings from Layer 0 hardware sources.
/// Fed into the engine alongside eBPF observations.
pub struct EnergyInput {
    /// Per-socket CPU energy in microjoules (delta for this interval)
    pub socket_energy_uj: Vec<u64>,

    /// Per-socket DRAM energy in microjoules (delta for this interval)
    pub dram_energy_uj: Vec<u64>,

    /// Per-GPU energy in microjoules (delta for this interval)
    pub gpu_energy_uj: Vec<u64>,

    /// Platform (PSU) energy in microjoules (delta for this interval)
    /// None if no platform source available
    pub platform_energy_uj: Option<u64>,

    /// Interval duration in nanoseconds
    pub interval_ns: u64,
}

/// The attribution engine. Holds the model and accumulated state.
pub struct AttributionEngine {
    model: Box<dyn AttributionModel>,
    num_cores: u32,
    cores_per_socket: u32,
}

impl AttributionEngine {
    pub fn new(num_cores: u32, num_sockets: u32) -> Self {
        let cores_per_socket = if num_sockets > 0 {
            num_cores / num_sockets
        } else {
            num_cores
        };

        Self {
            // Start with CpuTimeRatio; upgraded after first observation
            model: select_model(false, false),
            num_cores,
            cores_per_socket,
        }
    }

    /// Process one interval of observations + energy readings.
    ///
    /// This is called once per collection cycle from the main loop.
    pub fn attribute(
        &mut self,
        obs: &ObservationSnapshot,
        energy: &EnergyInput,
    ) -> AttributionSnapshot {
        // Upgrade model based on available data
        let has_freq = !obs.cpu_freq_times.is_empty();
        let has_counters = !obs.core_counters.is_empty();
        self.model = select_model(has_freq, has_counters);

        // Step 1: Estimate per-core energy from per-socket RAPL
        let core_energies = self.split_socket_to_cores(energy, obs);

        // Step 2: Compute per-core average frequencies
        let core_avg_freq = self.compute_core_avg_freq(obs);

        // Step 3: Build per-core per-PID observations
        let core_pid_obs = self.build_core_pid_observations(obs);

        // Step 4: Compute per-core hardware counter deltas per PID
        let core_counter_deltas = self.build_core_counter_map(obs);

        // Step 5: For each core, attribute energy to PIDs
        let mut process_cpu_energy: HashMap<u32, u64> = HashMap::new();
        let mut process_core_detail: HashMap<u32, Vec<CoreAttribution>> = HashMap::new();

        for core in 0..self.num_cores {
            let core_energy_uj = core_energies.get(&core).copied().unwrap_or(0);
            if core_energy_uj == 0 {
                continue;
            }

            let avg_freq = core_avg_freq.get(&core).copied().unwrap_or(1_000_000);

            // Get observations for this core
            let observations: Vec<PidCoreObservation> = core_pid_obs
                .get(&core)
                .map(|pid_times| {
                    pid_times
                        .iter()
                        .map(|(&pid, &time_ns)| {
                            let counters = core_counter_deltas
                                .get(&core)
                                .and_then(|m| m.get(&pid))
                                .copied()
                                .unwrap_or(CoreCounters {
                                    instructions: 0,
                                    cycles: 0,
                                    cache_misses: 0,
                                    cache_refs: 0,
                                });

                            PidCoreObservation {
                                pid,
                                time_ns,
                                instructions: counters.instructions,
                                cycles: counters.cycles,
                                cache_misses: counters.cache_misses,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            if observations.is_empty() {
                continue;
            }

            // Compute weights via the model
            let weights = self.model.compute_weights(&observations, avg_freq);
            let total_weight: f64 = weights.iter().map(|w| w.raw_weight).sum();

            if total_weight <= 0.0 {
                continue;
            }

            // Normalize: each PID gets a fraction of the core's energy
            for (weight, obs) in weights.iter().zip(observations.iter()) {
                let ratio = weight.raw_weight / total_weight;
                let pid_energy = (core_energy_uj as f64 * ratio) as u64;

                *process_cpu_energy.entry(weight.pid).or_default() += pid_energy;

                process_core_detail
                    .entry(weight.pid)
                    .or_default()
                    .push(CoreAttribution {
                        core,
                        time_ns: obs.time_ns,
                        avg_freq_khz: avg_freq,
                        energy_uj: pid_energy,
                        instructions: obs.instructions,
                        cycles: obs.cycles,
                        cache_misses: obs.cache_misses,
                    });
            }
        }

        // Step 6: Attribute memory energy via LLC miss ratio
        let process_mem_energy = self.attribute_memory(energy, obs, &process_cpu_energy);

        // Step 7: Build process power results
        let method = self.model.method();
        let processes: Vec<ProcessPower> = process_cpu_energy
            .keys()
            .map(|&pid| {
                let cpu_energy = process_cpu_energy.get(&pid).copied().unwrap_or(0);
                let mem_energy = process_mem_energy.get(&pid).copied().unwrap_or(0);
                let comm = String::new(); // Filled by K8s enrichment layer
                let cgroup_id = obs.pid_cgroups.get(&pid).copied().unwrap_or(0);

                ProcessPower {
                    pid,
                    comm,
                    cgroup_id,
                    power: PowerBreakdown {
                        cpu_uw: energy_to_power(cpu_energy, energy.interval_ns),
                        memory_uw: energy_to_power(mem_energy, energy.interval_ns),
                        gpu_uw: 0, // TODO: GPU attribution
                        nic_uw: 0, // TODO: NIC attribution
                        storage_uw: 0, // TODO: Storage attribution
                    },
                    core_detail: process_core_detail.remove(&pid).unwrap_or_default(),
                    attribution_method: method,
                }
            })
            .collect();

        // Step 8: Compute idle power (energy on cores with no processes)
        let total_attributed: u64 = process_cpu_energy.values().sum();
        let total_core_energy: u64 = core_energies.values().sum();
        let idle_energy = total_core_energy.saturating_sub(total_attributed);

        // Step 9: Reconciliation
        let component_sum = total_core_energy
            + energy.dram_energy_uj.iter().sum::<u64>()
            + energy.gpu_energy_uj.iter().sum::<u64>();

        let attributed_sum = total_attributed + idle_energy;

        let (unaccounted, error_ratio) = if let Some(platform) = energy.platform_energy_uj {
            let unaccounted = platform as i64 - component_sum as i64;
            let error_ratio = if platform > 0 {
                (unaccounted.unsigned_abs() as f64) / (platform as f64)
            } else {
                0.0
            };
            (unaccounted, error_ratio)
        } else {
            (0, 0.0)
        };

        AttributionSnapshot {
            timestamp: Instant::now(),
            interval_ns: energy.interval_ns,
            node: NodePower {
                measured: PowerBreakdown {
                    cpu_uw: energy_to_power(total_core_energy, energy.interval_ns),
                    memory_uw: energy_to_power(
                        energy.dram_energy_uj.iter().sum(),
                        energy.interval_ns,
                    ),
                    gpu_uw: energy_to_power(
                        energy.gpu_energy_uj.iter().sum(),
                        energy.interval_ns,
                    ),
                    ..Default::default()
                },
                platform_uw: energy
                    .platform_energy_uj
                    .map(|e| energy_to_power(e, energy.interval_ns)),
                attributed_total_uw: energy_to_power(attributed_sum, energy.interval_ns),
            },
            processes,
            pods: Vec::new(),       // Filled by K8s enrichment
            namespaces: Vec::new(), // Filled by K8s enrichment
            idle_power: PowerBreakdown {
                cpu_uw: energy_to_power(idle_energy, energy.interval_ns),
                ..Default::default()
            },
            reconciliation: Reconciliation {
                platform_uw: energy
                    .platform_energy_uj
                    .map(|e| energy_to_power(e, energy.interval_ns)),
                component_sum_uw: energy_to_power(component_sum, energy.interval_ns),
                attributed_sum_uw: energy_to_power(attributed_sum, energy.interval_ns),
                unaccounted_uw: unaccounted,
                error_ratio,
            },
        }
    }

    /// Split per-socket RAPL energy to per-core estimates.
    ///
    /// Uses frequency-weighted splitting: cores running at higher frequencies
    /// get proportionally more energy attributed to them.
    ///
    /// core_energy ≈ socket_energy × (core_freq² × core_busy_time) /
    ///                                Σ(all_core_freq² × busy_time)
    fn split_socket_to_cores(
        &self,
        energy: &EnergyInput,
        obs: &ObservationSnapshot,
    ) -> HashMap<u32, u64> {
        let mut core_energies = HashMap::new();

        // Build per-core busy time and weighted frequency
        let mut core_weight: HashMap<u32, f64> = HashMap::new();
        for &(ref key, time_ns) in &obs.pid_cpu_times {
            let freq = obs
                .cpu_freq_times
                .iter()
                .filter(|(fk, _)| fk.cpu == key.cpu)
                .map(|(fk, t)| (fk.freq_khz as f64, *t as f64))
                .fold((0.0, 0.0), |(wsum, tsum), (f, t)| (wsum + f * t, tsum + t));

            let avg_freq = if freq.1 > 0.0 {
                freq.0 / freq.1
            } else {
                1_000_000.0 // Default 1GHz if no freq data
            };

            let weight = time_ns as f64 * (avg_freq / 1_000_000.0).powi(2);
            *core_weight.entry(key.cpu).or_default() += weight;
        }

        // Distribute socket energy proportionally
        for socket_idx in 0..energy.socket_energy_uj.len() {
            let socket_energy = energy.socket_energy_uj[socket_idx];
            let first_core = socket_idx as u32 * self.cores_per_socket;
            let last_core = first_core + self.cores_per_socket;

            // Sum weights for cores in this socket
            let socket_total_weight: f64 = (first_core..last_core)
                .filter_map(|c| core_weight.get(&c))
                .sum();

            if socket_total_weight <= 0.0 {
                // No work on this socket — distribute evenly
                let per_core = socket_energy / self.cores_per_socket as u64;
                for core in first_core..last_core {
                    core_energies.insert(core, per_core);
                }
                continue;
            }

            for core in first_core..last_core {
                let w = core_weight.get(&core).copied().unwrap_or(0.0);
                let ratio = w / socket_total_weight;
                let core_energy = (socket_energy as f64 * ratio) as u64;
                core_energies.insert(core, core_energy);
            }
        }

        core_energies
    }

    /// Compute weighted average frequency per core during the interval.
    fn compute_core_avg_freq(&self, obs: &ObservationSnapshot) -> HashMap<u32, u32> {
        let mut core_freq_weighted: HashMap<u32, (f64, f64)> = HashMap::new();

        for &(ref key, time_ns) in &obs.cpu_freq_times {
            let entry = core_freq_weighted.entry(key.cpu).or_default();
            entry.0 += key.freq_khz as f64 * time_ns as f64; // weighted sum
            entry.1 += time_ns as f64; // total time
        }

        core_freq_weighted
            .into_iter()
            .map(|(cpu, (wsum, tsum))| {
                let avg = if tsum > 0.0 {
                    (wsum / tsum) as u32
                } else {
                    1_000_000 // 1GHz default
                };
                (cpu, avg)
            })
            .collect()
    }

    /// Build per-core per-PID time maps from eBPF observations.
    fn build_core_pid_observations(
        &self,
        obs: &ObservationSnapshot,
    ) -> HashMap<u32, HashMap<u32, u64>> {
        let mut result: HashMap<u32, HashMap<u32, u64>> = HashMap::new();

        for &(ref key, time_ns) in &obs.pid_cpu_times {
            result
                .entry(key.cpu)
                .or_default()
                .insert(key.pid, time_ns);
        }

        result
    }

    /// Build per-core per-PID counter maps.
    ///
    /// We have per-core totals from perf_event and per-PID per-core time
    /// from eBPF. We attribute counters proportionally by time:
    ///
    ///   pid_instructions_on_core = core_instructions ×
    ///       (pid_time_on_core / total_busy_time_on_core)
    fn build_core_counter_map(
        &self,
        obs: &ObservationSnapshot,
    ) -> HashMap<u32, HashMap<u32, CoreCounters>> {
        let mut result: HashMap<u32, HashMap<u32, CoreCounters>> = HashMap::new();

        if obs.core_counters.is_empty() {
            return result;
        }

        // Build per-core total busy time
        let core_pid_times = self.build_core_pid_observations(obs);

        for &(core, ref counters) in &obs.core_counters {
            let pid_times = match core_pid_times.get(&core) {
                Some(t) => t,
                None => continue,
            };

            let total_time: u64 = pid_times.values().sum();
            if total_time == 0 {
                continue;
            }

            let per_pid = result.entry(core).or_default();

            for (&pid, &pid_time) in pid_times {
                let ratio = pid_time as f64 / total_time as f64;
                per_pid.insert(
                    pid,
                    CoreCounters {
                        instructions: (counters.instructions as f64 * ratio) as u64,
                        cycles: (counters.cycles as f64 * ratio) as u64,
                        cache_misses: (counters.cache_misses as f64 * ratio) as u64,
                        cache_refs: (counters.cache_refs as f64 * ratio) as u64,
                    },
                );
            }
        }

        result
    }

    /// Attribute DRAM energy to processes via LLC miss ratio.
    ///
    /// A process that causes more LLC misses drives more DRAM traffic
    /// and therefore consumes more DRAM power.
    fn attribute_memory(
        &self,
        energy: &EnergyInput,
        obs: &ObservationSnapshot,
        process_cpu_energy: &HashMap<u32, u64>,
    ) -> HashMap<u32, u64> {
        let total_dram_energy: u64 = energy.dram_energy_uj.iter().sum();
        if total_dram_energy == 0 {
            return HashMap::new();
        }

        // If we have per-PID cache miss data, use it for attribution
        if !obs.core_counters.is_empty() {
            let pid_cache_misses = self.aggregate_pid_counter(obs, |c| c.cache_misses);
            let total_misses: u64 = pid_cache_misses.values().sum();

            if total_misses > 0 {
                return pid_cache_misses
                    .into_iter()
                    .map(|(pid, misses)| {
                        let ratio = misses as f64 / total_misses as f64;
                        (pid, (total_dram_energy as f64 * ratio) as u64)
                    })
                    .collect();
            }
        }

        // Fallback: attribute proportionally to CPU energy
        let total_cpu: u64 = process_cpu_energy.values().sum();
        if total_cpu == 0 {
            return HashMap::new();
        }

        process_cpu_energy
            .iter()
            .map(|(&pid, &cpu_e)| {
                let ratio = cpu_e as f64 / total_cpu as f64;
                (pid, (total_dram_energy as f64 * ratio) as u64)
            })
            .collect()
    }

    /// Aggregate a specific counter field across all cores for each PID.
    fn aggregate_pid_counter<F>(&self, obs: &ObservationSnapshot, field: F) -> HashMap<u32, u64>
    where
        F: Fn(&CoreCounters) -> u64,
    {
        let core_pid_times = self.build_core_pid_observations(obs);
        let mut result: HashMap<u32, u64> = HashMap::new();

        for &(core, ref counters) in &obs.core_counters {
            let pid_times = match core_pid_times.get(&core) {
                Some(t) => t,
                None => continue,
            };

            let total_time: u64 = pid_times.values().sum();
            if total_time == 0 {
                continue;
            }

            let counter_total = field(counters);

            for (&pid, &pid_time) in pid_times {
                let ratio = pid_time as f64 / total_time as f64;
                *result.entry(pid).or_default() += (counter_total as f64 * ratio) as u64;
            }
        }

        result
    }
}

/// Convert energy (microjoules) to power (microwatts) given interval duration.
fn energy_to_power(energy_uj: u64, interval_ns: u64) -> u64 {
    if interval_ns == 0 {
        return 0;
    }
    // power_uw = energy_uj / interval_s = energy_uj * 1e9 / interval_ns
    ((energy_uj as u128 * 1_000_000_000) / interval_ns as u128) as u64
}
