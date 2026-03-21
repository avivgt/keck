// SPDX-License-Identifier: Apache-2.0

//! Attribution engine: converts raw observations into per-workload power.
//!
//! This is the core algorithm that differentiates us from Kepler.
//! Instead of: node_energy × (pid_cpu_time / total_cpu_time)
//! We do:      core_energy × weighted_model(pid_on_core) for each core
//!
//! The attribution flows bottom-up:
//!   Core energy → per-process → per-container → per-pod → per-namespace
//!
//! Each level sums to the level above. Error bounds are computed at
//! every level by reconciling against measured totals.

pub mod engine;
pub mod model;
mod types;

pub use engine::AttributionEngine;
pub use model::AttributionModel;
pub use types::*;
