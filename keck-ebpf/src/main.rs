// SPDX-License-Identifier: Apache-2.0

//! eBPF programs for kernel-level power observation.
//!
//! Runs inside the Linux kernel, attached to scheduler tracepoints.
//! Collects per-PID per-core CPU time and per-core frequency residency.

#![no_std]
#![no_main]

use aya_ebpf::{
    helpers::{bpf_get_current_cgroup_id, bpf_get_current_pid_tgid, bpf_get_smp_processor_id, bpf_ktime_get_ns},
    macros::{kprobe, kretprobe, map, tracepoint},
    maps::{HashMap, PerfEventArray, PerCpuArray},
    programs::{ProbeContext, RetProbeContext, TracePointContext},
};
use keck_common::{
    CpuFreqKey, CpuFreqState, CpuFreqTime, CpuSchedState, MAX_CPU_FREQ_ENTRIES,
    MAX_PID_CGROUP_ENTRIES, MAX_PID_CPU_ENTRIES, MAX_PID_HW_COUNTER_ENTRIES,
    MAX_PID_NET_ENTRIES, PidCgroupValue, PidCpuCounterKey, PidCpuCounters,
    PidCpuKey, PidCpuTime, PidNetBytes,
};

// ─── BPF Maps ────────────────────────────────────────────────────

#[map]
static PID_CPU_TIME: HashMap<PidCpuKey, PidCpuTime> =
    HashMap::with_max_entries(MAX_PID_CPU_ENTRIES, 0);

#[map]
static CPU_SCHED_STATE: PerCpuArray<CpuSchedState> = PerCpuArray::with_max_entries(1, 0);

#[map]
static CPU_FREQ_TIME: HashMap<CpuFreqKey, CpuFreqTime> =
    HashMap::with_max_entries(MAX_CPU_FREQ_ENTRIES, 0);

#[map]
static CPU_FREQ_STATE: PerCpuArray<CpuFreqState> = PerCpuArray::with_max_entries(1, 0);

#[map]
static PID_CGROUP: HashMap<u32, PidCgroupValue> =
    HashMap::with_max_entries(MAX_PID_CGROUP_ENTRIES, 0);

#[map]
static PID_NET_BYTES: HashMap<u32, PidNetBytes> =
    HashMap::with_max_entries(MAX_PID_NET_ENTRIES, 0);

// Per-PID per-core hardware counter deltas (instructions, cycles, cache misses).
// Populated by sched_switch when perf event FDs are attached to the PERF_* maps.
#[map]
static PID_HW_COUNTERS: HashMap<PidCpuCounterKey, PidCpuCounters> =
    HashMap::with_max_entries(MAX_PID_HW_COUNTER_ENTRIES, 0);

// PerfEventArray maps — userspace attaches perf_event FDs (one per CPU).
// The eBPF program reads hardware counters via bpf_perf_event_read().
#[map]
static PERF_INSTRUCTIONS: PerfEventArray<u64> = PerfEventArray::new(0);

#[map]
static PERF_CYCLES: PerfEventArray<u64> = PerfEventArray::new(0);

#[map]
static PERF_CACHE_MISSES: PerfEventArray<u64> = PerfEventArray::new(0);

// Flag: set to 1 by userspace when perf FDs are attached.
// When 0, sched_switch skips PMC reads (graceful degradation).
#[map]
static PMC_ENABLED: PerCpuArray<u32> = PerCpuArray::with_max_entries(1, 0);

// ─── sched_switch ────────────────────────────────────────────────

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
    let cpu = unsafe { bpf_get_smp_processor_id() };

    let prev_pid: u32 = unsafe { ctx.read_at(24)? };
    let next_pid: u32 = unsafe { ctx.read_at(56)? };

    // Check if per-PID PMC tracking is enabled
    let pmc_active = PMC_ENABLED.get(0).map(|v| *v != 0).unwrap_or(false);

    // Get this CPU's scheduler state
    let state_ptr = CPU_SCHED_STATE.get_ptr_mut(0).ok_or(0i64)?;
    let state = unsafe { &mut *state_ptr };

    // Account time (and optionally PMC deltas) for the outgoing task
    if state.current_pid != 0 && state.start_time_ns != 0 {
        let elapsed = now.saturating_sub(state.start_time_ns);
        if elapsed > 0 {
            let key = PidCpuKey {
                cpu,
                pid: prev_pid,
            };

            if let Some(val) = unsafe { PID_CPU_TIME.get_ptr_mut(&key) } {
                unsafe { (*val).time_ns += elapsed };
            } else {
                let _ = PID_CPU_TIME.insert(&key, &PidCpuTime { time_ns: elapsed }, 0);
            }

            // Accumulate per-PID hardware counter deltas
            if pmc_active {
                let cur_instr = read_perf_counter(&PERF_INSTRUCTIONS, cpu);
                let cur_cycles = read_perf_counter(&PERF_CYCLES, cpu);
                let cur_misses = read_perf_counter(&PERF_CACHE_MISSES, cpu);

                let delta_instr = cur_instr.saturating_sub(state.start_instructions);
                let delta_cycles = cur_cycles.saturating_sub(state.start_cycles);
                let delta_misses = cur_misses.saturating_sub(state.start_cache_misses);

                if delta_instr > 0 || delta_cycles > 0 {
                    let ckey = PidCpuCounterKey { cpu, pid: prev_pid };
                    if let Some(val) = unsafe { PID_HW_COUNTERS.get_ptr_mut(&ckey) } {
                        unsafe {
                            (*val).instructions += delta_instr;
                            (*val).cycles += delta_cycles;
                            (*val).cache_misses += delta_misses;
                        }
                    } else {
                        let _ = PID_HW_COUNTERS.insert(
                            &ckey,
                            &PidCpuCounters {
                                instructions: delta_instr,
                                cycles: delta_cycles,
                                cache_misses: delta_misses,
                            },
                            0,
                        );
                    }
                }
            }
        }
    }

    // Record the incoming task and snapshot PMC start values
    state.current_pid = next_pid;
    state.start_time_ns = now;

    if pmc_active {
        state.start_instructions = read_perf_counter(&PERF_INSTRUCTIONS, cpu);
        state.start_cycles = read_perf_counter(&PERF_CYCLES, cpu);
        state.start_cache_misses = read_perf_counter(&PERF_CACHE_MISSES, cpu);
    }

    // Map incoming PID to its cgroup
    if next_pid != 0 {
        let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
        let _ = PID_CGROUP.insert(&next_pid, &PidCgroupValue { cgroup_id }, 0);
    }

    Ok(())
}

/// Read a hardware counter from a PerfEventArray map.
/// Returns 0 if the read fails (perf FD not attached for this CPU).
#[inline(always)]
fn read_perf_counter(map: &PerfEventArray<u64>, cpu: u32) -> u64 {
    // bpf_perf_event_read(map, index) — BPF helper #22
    // Reads the hardware counter value from the perf_event FD attached
    // at the given index (CPU number) in the PerfEventArray map.
    let map_ptr = map as *const _ as *mut core::ffi::c_void;
    let result = unsafe {
        // Use BPF_F_INDEX_MASK to read from the current CPU's perf event
        aya_ebpf::helpers::gen::bpf_perf_event_read(map_ptr, cpu as u64)
    };
    if result < 0 {
        0 // Perf event not attached or error — return 0
    } else {
        result as u64
    }
}

// ─── cpu_frequency ───────────────────────────────────────────────

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

            if let Some(val) = unsafe { CPU_FREQ_TIME.get_ptr_mut(&key) } {
                unsafe { (*val).time_ns += elapsed };
            } else {
                let _ = CPU_FREQ_TIME.insert(&key, &CpuFreqTime { time_ns: elapsed }, 0);
            }
        }
    }

    state.current_freq_khz = new_freq_khz;
    state.start_time_ns = now;

    Ok(())
}

// ─── TCP network I/O tracking ────────────────────────────────────

/// kprobe on tcp_sendmsg — tracks bytes sent per PID.
/// int tcp_sendmsg(struct sock *sk, struct msghdr *msg, size_t size)
/// The 3rd argument (size) is the number of bytes being sent.
#[kprobe]
pub fn tcp_sendmsg(ctx: ProbeContext) -> u32 {
    let pid = (unsafe { bpf_get_current_pid_tgid() } >> 32) as u32;
    if pid == 0 { return 0; }

    // 3rd argument = size (bytes to send)
    let size: u64 = match unsafe { ctx.arg(2) } {
        Some(s) => s,
        None => return 0,
    };

    if let Some(val) = unsafe { PID_NET_BYTES.get_ptr_mut(&pid) } {
        unsafe { (*val).tx_bytes += size };
    } else {
        let _ = PID_NET_BYTES.insert(&pid, &PidNetBytes { tx_bytes: size, rx_bytes: 0 }, 0);
    }
    0
}

/// kretprobe on tcp_recvmsg — tracks actual bytes received per PID.
/// int tcp_recvmsg(struct sock *sk, struct msghdr *msg, size_t len, ...)
/// Returns the number of bytes actually received (or negative error).
#[kretprobe]
pub fn tcp_recvmsg(ctx: RetProbeContext) -> u32 {
    let pid = (unsafe { bpf_get_current_pid_tgid() } >> 32) as u32;
    if pid == 0 { return 0; }

    // Return value = actual bytes received (negative on error)
    let ret: i64 = match ctx.ret() {
        Some(r) => r,
        None => return 0,
    };
    if ret <= 0 { return 0; }
    let received = ret as u64;

    if let Some(val) = unsafe { PID_NET_BYTES.get_ptr_mut(&pid) } {
        unsafe { (*val).rx_bytes += received };
    } else {
        let _ = PID_NET_BYTES.insert(&pid, &PidNetBytes { tx_bytes: 0, rx_bytes: received }, 0);
    }
    0
}

// ─── Panic handler ───────────────────────────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
