// SPDX-License-Identifier: Apache-2.0

//! Shared types between eBPF programs and userspace agent.
//!
//! These types are used as BPF map keys and values. They must be:
//! - `#[repr(C)]` for stable memory layout across eBPF and userspace
//! - Plain old data (no pointers, no heap allocation)
//! - `no_std` compatible (eBPF programs cannot use std)

#![no_std]

// ─── sched_switch: per-PID per-CPU time tracking ─────────────────

/// Key for the pid_cpu_time BPF hash map.
/// Tracks how long each PID ran on each CPU core.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PidCpuKey {
    /// CPU core index (0..N)
    pub cpu: u32,
    /// Process ID
    pub pid: u32,
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for PidCpuKey {}

/// Value for the pid_cpu_time BPF hash map.
/// Accumulated nanoseconds that this PID ran on this CPU.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PidCpuTime {
    /// Total nanoseconds this PID ran on this CPU since last drain
    pub time_ns: u64,
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for PidCpuTime {}

/// Per-CPU state for tracking the currently running task.
/// Stored in a BPF per-CPU array (one entry per CPU, no locking needed).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CpuSchedState {
    /// PID of the currently running task on this CPU
    pub current_pid: u32,
    /// Timestamp (ktime_ns) when this task started running
    pub start_time_ns: u64,
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for CpuSchedState {}

// ─── cpu_frequency: per-core frequency tracking ──────────────────

/// Key for the cpu_freq_time BPF hash map.
/// Tracks how long each CPU spent at each frequency.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CpuFreqKey {
    /// CPU core index
    pub cpu: u32,
    /// Frequency in KHz
    pub freq_khz: u32,
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for CpuFreqKey {}

/// Value for the cpu_freq_time BPF hash map.
/// Accumulated nanoseconds at this frequency.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CpuFreqTime {
    /// Total nanoseconds at this frequency since last drain
    pub time_ns: u64,
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for CpuFreqTime {}

/// Per-CPU state for tracking current frequency.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CpuFreqState {
    /// Current frequency in KHz
    pub current_freq_khz: u32,
    /// Timestamp when this frequency was set
    pub start_time_ns: u64,
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for CpuFreqState {}

// ─── Hardware performance counters (per-core, attributed by time) ──

/// Per-core hardware counter snapshot.
/// These are absolute values read from perf_event; userspace computes deltas.
/// Stored in a per-CPU array, updated on each sched_switch.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CoreCounters {
    /// Instructions retired on this core
    pub instructions: u64,
    /// CPU cycles (actual, reflects frequency scaling)
    pub cycles: u64,
    /// Last-level cache misses
    pub cache_misses: u64,
    /// Cache references (for computing miss ratio)
    pub cache_refs: u64,
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for CoreCounters {}

// ─── cgroup tracking: pid → cgroup_id ────────────────────────────

/// Value for the pid_cgroup BPF hash map.
/// Maps PID to its cgroup ID for K8s container attribution.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PidCgroupValue {
    /// Cgroup v2 ID (from bpf_get_current_cgroup_id)
    pub cgroup_id: u64,
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for PidCgroupValue {}

// ─── Network I/O tracking: per-PID bytes sent/received ───────

/// Value for the pid_net_bytes BPF hash map.
/// Accumulates TCP bytes sent and received per PID.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PidNetBytes {
    /// Total bytes sent via TCP
    pub tx_bytes: u64,
    /// Total bytes received via TCP
    pub rx_bytes: u64,
}

#[cfg(feature = "userspace")]
unsafe impl aya::Pod for PidNetBytes {}

// ─── Map size constants ──────────────────────────────────────────

/// Maximum number of entries in the pid_cpu_time hash map.
/// Sized for Standard profile (10K PIDs × ~4 cores avg = ~40K entries).
/// Overridden at load time based on agent profile.
pub const MAX_PID_CPU_ENTRIES: u32 = 65536;

/// Maximum number of CPUs supported.
/// Per-CPU arrays are sized to this.
pub const MAX_CPUS: u32 = 1024;

/// Maximum entries in cpu_freq_time map.
/// CPUs × unique frequencies (typically ~20 P-states per CPU).
pub const MAX_CPU_FREQ_ENTRIES: u32 = 32768;

/// Maximum entries in pid_cgroup map.
pub const MAX_PID_CGROUP_ENTRIES: u32 = 65536;

/// Maximum entries in pid_net_bytes map.
pub const MAX_PID_NET_ENTRIES: u32 = 65536;

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem;

    #[test]
    fn test_pid_cpu_key_size() {
        // Must be repr(C), so size = 4+4 = 8 bytes
        assert_eq!(mem::size_of::<PidCpuKey>(), 8);
    }

    #[test]
    fn test_pid_cpu_time_size() {
        assert_eq!(mem::size_of::<PidCpuTime>(), 8);
    }

    #[test]
    fn test_cpu_sched_state_size() {
        // u32 (4) + padding (4) + u64 (8) = 16 bytes (repr(C))
        assert_eq!(mem::size_of::<CpuSchedState>(), 16);
    }

    #[test]
    fn test_cpu_freq_key_size() {
        assert_eq!(mem::size_of::<CpuFreqKey>(), 8);
    }

    #[test]
    fn test_cpu_freq_time_size() {
        assert_eq!(mem::size_of::<CpuFreqTime>(), 8);
    }

    #[test]
    fn test_cpu_freq_state_size() {
        // u32 (4) + padding (4) + u64 (8) = 16 bytes
        assert_eq!(mem::size_of::<CpuFreqState>(), 16);
    }

    #[test]
    fn test_core_counters_size() {
        // 4 x u64 = 32 bytes
        assert_eq!(mem::size_of::<CoreCounters>(), 32);
    }

    #[test]
    fn test_pid_cgroup_value_size() {
        assert_eq!(mem::size_of::<PidCgroupValue>(), 8);
    }

    #[test]
    fn test_constants() {
        assert_eq!(MAX_PID_CPU_ENTRIES, 65536);
        assert_eq!(MAX_CPUS, 1024);
        assert_eq!(MAX_CPU_FREQ_ENTRIES, 32768);
        assert_eq!(MAX_PID_CGROUP_ENTRIES, 65536);
    }

    #[test]
    fn test_pid_cpu_key_clone() {
        let key = PidCpuKey { cpu: 3, pid: 42 };
        let cloned = key;
        assert_eq!(cloned.cpu, 3);
        assert_eq!(cloned.pid, 42);
    }

    #[test]
    fn test_core_counters_clone() {
        let counters = CoreCounters {
            instructions: 1000,
            cycles: 2000,
            cache_misses: 50,
            cache_refs: 500,
        };
        let cloned = counters;
        assert_eq!(cloned.instructions, 1000);
        assert_eq!(cloned.cycles, 2000);
        assert_eq!(cloned.cache_misses, 50);
        assert_eq!(cloned.cache_refs, 500);
    }

    #[test]
    fn test_repr_c_layout_pid_cpu_key() {
        // Verify offsets are stable for BPF interop
        let key = PidCpuKey { cpu: 0xAABBCCDD, pid: 0x11223344 };
        let bytes = unsafe {
            core::slice::from_raw_parts(&key as *const _ as *const u8, mem::size_of::<PidCpuKey>())
        };
        // cpu comes first in repr(C)
        let cpu_bytes = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(cpu_bytes, 0xAABBCCDD);
        let pid_bytes = u32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        assert_eq!(pid_bytes, 0x11223344);
    }
}
