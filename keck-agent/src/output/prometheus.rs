// SPDX-License-Identifier: Apache-2.0

//! Prometheus metrics exporter.
//!
//! Exposes power attribution data as Prometheus metrics on /metrics.
//! Unlike Kepler, we expose aggregated metrics by default (pod/namespace level)
//! and only expose process-level metrics when explicitly requested (Full profile).
//!
//! Key metrics:
//! - power_node_watts{component} — node-level power by component
//! - power_pod_watts{namespace, pod, component} — pod-level power
//! - power_namespace_watts{namespace, component} — namespace-level power
//! - power_reconciliation_error_ratio — attribution quality indicator
//! - power_attribution_method — which model is being used

use crate::attribution::AttributionSnapshot;

/// Prometheus metrics exporter.
pub struct PrometheusExporter {
    /// Whether to expose process-level metrics (high cardinality)
    expose_process_metrics: bool,
}

impl PrometheusExporter {
    pub fn new(expose_process_metrics: bool) -> Self {
        Self {
            expose_process_metrics,
        }
    }

    /// Update metrics from a new attribution snapshot.
    ///
    /// Called once per collection interval. Updates gauge values
    /// in the Prometheus registry.
    pub fn update(&self, snapshot: &AttributionSnapshot) {
        // TODO: Implement with prometheus-client crate
        //
        // Node-level gauges:
        //   power_node_watts{component="cpu"}     = snapshot.node.measured.cpu_uw / 1e6
        //   power_node_watts{component="memory"}  = snapshot.node.measured.memory_uw / 1e6
        //   power_node_watts{component="gpu"}     = snapshot.node.measured.gpu_uw / 1e6
        //   power_node_idle_watts                 = snapshot.idle_power.total_uw() / 1e6
        //   power_node_platform_watts             = snapshot.node.platform_uw / 1e6
        //
        // Reconciliation:
        //   power_reconciliation_error_ratio      = snapshot.reconciliation.error_ratio
        //   power_reconciliation_unaccounted_watts = snapshot.reconciliation.unaccounted_uw / 1e6
        //
        // Pod-level gauges (reset and re-set each interval):
        //   for pod in snapshot.pods:
        //     power_pod_watts{ns, pod, component="cpu"}    = pod.power.cpu_uw / 1e6
        //     power_pod_watts{ns, pod, component="memory"} = pod.power.memory_uw / 1e6
        //     power_pod_watts{ns, pod, component="gpu"}    = pod.power.gpu_uw / 1e6
        //
        // Namespace-level gauges:
        //   for ns in snapshot.namespaces:
        //     power_namespace_watts{ns, component} = ...
        //
        // Process-level (only in Full profile):
        //   if self.expose_process_metrics:
        //     for proc in snapshot.processes + pod processes:
        //       power_process_watts{pid, comm, component} = ...

        let _ = snapshot; // suppress unused warning until implemented
    }

    /// Render metrics in Prometheus text format.
    ///
    /// Called by the HTTP handler when Prometheus scrapes /metrics.
    pub fn render(&self) -> String {
        // TODO: Render from prometheus-client registry
        String::from("# No metrics yet\n")
    }
}
