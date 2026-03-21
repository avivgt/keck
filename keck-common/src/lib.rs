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
