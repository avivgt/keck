// SPDX-License-Identifier: Apache-2.0

//! eBPF observer: loads BPF programs, attaches to tracepoints,
//! and drains BPF maps for per-PID per-core CPU time data.

use std::collections::HashMap;
use std::time::Duration;

use aya::maps::{HashMap as BpfHashMap, MapData};
use aya::programs::{KProbe, TracePoint};
use aya::{Ebpf, EbpfLoader};
use keck_common::{CpuFreqKey, CpuFreqTime, PidCgroupValue, PidCpuKey, PidCpuTime, PidNetBytes};
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
    /// pid → (tx_bytes, rx_bytes) network I/O
    pub pid_net_bytes: HashMap<u32, (u64, u64)>,
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

        // Attach tcp_sendmsg kprobe (optional — network I/O tracking)
        match bpf.program_mut("tcp_sendmsg") {
            Some(prog) => {
                let result: Result<(), String> = (|| {
                    let kprobe: &mut KProbe = prog.try_into()
                        .map_err(|e| format!("cast: {}", e))?;
                    kprobe.load().map_err(|e| format!("load: {}", e))?;
                    kprobe.attach("tcp_sendmsg", 0).map_err(|e| format!("attach: {}", e))?;
                    Ok(())
                })();
                match result {
                    Ok(()) => info!("eBPF: attached tcp_sendmsg kprobe"),
                    Err(e) => warn!("eBPF: tcp_sendmsg kprobe failed ({}), network TX tracking disabled", e),
                }
            }
            None => warn!("eBPF: tcp_sendmsg program not found, network TX tracking disabled"),
        }

        // Attach tcp_recvmsg kprobe (optional — network I/O tracking)
        match bpf.program_mut("tcp_recvmsg") {
            Some(prog) => {
                let result: Result<(), String> = (|| {
                    let kprobe: &mut KProbe = prog.try_into()
                        .map_err(|e| format!("cast: {}", e))?;
                    kprobe.load().map_err(|e| format!("load: {}", e))?;
                    kprobe.attach("tcp_recvmsg", 0).map_err(|e| format!("attach: {}", e))?;
                    Ok(())
                })();
                match result {
                    Ok(()) => info!("eBPF: attached tcp_recvmsg kprobe"),
                    Err(e) => warn!("eBPF: tcp_recvmsg kprobe failed ({}), network RX tracking disabled", e),
                }
            }
            None => warn!("eBPF: tcp_recvmsg program not found, network RX tracking disabled"),
        }

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

        // Drain PID_NET_BYTES map (drain-and-delete for deltas)
        let mut pid_net_bytes = HashMap::new();
        if let Some(map_data) = self.bpf.map_mut("PID_NET_BYTES") {
            if let Ok(mut map) = BpfHashMap::<&mut MapData, u32, PidNetBytes>::try_from(map_data) {
                let mut keys_to_delete = Vec::new();
                for item in map.iter() {
                    if let Ok((pid, value)) = item {
                        pid_net_bytes.insert(pid, (value.tx_bytes, value.rx_bytes));
                        keys_to_delete.push(pid);
                    }
                }
                for key in &keys_to_delete {
                    let _ = map.remove(key);
                }
            }
        }

        Ok(EbpfSnapshot {
            pid_cpu_times,
            cpu_freq_times,
            pid_cgroups,
            pid_net_bytes,
        })
    }

    /// Remove PID_CGROUP entries for PIDs that no longer exist.
    /// Call periodically to prevent the BPF map from filling up.
    pub fn cleanup_dead_pids(&mut self) -> Result<usize, ObserverError> {
        let mut map: BpfHashMap<&mut MapData, u32, PidCgroupValue> =
            BpfHashMap::try_from(self.bpf.map_mut("PID_CGROUP").ok_or_else(|| {
                ObserverError::Ebpf("PID_CGROUP map not found".into())
            })?)
            .map_err(|e| ObserverError::Ebpf(format!("PID_CGROUP cast: {}", e)))?;

        let mut dead_pids = Vec::new();
        for item in map.iter() {
            if let Ok((pid, _)) = item {
                if !std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                    dead_pids.push(pid);
                }
            }
        }

        let count = dead_pids.len();
        for pid in &dead_pids {
            let _ = map.remove(pid);
        }

        Ok(count)
    }
}
