// SPDX-License-Identifier: Apache-2.0

//! eBPF observer: loads BPF programs, attaches to tracepoints,
//! and drains BPF maps for per-PID per-core CPU time data.

use std::collections::HashMap;
use std::time::Duration;

use aya::maps::{HashMap as BpfHashMap, MapData};
use aya::programs::TracePoint;
use aya::{Ebpf, EbpfLoader};
use keck_common::{CpuFreqKey, CpuFreqTime, PidCgroupValue, PidCpuKey, PidCpuTime};
use log::{info, warn};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ObserverError {
    #[error("ebpf error: {0}")]
    Ebpf(String),
}

/// Per-PID per-core time data from eBPF.
pub struct EbpfSnapshot {
    /// (cpu, pid) → nanoseconds on that core this interval
    pub pid_cpu_times: Vec<(u32, u32, u64)>,
    /// (cpu, freq_khz) → nanoseconds at that frequency
    pub cpu_freq_times: Vec<(u32, u32, u64)>,
    /// pid → cgroup_id
    pub pid_cgroups: HashMap<u32, u64>,
}

pub struct EbpfObserver {
    bpf: Ebpf,
}

impl EbpfObserver {
    /// Load eBPF programs and attach to tracepoints.
    pub fn load() -> Result<Self, ObserverError> {
        let mut bpf = EbpfLoader::new()
            .load(aya::include_bytes_aligned!(concat!(
                env!("OUT_DIR"),
                "/ebpf"
            )))
            .map_err(|e| ObserverError::Ebpf(format!("load: {}", e)))?;

        // Attach sched_switch
        let prog: &mut TracePoint = bpf
            .program_mut("sched_switch")
            .ok_or_else(|| ObserverError::Ebpf("sched_switch program not found".into()))?
            .try_into()
            .map_err(|e| ObserverError::Ebpf(format!("sched_switch cast: {}", e)))?;
        prog.load()
            .map_err(|e| ObserverError::Ebpf(format!("sched_switch load: {}", e)))?;
        prog.attach("sched", "sched_switch")
            .map_err(|e| ObserverError::Ebpf(format!("sched_switch attach: {}", e)))?;
        info!("eBPF: attached sched_switch tracepoint");

        // Attach cpu_frequency
        let prog: &mut TracePoint = bpf
            .program_mut("cpu_frequency")
            .ok_or_else(|| ObserverError::Ebpf("cpu_frequency program not found".into()))?
            .try_into()
            .map_err(|e| ObserverError::Ebpf(format!("cpu_frequency cast: {}", e)))?;
        prog.load()
            .map_err(|e| ObserverError::Ebpf(format!("cpu_frequency load: {}", e)))?;
        prog.attach("power", "cpu_frequency")
            .map_err(|e| ObserverError::Ebpf(format!("cpu_frequency attach: {}", e)))?;
        info!("eBPF: attached cpu_frequency tracepoint");

        Ok(Self { bpf })
    }

    /// Drain BPF maps: read all entries and delete them.
    /// Returns per-PID per-core time data for this interval.
    pub fn drain(&mut self) -> Result<EbpfSnapshot, ObserverError> {
        let mut pid_cpu_times = Vec::new();
        let mut cpu_freq_times = Vec::new();
        let mut pid_cgroups = HashMap::new();

        // Drain PID_CPU_TIME map
        {
            let mut map: BpfHashMap<&mut MapData, PidCpuKey, PidCpuTime> =
                BpfHashMap::try_from(self.bpf.map_mut("PID_CPU_TIME").ok_or_else(|| {
                    ObserverError::Ebpf("PID_CPU_TIME map not found".into())
                })?)
                .map_err(|e| ObserverError::Ebpf(format!("PID_CPU_TIME cast: {}", e)))?;

            let mut keys_to_delete = Vec::new();
            for item in map.iter() {
                if let Ok((key, value)) = item {
                    pid_cpu_times.push((key.cpu, key.pid, value.time_ns));
                    keys_to_delete.push(key);
                }
            }
            for key in &keys_to_delete {
                let _ = map.remove(key);
            }
        }

        // Drain CPU_FREQ_TIME map
        {
            let mut map: BpfHashMap<&mut MapData, CpuFreqKey, CpuFreqTime> =
                BpfHashMap::try_from(self.bpf.map_mut("CPU_FREQ_TIME").ok_or_else(|| {
                    ObserverError::Ebpf("CPU_FREQ_TIME map not found".into())
                })?)
                .map_err(|e| ObserverError::Ebpf(format!("CPU_FREQ_TIME cast: {}", e)))?;

            let mut keys_to_delete = Vec::new();
            for item in map.iter() {
                if let Ok((key, value)) = item {
                    cpu_freq_times.push((key.cpu, key.freq_khz, value.time_ns));
                    keys_to_delete.push(key);
                }
            }
            for key in &keys_to_delete {
                let _ = map.remove(key);
            }
        }

        // Read PID_CGROUP map (don't drain — it's a living mapping)
        {
            let mut map: BpfHashMap<&mut MapData, u32, PidCgroupValue> =
                BpfHashMap::try_from(self.bpf.map_mut("PID_CGROUP").ok_or_else(|| {
                    ObserverError::Ebpf("PID_CGROUP map not found".into())
                })?)
                .map_err(|e| ObserverError::Ebpf(format!("PID_CGROUP cast: {}", e)))?;

            for item in map.iter() {
                if let Ok((pid, value)) = item {
                    pid_cgroups.insert(pid, value.cgroup_id);
                }
            }
        }

        Ok(EbpfSnapshot {
            pid_cpu_times,
            cpu_freq_times,
            pid_cgroups,
        })
    }
}
