// SPDX-License-Identifier: Apache-2.0

//! Per-core hardware performance counter reader using perf_event_open(2).
//!
//! Opens one perf event group per CPU core with 3 counters:
//! - Instructions retired
//! - CPU cycles (actual, reflects frequency scaling)
//! - LLC cache misses
//!
//! These are per-CORE counters (not per-PID). The attribution engine
//! combines them with per-PID per-core time from eBPF to compute
//! per-process counter values:
//!
//!   process_instructions_on_core = core_instructions ×
//!       (process_time_on_core / total_busy_time_on_core)
//!
//! This approach is more efficient than per-PID perf events (which
//! require one FD per PID per counter) and equally accurate when
//! combined with precise scheduling data from eBPF.

use std::io;
use std::os::unix::io::RawFd;

use keck_common::CoreCounters;

use super::ObserverError;

/// perf_event_attr constants (from linux/perf_event.h)
const PERF_TYPE_HARDWARE: u32 = 0;

const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
const PERF_COUNT_HW_CPU_CYCLES: u64 = 0;
const PERF_COUNT_HW_CACHE_MISSES: u64 = 3;

/// Flags for perf_event_open
const PERF_FLAG_FD_CLOEXEC: u32 = 1 << 3;

/// perf_event_attr struct (simplified — only fields we use)
#[repr(C)]
#[derive(Default)]
struct PerfEventAttr {
    type_: u32,
    size: u32,
    config: u64,
    sample_period_or_freq: u64,
    sample_type: u64,
    read_format: u64,
    flags: u64, // bitfield: disabled, inherit, exclude_*, etc.
    wakeup_events_or_watermark: u32,
    bp_type: u32,
    bp_addr_or_config1: u64,
    bp_len_or_config2: u64,
    branch_sample_type: u64,
    sample_regs_user: u64,
    sample_stack_user: u32,
    clockid: i32,
    sample_regs_intr: u64,
    aux_watermark: u32,
    sample_max_stack: u16,
    reserved_2: u16,
    aux_sample_size: u32,
    reserved_3: u32,
    sig_data: u64,
    config3: u64,
}

/// One group of perf events for a single CPU core.
struct CorePerfGroup {
    /// CPU core index
    cpu: u32,
    /// File descriptor for the group leader (instructions)
    fd_instructions: RawFd,
    /// File descriptor for cycles (grouped with instructions)
    fd_cycles: RawFd,
    /// File descriptor for cache misses (grouped with instructions)
    fd_cache_misses: RawFd,
    /// Previous counter values for computing deltas
    prev: CoreCounters,
}

impl Drop for CorePerfGroup {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd_instructions);
            libc::close(self.fd_cycles);
            libc::close(self.fd_cache_misses);
        }
    }
}

/// Reads per-core hardware performance counters.
pub struct PerfCounterReader {
    groups: Vec<CorePerfGroup>,
}

impl PerfCounterReader {
    /// Open perf events on all available CPU cores.
    ///
    /// Requires CAP_PERFMON (Linux 5.8+) or CAP_SYS_ADMIN.
    /// Returns error if perf_event_open fails (e.g., in VMs without
    /// perf passthrough, or without sufficient capabilities).
    pub fn new() -> Result<Self, ObserverError> {
        let num_cores = num_online_cpus()
            .map_err(|e| ObserverError::PerfCounter(format!("failed to get CPU count: {}", e)))?;

        let mut groups = Vec::with_capacity(num_cores);

        for cpu in 0..num_cores {
            match open_core_group(cpu as u32) {
                Ok(group) => groups.push(group),
                Err(e) => {
                    // Clean up already-opened groups
                    drop(groups);
                    return Err(ObserverError::PerfCounter(format!(
                        "failed to open perf events on CPU {}: {}",
                        cpu, e
                    )));
                }
            }
        }

        Ok(Self { groups })
    }

    /// Number of cores being monitored.
    pub fn num_cores(&self) -> usize {
        self.groups.len()
    }

    /// Read current counter values from all cores.
    ///
    /// Returns (cpu_id, counters) pairs with DELTA values
    /// (difference since last read).
    pub fn read_all_cores(&mut self) -> Result<Vec<(u32, CoreCounters)>, ObserverError> {
        let mut results = Vec::with_capacity(self.groups.len());

        for group in &mut self.groups {
            let current = read_group_counters(group)?;

            // Compute deltas (handle counter wraparound via saturating_sub)
            let delta = CoreCounters {
                instructions: current.instructions.saturating_sub(group.prev.instructions),
                cycles: current.cycles.saturating_sub(group.prev.cycles),
                cache_misses: current.cache_misses.saturating_sub(group.prev.cache_misses),
                cache_refs: 0, // Not collected in this group
            };

            group.prev = current;
            results.push((group.cpu, delta));
        }

        Ok(results)
    }
}

/// Open a perf event group on a specific CPU core.
///
/// Creates three counters in one group:
/// - Group leader: instructions retired
/// - Member: CPU cycles
/// - Member: LLC cache misses
///
/// All three are read atomically with a single read() call on the
/// group leader FD.
fn open_core_group(cpu: u32) -> Result<CorePerfGroup, io::Error> {
    // Group leader: instructions
    let fd_instructions = perf_event_open(
        PERF_COUNT_HW_INSTRUCTIONS,
        -1,        // pid = -1: all processes on this CPU
        cpu as i32,
        -1,        // group_fd = -1: new group leader
    )?;

    // Group member: cycles
    let fd_cycles = perf_event_open(
        PERF_COUNT_HW_CPU_CYCLES,
        -1,
        cpu as i32,
        fd_instructions, // group with leader
    )?;

    // Group member: cache misses
    let fd_cache_misses = perf_event_open(
        PERF_COUNT_HW_CACHE_MISSES,
        -1,
        cpu as i32,
        fd_instructions,
    )?;

    Ok(CorePerfGroup {
        cpu,
        fd_instructions,
        fd_cycles,
        fd_cache_misses,
        prev: CoreCounters {
            instructions: 0,
            cycles: 0,
            cache_misses: 0,
            cache_refs: 0,
        },
    })
}

/// Wrapper around the perf_event_open(2) syscall.
fn perf_event_open(
    config: u64,
    pid: i32,
    cpu: i32,
    group_fd: RawFd,
) -> Result<RawFd, io::Error> {
    let mut attr = PerfEventAttr::default();
    attr.type_ = PERF_TYPE_HARDWARE;
    attr.size = std::mem::size_of::<PerfEventAttr>() as u32;
    attr.config = config;
    // disabled=0, exclude_kernel=0, exclude_hv=0 — count everything
    // inherit=0 — per-CPU, not per-task

    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &attr as *const PerfEventAttr,
            pid,
            cpu,
            group_fd,
            PERF_FLAG_FD_CLOEXEC,
        )
    };

    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(fd as RawFd)
}

/// Read counter values from a perf event group.
///
/// Reading the group leader FD returns all counters atomically.
fn read_group_counters(group: &CorePerfGroup) -> Result<CoreCounters, ObserverError> {
    let instructions = read_counter_value(group.fd_instructions)?;
    let cycles = read_counter_value(group.fd_cycles)?;
    let cache_misses = read_counter_value(group.fd_cache_misses)?;

    Ok(CoreCounters {
        instructions,
        cycles,
        cache_misses,
        cache_refs: 0,
    })
}

/// Read a single counter value from a perf event FD.
fn read_counter_value(fd: RawFd) -> Result<u64, ObserverError> {
    let mut value: u64 = 0;
    let ret = unsafe {
        libc::read(
            fd,
            &mut value as *mut u64 as *mut libc::c_void,
            std::mem::size_of::<u64>(),
        )
    };

    if ret != std::mem::size_of::<u64>() as isize {
        return Err(ObserverError::PerfCounter(format!(
            "read returned {} bytes, expected {}",
            ret,
            std::mem::size_of::<u64>()
        )));
    }

    Ok(value)
}

/// Get the number of online CPUs.
fn num_online_cpus() -> Result<usize, io::Error> {
    let cpus = std::fs::read_to_string("/sys/devices/system/cpu/online")?;
    // Format: "0-31" or "0-7,16-23"
    let mut count = 0;
    for range in cpus.trim().split(',') {
        let parts: Vec<&str> = range.split('-').collect();
        match parts.len() {
            1 => count += 1,
            2 => {
                let start: usize = parts[0].parse().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid CPU range")
                })?;
                let end: usize = parts[1].parse().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid CPU range")
                })?;
                count += end - start + 1;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid CPU range format",
                ))
            }
        }
    }
    Ok(count)
}
