// SPDX-License-Identifier: Apache-2.0

//! Per-core hardware performance counter reader.
//!
//! Opens perf_event_open per CPU core for multiple hardware counters:
//! - Instructions retired (actual work done)
//! - CPU cycles (frequency-weighted time)
//! - LLC cache misses (DRAM access proxy)
//!
//! Combined with eBPF sched_switch per-PID per-core time, these counters
//! enable accurate per-process power attribution:
//!   core_power = f(instructions, cycles, cache_misses)
//!   pid_share = pid_time_on_core / total_time_on_core
//!   pid_power += pid_share × core_power

use std::os::unix::io::RawFd;

/// Per-core snapshot of hardware counter deltas.
#[derive(Clone, Debug)]
pub struct CoreCounterDeltas {
    pub core_id: u32,
    pub instructions: u64,
    pub cycles: u64,
    pub cache_misses: u64,
}

/// Per-core hardware counter reader — instructions, cycles, LLC misses.
pub struct HwCounterReader {
    cores: Vec<CoreFds>,
    prev: Vec<CoreSnapshot>,
}

struct CoreFds {
    core_id: u32,
    instructions_fd: RawFd,
    cycles_fd: RawFd,
    cache_misses_fd: RawFd,
}

struct CoreSnapshot {
    instructions: u64,
    cycles: u64,
    cache_misses: u64,
}

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

impl HwCounterReader {
    /// Open hardware counters on all online CPUs.
    /// Requires CAP_PERFMON or privileged container.
    pub fn new() -> Result<Self, String> {
        let num_cores = num_online_cpus().map_err(|e| format!("CPU count: {}", e))?;

        let mut cores = Vec::new();
        for cpu in 0..num_cores {
            let cpu_i = cpu as i32;
            let instructions_fd = perf_event_open(cpu_i, PERF_COUNT_HW_INSTRUCTIONS)
                .map_err(|e| {
                    // Clean up on failure
                    for c in &cores { close_core_fds(c); }
                    format!("instructions on CPU {}: {}", cpu, e)
                })?;
            let cycles_fd = perf_event_open(cpu_i, PERF_COUNT_HW_CPU_CYCLES)
                .map_err(|e| {
                    unsafe { libc::close(instructions_fd); }
                    for c in &cores { close_core_fds(c); }
                    format!("cycles on CPU {}: {}", cpu, e)
                })?;
            let cache_misses_fd = perf_event_open(cpu_i, PERF_COUNT_HW_CACHE_MISSES)
                .map_err(|e| {
                    unsafe { libc::close(instructions_fd); libc::close(cycles_fd); }
                    for c in &cores { close_core_fds(c); }
                    format!("cache_misses on CPU {}: {}", cpu, e)
                })?;

            cores.push(CoreFds {
                core_id: cpu as u32,
                instructions_fd,
                cycles_fd,
                cache_misses_fd,
            });
        }

        let prev = cores.iter().map(|_| CoreSnapshot {
            instructions: 0,
            cycles: 0,
            cache_misses: 0,
        }).collect();

        log::info!("Hardware counters opened on {} cores (instructions, cycles, LLC misses)", cores.len());
        Ok(Self { cores, prev })
    }

    /// Read per-core counter deltas since last read.
    pub fn read_deltas(&mut self) -> Vec<CoreCounterDeltas> {
        let mut deltas = Vec::with_capacity(self.cores.len());

        for (i, core) in self.cores.iter().enumerate() {
            let instr = read_counter(core.instructions_fd).unwrap_or(0);
            let cyc = read_counter(core.cycles_fd).unwrap_or(0);
            let miss = read_counter(core.cache_misses_fd).unwrap_or(0);

            let prev = &self.prev[i];
            deltas.push(CoreCounterDeltas {
                core_id: core.core_id,
                instructions: instr.saturating_sub(prev.instructions),
                cycles: cyc.saturating_sub(prev.cycles),
                cache_misses: miss.saturating_sub(prev.cache_misses),
            });

            self.prev[i] = CoreSnapshot {
                instructions: instr,
                cycles: cyc,
                cache_misses: miss,
            };
        }

        deltas
    }

    /// Total LLC misses across all cores (backward-compatible with old API).
    pub fn total_deltas(&mut self) -> u64 {
        self.read_deltas().iter().map(|d| d.cache_misses).sum()
    }
}

impl Drop for HwCounterReader {
    fn drop(&mut self) {
        for core in &self.cores {
            close_core_fds(core);
        }
    }
}

fn close_core_fds(core: &CoreFds) {
    unsafe {
        libc::close(core.instructions_fd);
        libc::close(core.cycles_fd);
        libc::close(core.cache_misses_fd);
    }
}

fn perf_event_open(cpu: i32, config: u64) -> Result<RawFd, std::io::Error> {
    let mut attr = PerfEventAttr::new();
    attr.type_ = PERF_TYPE_HARDWARE;
    attr.size = std::mem::size_of::<PerfEventAttr>() as u32;
    attr.config = config;

    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &attr as *const PerfEventAttr,
            -1i32,  // pid = -1: all processes on this CPU
            cpu,
            -1i32,  // group_fd
            0u32,   // flags
        )
    };

    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(fd as RawFd)
}

fn read_counter(fd: RawFd) -> Result<u64, std::io::Error> {
    let mut value: u64 = 0;
    let ret = unsafe {
        libc::read(
            fd,
            &mut value as *mut u64 as *mut libc::c_void,
            std::mem::size_of::<u64>(),
        )
    };
    if ret != std::mem::size_of::<u64>() as isize {
        return Err(std::io::Error::last_os_error());
    }
    Ok(value)
}

fn num_online_cpus() -> Result<usize, std::io::Error> {
    let path = if std::path::Path::new("/host/sys/devices/system/cpu/online").exists() {
        "/host/sys/devices/system/cpu/online"
    } else {
        "/sys/devices/system/cpu/online"
    };
    let cpus = std::fs::read_to_string(path)?;
    let mut count = 0;
    for range in cpus.trim().split(',') {
        let parts: Vec<&str> = range.split('-').collect();
        match parts.len() {
            1 => count += 1,
            2 => {
                let start: usize = parts[0].parse().map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad cpu range"))?;
                let end: usize = parts[1].parse().map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad cpu range"))?;
                count += end - start + 1;
            }
            _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad format")),
        }
    }
    Ok(count)
}
