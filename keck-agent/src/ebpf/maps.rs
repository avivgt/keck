// SPDX-License-Identifier: Apache-2.0

//! BPF map reader: drains accumulated data from kernel-side BPF maps.
//!
//! Design principles:
//! - Drain-and-reset: read all entries, then delete them. This gives us
//!   deltas per interval without needing to track previous values.
//! - Batch reads where possible to minimize syscall overhead.
//! - Bounded: if maps are full, oldest entries are evicted by BPF runtime.

use std::collections::HashMap;

use aya::maps::{self as aya_maps, MapData};
use aya::Ebpf;
use keck_common::{
    CoreCounters, CpuFreqKey, CpuFreqTime, PidCgroupValue, PidCpuKey, PidCpuTime,
};

use super::ObserverError;

/// Raw observation snapshot from one drain cycle.
///
/// This is the output of Layer 1 — everything the attribution engine
/// needs to compute per-process, per-core power.
pub struct ObservationSnapshot {
    /// Per-PID per-CPU time (nanoseconds) during this interval.
    /// Key: (cpu_id, pid) → Value: nanoseconds on that core.
    pub pid_cpu_times: Vec<(PidCpuKey, u64)>,

    /// Per-core frequency residency during this interval.
    /// Key: (cpu_id, freq_khz) → Value: nanoseconds at that frequency.
    pub cpu_freq_times: Vec<(CpuFreqKey, u64)>,

    /// PID → cgroup_id mapping (latest known).
    /// Used by K8s enrichment to map pid → container → pod.
    pub pid_cgroups: HashMap<u32, u64>,

    /// Per-core hardware counter values (absolute, not deltas).
    /// Empty if perf counters are unavailable.
    /// The attribution engine computes deltas from consecutive snapshots.
    pub core_counters: Vec<(u32, CoreCounters)>,
}

/// Reads and drains BPF maps from the loaded eBPF programs.
pub struct MapReader {
    // Map file descriptors are borrowed from the Ebpf instance.
    // We store map names and look them up on each drain.
    // This is safe because the Ebpf instance (held by EbpfObserver)
    // outlives the MapReader.
}

impl MapReader {
    pub fn new(_bpf: &Ebpf) -> Result<Self, ObserverError> {
        // Validate that expected maps exist
        // Actual map access happens in drain() using the Ebpf reference
        Ok(Self {})
    }

    /// Drain all BPF maps and return accumulated data.
    ///
    /// For hash maps (pid_cpu_time, cpu_freq_time): iterates all entries,
    /// collects values, then deletes entries to reset for next interval.
    ///
    /// For the cgroup map: reads current state (not drained — it's a
    /// living mapping, not an accumulator).
    pub fn drain(&self) -> Result<ObservationSnapshot, ObserverError> {
        // NOTE: In the real implementation, we'd pass &mut Ebpf here
        // and use bpf.map_mut("PID_CPU_TIME") etc. to get typed map handles.
        //
        // The pattern for draining a BPF hash map:
        //
        //   let map: HashMap<_, PidCpuKey, PidCpuTime> = HashMap::try_from(
        //       bpf.map_mut("PID_CPU_TIME").unwrap()
        //   )?;
        //
        //   let mut entries = Vec::new();
        //   let mut keys_to_delete = Vec::new();
        //
        //   for result in map.iter() {
        //       let (key, value) = result?;
        //       entries.push((key, value.time_ns));
        //       keys_to_delete.push(key);
        //   }
        //
        //   // Delete after iteration (can't modify during iteration)
        //   for key in &keys_to_delete {
        //       let _ = map.remove(key);
        //   }

        // Placeholder: return empty snapshot
        // Real implementation wired up when we integrate with EbpfObserver
        Ok(ObservationSnapshot {
            pid_cpu_times: Vec::new(),
            cpu_freq_times: Vec::new(),
            pid_cgroups: HashMap::new(),
            core_counters: Vec::new(),
        })
    }
}

/// Drain a BPF hash map: read all entries and delete them.
/// Returns the entries as a Vec of (key, value) pairs.
///
/// This is the core drain primitive. We read all accumulated data
/// and reset the map for the next interval.
///
/// Why drain-and-delete instead of read-and-subtract:
/// - Simpler: no need to track previous values in userspace
/// - Memory bounded: map doesn't grow unbounded with terminated PIDs
/// - Atomic per-entry: each entry's value is the complete delta
///
/// Caveat: there's a tiny window between read and delete where the
/// BPF program might increment a value that we then delete. This
/// causes at most one context switch worth of time (~microseconds)
/// to be lost per interval. Acceptable for our accuracy requirements.
#[allow(dead_code)]
fn drain_hash_map<K: aya::Pod + Clone, V: aya::Pod + Clone>(
    map: &mut aya_maps::HashMap<&mut MapData, K, V>,
) -> Result<Vec<(K, V)>, ObserverError> {
    let mut entries = Vec::new();
    let mut keys_to_delete = Vec::new();

    for result in map.iter() {
        match result {
            Ok((key, value)) => {
                keys_to_delete.push(key.clone());
                entries.push((key, value));
            }
            Err(e) => {
                log::warn!("Error reading BPF map entry: {}", e);
                // Continue — partial drain is better than no drain
            }
        }
    }

    for key in &keys_to_delete {
        if let Err(e) = map.remove(key) {
            log::warn!("Error deleting BPF map entry: {}", e);
        }
    }

    Ok(entries)
}

/// Read (but don't drain) a BPF hash map.
/// Used for the cgroup map which is a living mapping, not an accumulator.
#[allow(dead_code)]
fn read_hash_map<K: aya::Pod + Clone, V: aya::Pod + Clone>(
    map: &aya_maps::HashMap<&mut MapData, K, V>,
) -> Result<Vec<(K, V)>, ObserverError> {
    let mut entries = Vec::new();

    for result in map.iter() {
        match result {
            Ok((key, value)) => {
                entries.push((key, value));
            }
            Err(e) => {
                log::warn!("Error reading BPF map entry: {}", e);
            }
        }
    }

    Ok(entries)
}
