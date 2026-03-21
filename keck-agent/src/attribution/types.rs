// SPDX-License-Identifier: Apache-2.0

//! Types for power attribution results.
//!
//! These types flow from the attribution engine through K8s enrichment
//! to the output sinks. They carry both the attributed power values
//! and metadata about how the attribution was computed.

use std::collections::HashMap;
use std::time::Instant;

/// How a power reading was obtained — propagated from Layer 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadingType {
    /// Direct electrical measurement (PSU, GPU shunt, VR sensor)
    Measured,
    /// Hardware-internal model (RAPL, firmware estimate)
    Estimated,
    /// Calculated by our attribution model
    Derived,
}

/// Physical component that consumed the energy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Component {
    Cpu,
    Memory,
    Gpu,
    Nic,
    Storage,
    Fan,
    Platform,
}

/// Per-component power breakdown in microwatts.
#[derive(Clone, Debug, Default)]
pub struct PowerBreakdown {
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    pub nic_uw: u64,
    pub storage_uw: u64,
}

impl PowerBreakdown {
    pub fn total_uw(&self) -> u64 {
        self.cpu_uw + self.memory_uw + self.gpu_uw + self.nic_uw + self.storage_uw
    }
}

/// Per-core attribution detail for a single process.
/// This is the finest granularity — used for drill-down queries.
#[derive(Clone, Debug)]
pub struct CoreAttribution {
    /// CPU core index
    pub core: u32,
    /// Time this process ran on this core (nanoseconds)
    pub time_ns: u64,
    /// Frequency the core was running at (weighted average KHz)
    pub avg_freq_khz: u32,
    /// Attributed energy on this core (microjoules)
    pub energy_uj: u64,
    /// Hardware counters attributed to this process on this core
    pub instructions: u64,
    pub cycles: u64,
    pub cache_misses: u64,
}

/// Attributed power for a single process.
#[derive(Clone, Debug)]
pub struct ProcessPower {
    pub pid: u32,
    pub comm: String,
    pub cgroup_id: u64,

    /// Total power across all components
    pub power: PowerBreakdown,

    /// Per-core breakdown (for drill-down queries)
    /// Only populated in Full profile; empty in Minimal/Standard
    pub core_detail: Vec<CoreAttribution>,

    /// How the CPU attribution was computed
    pub attribution_method: AttributionMethod,
}

/// How the CPU power was attributed to this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributionMethod {
    /// Per-core time + frequency + hardware counters (best accuracy)
    FullModel,
    /// Per-core time + frequency weighting (no HW counters available)
    FrequencyWeighted,
    /// Simple CPU time ratio (fallback when eBPF unavailable)
    CpuTimeRatio,
}

/// Attributed power for a container (aggregated from processes).
#[derive(Clone, Debug)]
pub struct ContainerPower {
    pub container_id: String,
    pub name: String,
    pub cgroup_id: u64,

    pub power: PowerBreakdown,

    /// Process-level breakdown (for drill-down)
    pub processes: Vec<ProcessPower>,
}

/// Attributed power for a pod (aggregated from containers).
#[derive(Clone, Debug)]
pub struct PodPower {
    pub pod_uid: String,
    pub name: String,
    pub namespace: String,

    pub power: PowerBreakdown,

    /// Container-level breakdown
    pub containers: Vec<ContainerPower>,
}

/// Attributed power for a namespace (aggregated from pods).
#[derive(Clone, Debug)]
pub struct NamespacePower {
    pub namespace: String,
    pub power: PowerBreakdown,
    pub pod_count: usize,
}

/// Complete attribution snapshot for one collection interval.
///
/// This is the primary output of Layer 2 — consumed by the
/// local store, output sinks, and query API.
#[derive(Clone, Debug)]
pub struct AttributionSnapshot {
    pub timestamp: Instant,
    pub interval_ns: u64,

    /// Node-level totals
    pub node: NodePower,

    /// Per-process attribution (full detail)
    pub processes: Vec<ProcessPower>,

    /// Per-pod attribution (aggregated, used for upstream reporting)
    pub pods: Vec<PodPower>,

    /// Per-namespace attribution (aggregated)
    pub namespaces: Vec<NamespacePower>,

    /// Idle power: energy consumed by cores with no userspace work
    pub idle_power: PowerBreakdown,

    /// Reconciliation against platform measurement
    pub reconciliation: Reconciliation,
}

/// Node-level power totals with reconciliation.
#[derive(Clone, Debug)]
pub struct NodePower {
    /// Per-component power (from hardware sources)
    pub measured: PowerBreakdown,

    /// Platform total (PSU input, if available)
    pub platform_uw: Option<u64>,

    /// Sum of all attributed workload power + idle
    pub attributed_total_uw: u64,
}

/// Reconciliation of attributed power against measured platform power.
#[derive(Clone, Debug)]
pub struct Reconciliation {
    /// Platform (PSU) measured power in microwatts
    pub platform_uw: Option<u64>,

    /// Sum of all component measurements
    pub component_sum_uw: u64,

    /// Sum of all process attributions + idle
    pub attributed_sum_uw: u64,

    /// Unaccounted power: platform - component_sum
    /// Positive = we're underreporting, negative = overreporting
    pub unaccounted_uw: i64,

    /// Error ratio: |unaccounted| / platform
    pub error_ratio: f64,
}
