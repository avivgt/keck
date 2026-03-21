// SPDX-License-Identifier: Apache-2.0

//! Layer 0: Hardware signal collection.
//!
//! Reads power and energy data from all available hardware sources,
//! manages tiered polling, and reconciles component measurements
//! against platform (PSU) ground truth.
//!
//! Every reading is tagged with:
//! - Component: what physical component (CPU, Memory, GPU, NIC, Storage, Platform)
//! - Granularity: at what level (Node, Socket, Core, Device)
//! - ReadingType: how it was obtained (Measured, Estimated, Derived)
//!
//! Consumers upstream know exactly what they're working with.

mod collector;
mod gpu;
mod hwmon;
mod platform;
mod rapl;

pub use collector::{HardwareCollector, CollectorConfig, Reconciliation};

use std::fmt;
use std::time::{Duration, Instant};

use thiserror::Error;

// ─── Core Types ──────────────────────────────────────────────────

/// How this reading was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadingType {
    /// Direct electrical measurement (PSU shunt, GPU on-board sensor)
    Measured,
    /// Hardware-internal power model (RAPL, firmware estimate)
    Estimated,
    /// Calculated by us from other signals
    Derived,
}

/// What physical component this reading covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Component {
    Cpu,
    Memory,
    Gpu,
    Nic,
    Storage,
    Fan,
    /// PSU input — the whole server. This is the ground truth anchor.
    Platform,
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::Memory => write!(f, "memory"),
            Self::Gpu => write!(f, "gpu"),
            Self::Nic => write!(f, "nic"),
            Self::Storage => write!(f, "storage"),
            Self::Fan => write!(f, "fan"),
            Self::Platform => write!(f, "platform"),
        }
    }
}

/// At what physical level this reading is measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    /// Whole server
    Node,
    /// CPU socket (index 0..N)
    Socket(u8),
    /// Individual core (socket, core)
    Core(u8, u16),
    /// Discrete device (GPU 0, NIC 1, etc.)
    Device(u8),
}

/// Unique identifier for a power source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(pub String);

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A single power/energy reading from hardware.
///
/// Carries BOTH energy (cumulative counter) and power (instantaneous)
/// when available. The attribution engine decides which to use.
#[derive(Clone, Debug)]
pub struct PowerReading {
    pub source_id: SourceId,
    pub timestamp: Instant,
    pub component: Component,
    pub granularity: Granularity,
    pub reading_type: ReadingType,

    /// Cumulative energy counter in microjoules.
    /// For counter-based sources (RAPL). None for power-only sources.
    pub energy_uj: Option<u64>,

    /// Instantaneous power in microwatts.
    /// For power-based sources (Redfish, NVML). None for counter-only.
    pub power_uw: Option<u64>,

    /// Counter max before wraparound. 0 if not applicable.
    pub max_energy_uj: u64,
}

/// Errors from power source operations.
#[derive(Debug, Error)]
pub enum SourceError {
    #[error("source unavailable: {0}")]
    Unavailable(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("read error: {0}")]
    Read(#[from] std::io::Error),

    #[error("parse error: {0}")]
    Parse(String),
}

/// A hardware power source that can be discovered and read.
///
/// Implementations: RAPL zones, hwmon sensors, NVML GPUs, Redfish PSU.
pub trait PowerSource: Send + Sync {
    /// Unique identifier (e.g., "rapl:package:0", "nvml:gpu:1")
    fn id(&self) -> &SourceId;

    /// Human-readable name for logging
    fn name(&self) -> &str;

    /// What component and granularity
    fn component(&self) -> Component;
    fn granularity(&self) -> Granularity;
    fn reading_type(&self) -> ReadingType;

    /// Read current value. Returns both energy and power when available.
    fn read(&self) -> Result<PowerReading, SourceError>;

    /// Default polling interval. Can be overridden by config.
    fn default_poll_interval(&self) -> Duration;
}

/// Discover all available power sources on this machine.
///
/// Probes sysfs, device APIs, and BMC endpoints.
/// Returns only sources that are readable (silently skips unavailable ones).
pub fn discover_sources() -> Vec<Box<dyn PowerSource>> {
    let mut sources: Vec<Box<dyn PowerSource>> = Vec::new();

    // CPU: RAPL domains (package, core, uncore, psys)
    match rapl::discover() {
        Ok(rapl_sources) => {
            log::info!("Discovered {} RAPL source(s)", rapl_sources.len());
            for s in &rapl_sources {
                log::info!("  {}: {} ({:?})", s.id(), s.name(), s.granularity());
            }
            sources.extend(rapl_sources.into_iter().map(|s| Box::new(s) as Box<dyn PowerSource>));
        }
        Err(e) => log::warn!("RAPL discovery failed: {}", e),
    }

    // Memory: RAPL DRAM domain
    match rapl::discover_dram() {
        Ok(dram_sources) => {
            log::info!("Discovered {} DRAM RAPL source(s)", dram_sources.len());
            sources.extend(dram_sources.into_iter().map(|s| Box::new(s) as Box<dyn PowerSource>));
        }
        Err(e) => log::debug!("DRAM RAPL not available: {}", e),
    }

    // hwmon: voltage/current/power sensors
    match hwmon::discover() {
        Ok(hwmon_sources) => {
            log::info!("Discovered {} hwmon source(s)", hwmon_sources.len());
            for s in &hwmon_sources {
                log::info!("  {}: {} ({:?})", s.id(), s.name(), s.component());
            }
            sources.extend(hwmon_sources.into_iter().map(|s| Box::new(s) as Box<dyn PowerSource>));
        }
        Err(e) => log::debug!("hwmon discovery failed: {}", e),
    }

    // GPU: NVIDIA NVML
    match gpu::discover() {
        Ok(gpu_sources) => {
            log::info!("Discovered {} GPU source(s)", gpu_sources.len());
            sources.extend(gpu_sources.into_iter().map(|s| Box::new(s) as Box<dyn PowerSource>));
        }
        Err(e) => log::debug!("GPU discovery failed: {}", e),
    }

    // Platform: Redfish/IPMI
    match platform::discover() {
        Ok(platform_sources) => {
            log::info!("Discovered {} platform source(s)", platform_sources.len());
            sources.extend(platform_sources);
        }
        Err(e) => log::debug!("Platform source discovery failed: {}", e),
    }

    log::info!(
        "Total: {} power source(s) discovered",
        sources.len()
    );

    sources
}
