// SPDX-License-Identifier: Apache-2.0

//! Output sinks: how the agent exposes data to the outside world.
//!
//! Three output channels:
//! 1. Prometheus /metrics endpoint (for existing dashboards, backward compat)
//! 2. gRPC upstream (pod-level summaries to cluster controller)
//! 3. Query API (drill-down from cluster controller or CLI)
//!
//! The Prometheus endpoint is optional (can be disabled to reduce overhead).
//! The gRPC upstream is the primary data path.
//! The query API handles on-demand drill-down requests.

mod prometheus;
mod query;

pub use self::prometheus::PrometheusExporter;
pub use query::QueryServer;
