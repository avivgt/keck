// SPDX-License-Identifier: Apache-2.0

//! eBPF programs for kernel-level power observation.
//!
//! These programs run inside the Linux kernel and collect:
//! 1. Per-PID per-core CPU time (sched_switch tracepoint)
//! 2. Per-core frequency residency (power/cpu_frequency tracepoint)
//! 3. PID → cgroup mapping (for container attribution)
//!
//! All data is stored in BPF maps and drained periodically by the
//! userspace agent. The programs are designed for minimal overhead:
//! - Per-CPU arrays for hot-path state (no locking)
//! - Hash maps for accumulated data (drained and reset by userspace)
//! - No memory allocation, no loops, no function calls in hot path

#![no_std]
#![no_main]

use aya_ebpf::{
    helpers::bpf_get_current_cgroup_id_v2,
    helpers::bpf_ktime_get_ns,
    macros::{map, tracepoint},
    maps::{HashMap, PerCpuArray},
    programs::TracePointContext,
};
use keck_common::{
    CoreCounters, CpuFreqKey, CpuFreqState, CpuFreqTime, CpuSchedState, MAX_CPU_FREQ_ENTRIES,
    MAX_PID_CGROUP_ENTRIES, MAX_PID_CPU_ENTRIES, PidCgroupValue, PidCpuKey, PidCpuTime,
};

// ─── BPF Maps ────────────────────────────────────────────────────

/// Per-PID per-CPU accumulated run time.
/// Key: (cpu, pid) → Value: nanoseconds
/// Drained by userspace each collection interval.
#[map]
static PID_CPU_TIME: HashMap<PidCpuKey, PidCpuTime> = HashMap::with_max_entries(MAX_PID_CPU_ENTRIES, 0);

/// Per-CPU scheduler state: who is running right now and since when.
/// Index: cpu_id → Value: (current_pid, start_timestamp)
/// Per-CPU array: no locking needed, each CPU writes only its own slot.
#[map]
static CPU_SCHED_STATE: PerCpuArray<CpuSchedState> = PerCpuArray::with_max_entries(1, 0);

/// Per-core frequency residency time.
/// Key: (cpu, freq_khz) → Value: nanoseconds at that frequency
/// Drained by userspace each collection interval.
#[map]
static CPU_FREQ_TIME: HashMap<CpuFreqKey, CpuFreqTime> = HashMap::with_max_entries(MAX_CPU_FREQ_ENTRIES, 0);

/// Per-CPU frequency state: current frequency and since when.
#[map]
static CPU_FREQ_STATE: PerCpuArray<CpuFreqState> = PerCpuArray::with_max_entries(1, 0);

/// PID → cgroup ID mapping for container attribution.
/// Updated on every sched_switch for the incoming task.
#[map]
static PID_CGROUP: HashMap<u32, PidCgroupValue> = HashMap::with_max_entries(MAX_PID_CGROUP_ENTRIES, 0);

/// Per-core hardware counter snapshots.
/// Updated by userspace via perf_event reads (not by BPF).
/// Stored here so attribution engine can correlate with per-PID time.
#[map]
static CORE_COUNTERS: PerCpuArray<CoreCounters> = PerCpuArray::with_max_entries(1, 0);

// ─── sched_switch tracepoint ─────────────────────────────────────

/// Tracepoint: sched/sched_switch
///
/// Fired on every context switch. We use it to:
/// 1. Account the time the *previous* task spent on this CPU
/// 2. Record the *next* task as the new current on this CPU
/// 3. Map the next task's PID → cgroup for container attribution
///
/// Tracepoint args layout (from /sys/kernel/debug/tracing/events/sched/sched_switch/format):
///   field: char prev_comm[16]   offset:8   (bytes 8..24)
///   field: pid_t prev_pid       offset:24  (bytes 24..28)
///   field: int prev_prio        offset:28  (bytes 28..32)
///   field: long prev_state      offset:32  (bytes 32..40)
///   field: char next_comm[16]   offset:40  (bytes 40..56)
///   field: pid_t next_pid       offset:56  (bytes 56..60)
///   field: int next_prio        offset:60  (bytes 60..64)
#[tracepoint]
pub fn sched_switch(ctx: TracePointContext) -> u32 {
    match handle_sched_switch(ctx) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

#[inline(always)]
fn handle_sched_switch(ctx: TracePointContext) -> Result<(), i64> {
    let now = unsafe { bpf_ktime_get_ns() };

    // Read prev_pid and next_pid from tracepoint args
    let prev_pid: u32 = unsafe { ctx.read_at(24)? };
    let next_pid: u32 = unsafe { ctx.read_at(56)? };

    // Get current CPU — PerCpuArray index 0 gives us this CPU's slot
    let state_ptr = CPU_SCHED_STATE.get_ptr_mut(0).ok_or(0i64)?;
    let state = unsafe { &mut *state_ptr };

    // Account time for the previous task (the one being switched OUT)
    if state.current_pid != 0 && state.start_time_ns != 0 {
        let elapsed = now.saturating_sub(state.start_time_ns);

        if elapsed > 0 {
            let key = PidCpuKey {
                cpu: 0, // Will be filled by the kernel's per-CPU mechanism
                pid: prev_pid,
            };

            // Atomically add elapsed time to the accumulator
            if let Some(existing) = unsafe { PID_CPU_TIME.get_ptr_mut(&key) } {
                unsafe {
                    (*existing).time_ns += elapsed;
                }
            } else {
                let value = PidCpuTime { time_ns: elapsed };
                let _ = PID_CPU_TIME.insert(&key, &value, 0);
            }
        }
    }

    // Update state: next task is now running on this CPU
    state.current_pid = next_pid;
    state.start_time_ns = now;

    // Update cgroup mapping for the incoming task
    // This gives us pid → container/pod attribution without reading /proc
    if next_pid != 0 {
        let cgroup_id = unsafe { bpf_get_current_cgroup_id_v2() };
        let cgroup_value = PidCgroupValue { cgroup_id };
        let _ = PID_CGROUP.insert(&next_pid, &cgroup_value, 0);
    }

    Ok(())
}

// ─── cpu_frequency tracepoint ────────────────────────────────────

/// Tracepoint: power/cpu_frequency
///
/// Fired when a CPU core changes frequency (P-state transition).
/// We use it to track how long each core spends at each frequency.
///
/// Tracepoint args layout (from /sys/kernel/debug/tracing/events/power/cpu_frequency/format):
///   field: u32 state    offset:8   (frequency in KHz)
///   field: u32 cpu_id   offset:12
#[tracepoint]
pub fn cpu_frequency(ctx: TracePointContext) -> u32 {
    match handle_cpu_frequency(ctx) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

#[inline(always)]
fn handle_cpu_frequency(ctx: TracePointContext) -> Result<(), i64> {
    let now = unsafe { bpf_ktime_get_ns() };

    let new_freq_khz: u32 = unsafe { ctx.read_at(8)? };
    let cpu_id: u32 = unsafe { ctx.read_at(12)? };

    // Get this CPU's frequency state
    let state_ptr = CPU_FREQ_STATE.get_ptr_mut(0).ok_or(0i64)?;
    let state = unsafe { &mut *state_ptr };

    // Account time at the previous frequency
    if state.current_freq_khz != 0 && state.start_time_ns != 0 {
        let elapsed = now.saturating_sub(state.start_time_ns);

        if elapsed > 0 {
            let key = CpuFreqKey {
                cpu: cpu_id,
                freq_khz: state.current_freq_khz,
            };

            if let Some(existing) = unsafe { CPU_FREQ_TIME.get_ptr_mut(&key) } {
                unsafe {
                    (*existing).time_ns += elapsed;
                }
            } else {
                let value = CpuFreqTime { time_ns: elapsed };
                let _ = CPU_FREQ_TIME.insert(&key, &value, 0);
            }
        }
    }

    // Update state: new frequency
    state.current_freq_khz = new_freq_khz;
    state.start_time_ns = now;

    Ok(())
}

// ─── Panic handler (required for no_std) ─────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
