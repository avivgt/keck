// SPDX-License-Identifier: Apache-2.0

use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use std::sync::atomic::AtomicU64;

#[derive(Clone, Debug, Hash, PartialEq, Eq, prometheus_client::encoding::EncodeLabelSet)]
pub struct ComponentLabel {
    pub component: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, prometheus_client::encoding::EncodeLabelSet)]
pub struct NodeLabel {
    pub node: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, prometheus_client::encoding::EncodeLabelSet)]
pub struct NamespaceComponentLabel {
    pub namespace: String,
    pub component: String,
}

pub struct Metrics {
    registry: Registry,
    pub cluster_power_watts: Family<ComponentLabel, Gauge<f64, AtomicU64>>,
    pub namespace_power_watts: Family<NamespaceComponentLabel, Gauge<f64, AtomicU64>>,
    pub node_power_watts: Family<NodeLabel, Gauge<f64, AtomicU64>>,
    pub node_error_ratio: Family<NodeLabel, Gauge<f64, AtomicU64>>,
    pub agent_last_report_secs: Family<NodeLabel, Gauge<f64, AtomicU64>>,
    pub agent_report_total: Family<NodeLabel, Counter>,
    pub pod_count: Gauge,
    pub node_count: Gauge,
}

impl Metrics {
    pub fn new() -> Self {
        let mut registry = Registry::default();

        let cluster_power_watts = Family::default();
        registry.register(
            "keck_cluster_power_watts",
            "Total cluster power by component",
            cluster_power_watts.clone(),
        );

        let namespace_power_watts = Family::default();
        registry.register(
            "keck_namespace_power_watts",
            "Total namespace power by component",
            namespace_power_watts.clone(),
        );

        let node_power_watts = Family::default();
        registry.register(
            "keck_node_power_watts",
            "Total node power",
            node_power_watts.clone(),
        );

        let node_error_ratio = Family::default();
        registry.register(
            "keck_node_error_ratio",
            "PSU reconciliation error ratio per node",
            node_error_ratio.clone(),
        );

        let agent_last_report_secs = Family::default();
        registry.register(
            "keck_agent_last_report_seconds_ago",
            "Seconds since last agent report per node",
            agent_last_report_secs.clone(),
        );

        let agent_report_total = Family::default();
        registry.register(
            "keck_agent_report_total",
            "Total reports received per node",
            agent_report_total.clone(),
        );

        let pod_count = Gauge::default();
        registry.register(
            "keck_pod_count",
            "Number of pods with power data",
            pod_count.clone(),
        );

        let node_count = Gauge::default();
        registry.register(
            "keck_node_count",
            "Number of nodes reporting power data",
            node_count.clone(),
        );

        Self {
            registry,
            cluster_power_watts,
            namespace_power_watts,
            node_power_watts,
            node_error_ratio,
            agent_last_report_secs,
            agent_report_total,
            pod_count,
            node_count,
        }
    }

    pub fn render(&self) -> String {
        let mut buf = String::new();
        encode(&mut buf, &self.registry).unwrap_or_default();
        buf
    }
}
