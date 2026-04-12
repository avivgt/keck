// SPDX-License-Identifier: Apache-2.0

//! eBPF observer: loads BPF programs, attaches to tracepoints,
//! and drains BPF maps for per-PID per-core CPU time data.

use std::collections::HashMap;
use std::os::unix::io::RawFd;

use aya::maps::{HashMap as BpfHashMap, MapData};
use aya::programs::{KProbe, TracePoint};
use aya::{Ebpf, EbpfLoader};
use keck_common::{
    CpuFreqKey, CpuFreqTime, PidCgroupValue, PidCpuCounterKey, PidCpuCounters,
    PidCpuKey, PidCpuTime, PidNetBytes,
};
use log::{info, warn};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ObserverError {
    #[error("ebpf error: {0}")]
    Ebpf(String),
}

/// Observation snapshot for the attribution engine (forward-looking type).
/// Maps eBPF data into the format expected by AttributionEngine.
/// TODO: wire AttributionEngine into main loop and populate this from EbpfSnapshot.
pub struct ObservationSnapshot {
    pub pid_cpu_times: Vec<(keck_common::PidCpuKey, u64)>,
    pub cpu_freq_times: Vec<(keck_common::CpuFreqKey, u64)>,
    pub core_counters: Vec<(u32, keck_common::CoreCounters)>,
    pub pid_cgroups: HashMap<u32, u64>,
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
    /// (cpu, pid) → (instructions, cycles, cache_misses) per-PID hardware counters
    pub pid_hw_counters: HashMap<(u32, u32), (u64, u64, u64)>,
}

pub struct EbpfObserver {
    bpf: Ebpf,
    /// Whether per-PID PMC tracking is active (perf FDs attached to BPF maps)
    pid_pmc_active: bool,
    /// Perf event FDs attached to BPF maps (kept open for lifetime of observer)
    perf_fds: Vec<RawFd>,
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

        // Attach tcp_recvmsg kretprobe (optional — network I/O tracking)
        // Uses kretprobe to read actual received bytes from return value
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
                    Ok(()) => info!("eBPF: attached tcp_recvmsg kretprobe"),
                    Err(e) => warn!("eBPF: tcp_recvmsg kretprobe failed ({}), network RX tracking disabled", e),
                }
            }
            None => warn!("eBPF: tcp_recvmsg program not found, network RX tracking disabled"),
        }

        // Attach per-CPU perf events to BPF maps for in-kernel PMC reading
        let (pid_pmc_active, perf_fds) = match attach_perf_to_bpf_maps(&mut bpf) {
            Ok(fds) => {
                info!("eBPF: per-PID PMC tracking active ({} CPUs)", fds.len() / 3);
                (true, fds)
            }
            Err(e) => {
                warn!("eBPF: per-PID PMC unavailable ({}), using per-core proportional fallback", e);
                (false, Vec::new())
            }
        };

        Ok(Self { bpf, pid_pmc_active, perf_fds })
    }

    /// Whether per-PID hardware counter tracking is active.
    pub fn has_pid_counters(&self) -> bool {
        self.pid_pmc_active
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

        // Drain PID_HW_COUNTERS map (per-PID per-core hardware counter deltas)
        let mut pid_hw_counters = HashMap::new();
        if self.pid_pmc_active {
            if let Some(map_data) = self.bpf.map_mut("PID_HW_COUNTERS") {
                if let Ok(mut map) = BpfHashMap::<&mut MapData, PidCpuCounterKey, PidCpuCounters>::try_from(map_data) {
                    let mut keys_to_delete = Vec::new();
                    for item in map.iter() {
                        if let Ok((key, value)) = item {
                            pid_hw_counters.insert(
                                (key.cpu, key.pid),
                                (value.instructions, value.cycles, value.cache_misses),
                            );
                            keys_to_delete.push(key);
                        }
                    }
                    for key in &keys_to_delete {
                        let _ = map.remove(key);
                    }
                }
            }
        }

        Ok(EbpfSnapshot {
            pid_cpu_times,
            cpu_freq_times,
            pid_cgroups,
            pid_net_bytes,
            pid_hw_counters,
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

impl Drop for EbpfObserver {
    fn drop(&mut self) {
        for &fd in &self.perf_fds {
            unsafe { libc::close(fd); }
        }
    }
}

// ─── Per-PID PMC: attach perf_event FDs to BPF PerfEventArray maps ──

/// perf_event_attr constants
const PERF_TYPE_HARDWARE: u32 = 0;
const PERF_COUNT_HW_INSTRUCTIONS: u64 = 0;
const PERF_COUNT_HW_CPU_CYCLES: u64 = 1;
const PERF_COUNT_HW_CACHE_MISSES: u64 = 3;

#[repr(C)]
struct PerfEventAttr {
    type_: u32,
    size: u32,
    config: u64,
    _rest: [u8; 104],
}

impl PerfEventAttr {
    fn new() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// Open a perf_event on a specific CPU, measuring all processes.
fn perf_event_open(cpu: i32, config: u64) -> Result<RawFd, String> {
    let mut attr = PerfEventAttr::new();
    attr.type_ = PERF_TYPE_HARDWARE;
    attr.size = std::mem::size_of::<PerfEventAttr>() as u32;
    attr.config = config;

    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &attr as *const PerfEventAttr,
            -1i32,  // all processes on this CPU
            cpu,
            -1i32,  // no group
            0u32,
        )
    };

    if fd < 0 {
        return Err(format!("perf_event_open(cpu={}, config={}): {}",
            cpu, config, std::io::Error::last_os_error()));
    }
    Ok(fd as RawFd)
}

fn num_online_cpus() -> Result<usize, String> {
    let path = if std::path::Path::new("/host/sys/devices/system/cpu/online").exists() {
        "/host/sys/devices/system/cpu/online"
    } else {
        "/sys/devices/system/cpu/online"
    };
    let cpus = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {}", path, e))?;
    let mut count = 0;
    for range in cpus.trim().split(',') {
        let parts: Vec<&str> = range.split('-').collect();
        match parts.len() {
            1 => count += 1,
            2 => {
                let start: usize = parts[0].parse().map_err(|_| "bad cpu range")?;
                let end: usize = parts[1].parse().map_err(|_| "bad cpu range")?;
                count += end - start + 1;
            }
            _ => return Err("bad cpu format".into()),
        }
    }
    Ok(count)
}

/// Extract the raw file descriptor from an aya Map.
///
/// Map in aya 0.13 doesn't implement AsFd, so we get the FD via MapInfo.
fn get_map_fd(map: &mut aya::maps::Map) -> Option<RawFd> {
    // Map::info() returns MapInfo which contains the id. But we need the FD.
    // The map FD is the underlying file descriptor that aya holds.
    // In aya 0.13, we can get it by converting to a concrete map type.
    // PerfEventArray and PerCpuArray both wrap MapData which has fd().
    // Use the map's id to look it up via BPF_MAP_GET_FD_BY_ID.
    let info = map.info().ok()?;
    let id = info.id();
    bpf_map_get_fd_by_id(id)
}

fn bpf_map_get_fd_by_id(id: u32) -> Option<RawFd> {
    #[repr(C)]
    struct BpfAttrGetId {
        id: u32,
        next_id: u32,
        open_flags: u32,
    }
    let attr = BpfAttrGetId { id, next_id: 0, open_flags: 0 };
    const BPF_MAP_GET_FD_BY_ID: u32 = 14;
    let fd = unsafe {
        libc::syscall(
            libc::SYS_bpf,
            BPF_MAP_GET_FD_BY_ID,
            &attr as *const BpfAttrGetId,
            std::mem::size_of::<BpfAttrGetId>(),
        )
    };
    if fd < 0 { None } else { Some(fd as RawFd) }
}

/// BPF syscall constants for raw map operations
const BPF_MAP_UPDATE_ELEM: u32 = 2;
const BPF_ANY: u64 = 0;

/// Raw BPF map update via syscall. Used to set perf_event FDs in
/// PerfEventArray maps (aya doesn't expose this for the HW counter
/// reading use case) and to set the PMC_ENABLED flag.
fn bpf_map_update_raw(map_fd: RawFd, key_ptr: u64, value_ptr: u64) -> Result<(), String> {
    #[repr(C)]
    struct BpfAttrMapElem {
        map_fd: u32,
        key: u64,
        value_or_next: u64,
        flags: u64,
    }

    let attr = BpfAttrMapElem {
        map_fd: map_fd as u32,
        key: key_ptr,
        value_or_next: value_ptr,
        flags: BPF_ANY,
    };

    let ret = unsafe {
        libc::syscall(
            libc::SYS_bpf,
            BPF_MAP_UPDATE_ELEM,
            &attr as *const BpfAttrMapElem,
            std::mem::size_of::<BpfAttrMapElem>(),
        )
    };

    if ret < 0 {
        Err(format!("bpf map update: {}", std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

/// Attach perf_event FDs to the BPF PerfEventArray maps and enable
/// per-PID PMC tracking by setting PMC_ENABLED flag.
///
/// For each CPU, opens perf_event for instructions, cycles, and cache_misses,
/// then attaches the FDs to the corresponding BPF map indices. The eBPF
/// sched_switch handler reads these counters at context-switch time.
fn attach_perf_to_bpf_maps(bpf: &mut Ebpf) -> Result<Vec<RawFd>, String> {
    let num_cpus = num_online_cpus()?;
    let mut fds = Vec::new();

    // Map names and their perf_event configs
    let maps = [
        ("PERF_INSTRUCTIONS", PERF_COUNT_HW_INSTRUCTIONS),
        ("PERF_CYCLES", PERF_COUNT_HW_CPU_CYCLES),
        ("PERF_CACHE_MISSES", PERF_COUNT_HW_CACHE_MISSES),
    ];

    for &(map_name, config) in &maps {
        let map_data = bpf.map_mut(map_name)
            .ok_or_else(|| format!("{} map not found", map_name))?;
        let map_fd = get_map_fd(map_data)
            .ok_or_else(|| format!("{}: cannot get map FD", map_name))?;

        for cpu in 0..num_cpus {
            let perf_fd = perf_event_open(cpu as i32, config)
                .map_err(|e| {
                    for &f in &fds { unsafe { libc::close(f); } }
                    e
                })?;

            let cpu_key = cpu as u32;
            bpf_map_update_raw(map_fd, &cpu_key as *const _ as u64, &perf_fd as *const _ as u64)
                .map_err(|e| {
                    unsafe { libc::close(perf_fd); }
                    for &f in &fds { unsafe { libc::close(f); } }
                    format!("{} set cpu {}: {}", map_name, cpu, e)
                })?;

            fds.push(perf_fd);
        }
    }

    // Enable PMC reading in the eBPF program by setting PMC_ENABLED[0] = 1
    if let Some(map_data) = bpf.map_mut("PMC_ENABLED") {
        if let Some(map_fd) = get_map_fd(map_data) {
            let key: u32 = 0;
            let value: u32 = 1;
            let _ = bpf_map_update_raw(map_fd, &key as *const _ as u64, &value as *const _ as u64);
        }
    }

    Ok(fds)
}
