// SPDX-License-Identifier: Apache-2.0

//! Per-core hardware performance counter reader for LLC misses.
//!
//! Opens perf_event_open per CPU core to read LLC cache misses.
//! Each LLC miss = one DRAM access = dynamic memory power cost.
//!
//! Combined with eBPF sched_switch per-PID per-core time data,
//! LLC misses can be attributed to individual processes proportionally.

use std::os::unix::io::RawFd;

/// Per-core LLC miss counter.
pub struct LlcMissReader {
    fds: Vec<(u32, RawFd)>, // (core_id, fd)
    prev_values: Vec<(u32, u64)>,
}

/// perf_event_attr constants
const PERF_TYPE_HARDWARE: u32 = 0;
const PERF_COUNT_HW_CACHE_MISSES: u64 = 3;

/// Simplified perf_event_attr
#[repr(C)]
#[derive(Default)]
struct PerfEventAttr {
    type_: u32,
    size: u32,
    config: u64,
    _rest: [u8; 104], // padding to match kernel struct size
}

impl LlcMissReader {
    /// Open LLC miss counters on all online CPUs.
    /// Requires CAP_PERFMON or privileged container.
    pub fn new() -> Result<Self, String> {
        let num_cores = num_online_cpus().map_err(|e| format!("CPU count: {}", e))?;

        let mut fds = Vec::new();
        for cpu in 0..num_cores {
            match perf_event_open_llc(cpu as i32) {
                Ok(fd) => fds.push((cpu as u32, fd)),
                Err(e) => {
                    // Clean up already opened FDs
                    for (_, fd) in &fds {
                        unsafe { libc::close(*fd); }
                    }
                    return Err(format!("perf_event_open on CPU {}: {}", cpu, e));
                }
            }
        }

        let prev_values = fds.iter().map(|&(cpu, _)| (cpu, 0u64)).collect();

        log::info!("LLC miss counters opened on {} cores", fds.len());
        Ok(Self { fds, prev_values })
    }

    /// Read LLC miss deltas since last read, per core.
    /// Returns Vec<(core_id, llc_miss_delta)>.
    pub fn read_deltas(&mut self) -> Vec<(u32, u64)> {
        let mut deltas = Vec::with_capacity(self.fds.len());

        for (i, &(cpu, fd)) in self.fds.iter().enumerate() {
            let current = read_counter(fd).unwrap_or(0);
            let prev = self.prev_values[i].1;
            let delta = current.saturating_sub(prev);
            self.prev_values[i].1 = current;
            deltas.push((cpu, delta));
        }

        deltas
    }

    /// Total LLC misses across all cores (delta since last read).
    pub fn total_deltas(&mut self) -> u64 {
        self.read_deltas().iter().map(|&(_, d)| d).sum()
    }
}

impl Drop for LlcMissReader {
    fn drop(&mut self) {
        for &(_, fd) in &self.fds {
            unsafe { libc::close(fd); }
        }
    }
}

fn perf_event_open_llc(cpu: i32) -> Result<RawFd, std::io::Error> {
    let mut attr = PerfEventAttr::default();
    attr.type_ = PERF_TYPE_HARDWARE;
    attr.size = std::mem::size_of::<PerfEventAttr>() as u32;
    attr.config = PERF_COUNT_HW_CACHE_MISSES;

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
