// SPDX-License-Identifier: Apache-2.0

//! Layer 0: Hardware signal collection.

mod gpu;
mod hwmon;
mod platform;
mod rapl;

use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadingType {
    Measured,
    Estimated,
    Derived,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    Node,
    Socket(u8),
    Core(u8, u16),
    Device(u8),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(pub String);

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug)]
pub struct PowerReading {
    pub source_id: SourceId,
    pub timestamp: Instant,
    pub component: Component,
    pub granularity: Granularity,
    pub reading_type: ReadingType,
    pub energy_uj: Option<u64>,
    pub power_uw: Option<u64>,
    pub max_energy_uj: u64,
}

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

pub trait PowerSource: Send + Sync {
    fn id(&self) -> &SourceId;
    fn name(&self) -> &str;
    fn component(&self) -> Component;
    fn granularity(&self) -> Granularity;
    fn reading_type(&self) -> ReadingType;
    fn read(&self) -> Result<PowerReading, SourceError>;
    fn default_poll_interval(&self) -> Duration;
}

/// Get the sysfs root — /host/sys in containers, /sys on bare metal.
pub fn sysfs_root() -> &'static str {
    if Path::new("/host/sys/class").exists() {
        "/host/sys"
    } else {
        "/sys"
    }
}

/// Get the procfs root — /host/proc in containers, /proc on bare metal.
pub fn procfs_root() -> &'static str {
    if Path::new("/host/proc/1").exists() {
        "/host/proc"
    } else {
        "/proc"
    }
}

/// Discover all available power sources.
pub fn discover_sources() -> Vec<Box<dyn PowerSource>> {
    let mut sources: Vec<Box<dyn PowerSource>> = Vec::new();

    let sys = sysfs_root();
    log::info!("Using sysfs root: {}", sys);

    match rapl::discover(sys) {
        Ok(rapl_sources) => {
            log::info!("Discovered {} RAPL source(s)", rapl_sources.len());
            for s in &rapl_sources {
                log::info!("  {}: {} ({:?})", s.id(), s.name(), s.granularity());
            }
            sources.extend(rapl_sources.into_iter().map(|s| Box::new(s) as Box<dyn PowerSource>));
        }
        Err(e) => log::warn!("RAPL discovery failed: {}", e),
    }

    match rapl::discover_dram(sys) {
        Ok(dram_sources) => {
            log::info!("Discovered {} DRAM RAPL source(s)", dram_sources.len());
            sources.extend(dram_sources.into_iter().map(|s| Box::new(s) as Box<dyn PowerSource>));
        }
        Err(e) => log::debug!("DRAM RAPL not available: {}", e),
    }

    match hwmon::discover(sys) {
        Ok(hwmon_sources) => {
            log::info!("Discovered {} hwmon source(s)", hwmon_sources.len());
            sources.extend(hwmon_sources.into_iter().map(|s| Box::new(s) as Box<dyn PowerSource>));
        }
        Err(e) => log::debug!("hwmon discovery failed: {}", e),
    }

    match gpu::discover() {
        Ok(gpu_sources) => {
            log::info!("Discovered {} GPU source(s)", gpu_sources.len());
            sources.extend(gpu_sources.into_iter().map(|s| Box::new(s) as Box<dyn PowerSource>));
        }
        Err(e) => log::debug!("GPU discovery failed: {}", e),
    }

    match platform::discover() {
        Ok(platform_sources) => {
            log::info!("Discovered {} platform source(s)", platform_sources.len());
            sources.extend(platform_sources);
        }
        Err(e) => log::debug!("Platform source discovery failed: {}", e),
    }

    log::info!("Total: {} power source(s) discovered", sources.len());
    sources
}
