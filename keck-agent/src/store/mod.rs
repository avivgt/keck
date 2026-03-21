// SPDX-License-Identifier: Apache-2.0

//! Local store: bounded ring buffer for attribution snapshots.
//!
//! Keeps two levels of data:
//! - Detail ring: full process-level snapshots (for drill-down queries)
//! - Summary ring: pod-level summaries (for longer retention)
//!
//! Memory is bounded by the agent profile. When the buffer is full,
//! oldest entries are evicted.
//!
//! The store supports queries from the local query API (Layer 2 output)
//! without recomputation — we serve directly from stored snapshots.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::attribution::{AttributionSnapshot, PodPower, ProcessPower, Reconciliation};

/// Compact pod summary for longer retention and upstream reporting.
#[derive(Clone, Debug)]
pub struct PodSummary {
    pub timestamp: Instant,
    pub pod_uid: String,
    pub name: String,
    pub namespace: String,
    pub cpu_uw: u64,
    pub memory_uw: u64,
    pub gpu_uw: u64,
    pub total_uw: u64,
}

impl From<&PodPower> for PodSummary {
    fn from(pod: &PodPower) -> Self {
        Self {
            timestamp: Instant::now(),
            pod_uid: pod.pod_uid.clone(),
            name: pod.name.clone(),
            namespace: pod.namespace.clone(),
            cpu_uw: pod.power.cpu_uw,
            memory_uw: pod.power.memory_uw,
            gpu_uw: pod.power.gpu_uw,
            total_uw: pod.power.total_uw(),
        }
    }
}

/// Agent profile determines store capacity.
#[derive(Clone, Copy, Debug)]
pub enum StoreProfile {
    /// Edge: ~10MB budget
    Minimal {
        detail_capacity: usize,  // ~100 snapshots
        summary_capacity: usize, // ~5000 summaries
    },
    /// Standard cluster node: ~50MB budget
    Standard {
        detail_capacity: usize,  // ~500 snapshots
        summary_capacity: usize, // ~50000 summaries
    },
    /// High-fidelity: ~200MB budget
    Full {
        detail_capacity: usize,  // ~2000 snapshots
        summary_capacity: usize, // ~100000 summaries
    },
}

impl StoreProfile {
    pub fn minimal() -> Self {
        Self::Minimal {
            detail_capacity: 100,
            summary_capacity: 5_000,
        }
    }

    pub fn standard() -> Self {
        Self::Standard {
            detail_capacity: 500,
            summary_capacity: 50_000,
        }
    }

    pub fn full() -> Self {
        Self::Full {
            detail_capacity: 2_000,
            summary_capacity: 100_000,
        }
    }

    fn detail_capacity(&self) -> usize {
        match self {
            Self::Minimal { detail_capacity, .. } => *detail_capacity,
            Self::Standard { detail_capacity, .. } => *detail_capacity,
            Self::Full { detail_capacity, .. } => *detail_capacity,
        }
    }

    fn summary_capacity(&self) -> usize {
        match self {
            Self::Minimal { summary_capacity, .. } => *summary_capacity,
            Self::Standard { summary_capacity, .. } => *summary_capacity,
            Self::Full { summary_capacity, .. } => *summary_capacity,
        }
    }
}

/// Local store with bounded ring buffers.
pub struct LocalStore {
    /// Full snapshots for drill-down queries
    detail_ring: VecDeque<AttributionSnapshot>,

    /// Compact pod summaries for longer retention
    summary_ring: VecDeque<PodSummary>,

    /// Pending summaries to send upstream (not yet acknowledged)
    outbox: VecDeque<PodSummary>,

    profile: StoreProfile,
}

impl LocalStore {
    pub fn new(profile: StoreProfile) -> Self {
        Self {
            detail_ring: VecDeque::with_capacity(profile.detail_capacity()),
            summary_ring: VecDeque::with_capacity(profile.summary_capacity()),
            outbox: VecDeque::new(),
            profile,
        }
    }

    /// Store a new attribution snapshot.
    ///
    /// - Full snapshot goes to detail ring (bounded, oldest evicted)
    /// - Pod summaries go to summary ring and outbox
    pub fn push(&mut self, snapshot: AttributionSnapshot) {
        // Extract pod summaries before storing
        let summaries: Vec<PodSummary> = snapshot
            .pods
            .iter()
            .map(PodSummary::from)
            .collect();

        // Store full snapshot (evict oldest if at capacity)
        if self.detail_ring.len() >= self.profile.detail_capacity() {
            self.detail_ring.pop_front();
        }
        self.detail_ring.push_back(snapshot);

        // Store summaries
        for summary in summaries {
            if self.summary_ring.len() >= self.profile.summary_capacity() {
                self.summary_ring.pop_front();
            }
            self.outbox.push_back(summary.clone());
            self.summary_ring.push_back(summary);
        }
    }

    /// Get the latest snapshot (for Prometheus metrics, current state queries).
    pub fn latest(&self) -> Option<&AttributionSnapshot> {
        self.detail_ring.back()
    }

    /// Query process detail for a specific pod in recent history.
    /// Used by the drill-down query API.
    pub fn query_pod_processes(
        &self,
        pod_uid: &str,
        since: Instant,
    ) -> Vec<&ProcessPower> {
        // Search recent snapshots for processes belonging to this pod
        // We look at the pod's containers' processes
        self.detail_ring
            .iter()
            .rev()
            .take_while(|s| s.timestamp >= since)
            .flat_map(|s| {
                s.pods
                    .iter()
                    .filter(|p| p.pod_uid == pod_uid)
                    .flat_map(|p| p.containers.iter())
                    .flat_map(|c| c.processes.iter())
            })
            .collect()
    }

    /// Query pod summaries for a namespace in a time range.
    pub fn query_namespace_pods(
        &self,
        namespace: &str,
        since: Instant,
    ) -> Vec<&PodSummary> {
        self.summary_ring
            .iter()
            .rev()
            .take_while(|s| s.timestamp >= since)
            .filter(|s| s.namespace == namespace)
            .collect()
    }

    /// Get reconciliation history (for monitoring attribution quality).
    pub fn reconciliation_history(&self, count: usize) -> Vec<&Reconciliation> {
        self.detail_ring
            .iter()
            .rev()
            .take(count)
            .map(|s| &s.reconciliation)
            .collect()
    }

    /// Drain pending outbox for upstream reporting.
    /// Returns summaries that need to be sent to the cluster controller.
    pub fn drain_outbox(&mut self) -> Vec<PodSummary> {
        self.outbox.drain(..).collect()
    }

    /// Put summaries back in outbox if upstream send failed.
    /// Used for retry on network failure.
    pub fn requeue(&mut self, summaries: Vec<PodSummary>) {
        for s in summaries.into_iter().rev() {
            self.outbox.push_front(s);
        }
    }

    /// Current memory usage estimate in bytes.
    pub fn estimated_memory(&self) -> usize {
        // Rough estimate: each snapshot ~1KB + processes * ~200B
        let detail_est = self.detail_ring.len() * 4096; // ~4KB per snapshot avg
        let summary_est = self.summary_ring.len() * 128; // ~128B per summary
        let outbox_est = self.outbox.len() * 128;
        detail_est + summary_est + outbox_est
    }
}
