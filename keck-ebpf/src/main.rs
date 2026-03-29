// SPDX-License-Identifier: Apache-2.0

//! eBPF programs for kernel-level power observation.
//!
//! Runs inside the Linux kernel, attached to scheduler tracepoints.
//! Collects per-PID per-core CPU time and per-core frequency residency.

#![no_std]
#![no_main]

use aya_ebpf::{
    helpers::{bpf_get_current_cgroup_id, bpf_get_current_pid_tgid, bpf_get_smp_processor_id, bpf_ktime_get_ns},
    macros::{kprobe, map, tracepoint},
    maps::{HashMap, PerCpuArray},
    programs::{ProbeContext, TracePointContext},
};
use keck_common::{
    CpuFreqKey, CpuFreqState, CpuFreqTime, CpuSchedState, MAX_CPU_FREQ_ENTRIES,
    MAX_PID_CGROUP_ENTRIES, MAX_PID_CPU_ENTRIES, MAX_PID_NET_ENTRIES,
    PidCgroupValue, PidCpuKey, PidCpuTime, PidNetBytes,
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

    // Get this CPU's scheduler state
    let state_ptr = CPU_SCHED_STATE.get_ptr_mut(0).ok_or(0i64)?;
    let state = unsafe { &mut *state_ptr };

    // Account time for the outgoing task
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
        }
    }

    // Record the incoming task
    state.current_pid = next_pid;
    state.start_time_ns = now;

    // Map incoming PID to its cgroup
    if next_pid != 0 {
        let cgroup_id = unsafe { bpf_get_current_cgroup_id() };
        let _ = PID_CGROUP.insert(&next_pid, &PidCgroupValue { cgroup_id }, 0);
    }

    Ok(())
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

/// kprobe on tcp_recvmsg — tracks bytes received per PID.
/// int tcp_recvmsg(struct sock *sk, struct msghdr *msg, size_t len, ...)
/// The 3rd argument (len) is the buffer size, not actual bytes received.
/// We use it as an approximation; the actual received bytes are in the return value
/// which we'd need a kretprobe for. This gives an upper bound.
#[kprobe]
pub fn tcp_recvmsg(ctx: ProbeContext) -> u32 {
    let pid = (unsafe { bpf_get_current_pid_tgid() } >> 32) as u32;
    if pid == 0 { return 0; }

    let size: u64 = match unsafe { ctx.arg(2) } {
        Some(s) => s,
        None => return 0,
    };

    if let Some(val) = unsafe { PID_NET_BYTES.get_ptr_mut(&pid) } {
        unsafe { (*val).rx_bytes += size };
    } else {
        let _ = PID_NET_BYTES.insert(&pid, &PidNetBytes { tx_bytes: 0, rx_bytes: size }, 0);
    }
    0
}

// ─── Panic handler ───────────────────────────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
