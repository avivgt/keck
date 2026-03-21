// SPDX-License-Identifier: Apache-2.0

//! RAPL (Running Average Power Limit) sysfs reader.
//!
//! Reads energy counters from /sys/class/powercap/intel-rapl:*/
//! Works on both Intel (since Sandy Bridge) and AMD (since Zen).
//!
//! RAPL domains:
//!   package (intel-rapl:N)         — entire socket: cores + uncore + iGPU
//!   core    (intel-rapl:N:0)       — CPU cores only (PP0)
//!   uncore  (intel-rapl:N:1)       — iGPU, memory controller (PP1)
//!   dram    (intel-rapl:N:2)       — DRAM DIMMs attached to this socket
//!   psys    (intel-rapl:N:3)       — entire SoC platform (Skylake+)
//!
//! Note: RAPL is NOT a direct measurement. It's Intel/AMD's firmware power
//! model running on internal counters. Accuracy is typically ±5-15%.
//! We tag all RAPL readings as ReadingType::Estimated.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{
    Component, Granularity, PowerReading, PowerSource, ReadingType, SourceError, SourceId,
};

/// A single RAPL energy zone (one sysfs entry).
pub struct RaplSource {
    id: SourceId,
    name: String,
    path: PathBuf,
    component: Component,
    granularity: Granularity,
    max_energy_uj: u64,
}

impl RaplSource {
    fn new(
        domain: &str,
        socket: u8,
        path: PathBuf,
        component: Component,
    ) -> Result<Self, SourceError> {
        let id_str = format!("rapl:{}:{}", domain, socket);

        // Read max energy range for wraparound handling
        let max_path = path.join("max_energy_range_uj");
        let max_energy_uj = read_u64_file(&max_path).unwrap_or(0);

        // Read the zone name for logging
        let name_path = path.join("name");
        let zone_name = fs::read_to_string(&name_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| domain.to_string());

        Ok(Self {
            id: SourceId(id_str),
            name: format!("RAPL {} (socket {}): {}", domain, socket, zone_name),
            path,
            component,
            granularity: Granularity::Socket(socket),
            max_energy_uj,
        })
    }
}

impl PowerSource for RaplSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn component(&self) -> Component {
        self.component
    }

    fn granularity(&self) -> Granularity {
        self.granularity
    }

    fn reading_type(&self) -> ReadingType {
        // RAPL is a firmware model, not a direct electrical measurement
        ReadingType::Estimated
    }

    fn read(&self) -> Result<PowerReading, SourceError> {
        let energy_path = self.path.join("energy_uj");
        let energy_uj = read_u64_file(&energy_path)?;

        Ok(PowerReading {
            source_id: self.id.clone(),
            timestamp: Instant::now(),
            component: self.component,
            granularity: self.granularity,
            reading_type: ReadingType::Estimated,
            energy_uj: Some(energy_uj),
            power_uw: None, // RAPL gives energy counters, not instantaneous power
            max_energy_uj: self.max_energy_uj,
        })
    }

    fn default_poll_interval(&self) -> Duration {
        // RAPL counters update every ~1ms on modern CPUs.
        // 100ms gives good resolution without excessive syscalls.
        Duration::from_millis(100)
    }
}

/// Discover RAPL CPU package domains.
///
/// Scans /sys/class/powercap/intel-rapl:N for each socket.
/// Returns one source per socket's package domain.
pub fn discover() -> Result<Vec<RaplSource>, SourceError> {
    let powercap = Path::new("/sys/class/powercap");
    if !powercap.exists() {
        return Err(SourceError::Unavailable(
            "/sys/class/powercap not found (no RAPL support)".into(),
        ));
    }

    let mut sources = Vec::new();

    // Enumerate top-level RAPL domains: intel-rapl:0, intel-rapl:1, ...
    let entries = fs::read_dir(powercap)?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();

        // Match intel-rapl:N (top-level package domain)
        if !name.starts_with("intel-rapl:") {
            continue;
        }
        // Skip sub-domains (intel-rapl:N:M) — handled separately
        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() != 2 {
            continue;
        }

        let socket: u8 = match parts[1].parse() {
            Ok(n) => n,
            Err(_) => continue,
        };

        let path = entry.path();

        // Verify we can read the energy counter
        let energy_path = path.join("energy_uj");
        if !energy_path.exists() {
            continue;
        }

        match RaplSource::new("package", socket, path, Component::Cpu) {
            Ok(source) => sources.push(source),
            Err(e) => log::warn!("Failed to init RAPL package:{}: {}", socket, e),
        }
    }

    if sources.is_empty() {
        return Err(SourceError::Unavailable("No RAPL package domains found".into()));
    }

    // Sort by socket index for deterministic ordering
    sources.sort_by_key(|s| match s.granularity {
        Granularity::Socket(n) => n,
        _ => 0,
    });

    Ok(sources)
}

/// Discover RAPL DRAM domains (memory energy).
///
/// DRAM is typically intel-rapl:N:2 (sub-domain of each package).
/// Not available on all CPUs.
pub fn discover_dram() -> Result<Vec<RaplSource>, SourceError> {
    let powercap = Path::new("/sys/class/powercap");
    if !powercap.exists() {
        return Err(SourceError::Unavailable("no powercap".into()));
    }

    let mut sources = Vec::new();

    let entries = fs::read_dir(powercap)?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();

        // Match intel-rapl:N:M (sub-domains)
        if !name.starts_with("intel-rapl:") {
            continue;
        }
        let parts: Vec<&str> = name.split(':').collect();
        if parts.len() != 3 {
            continue;
        }

        let socket: u8 = match parts[1].parse() {
            Ok(n) => n,
            Err(_) => continue,
        };

        let path = entry.path();

        // Check if this is a DRAM domain by reading the name file
        let name_path = path.join("name");
        let zone_name = match fs::read_to_string(&name_path) {
            Ok(s) => s.trim().to_string(),
            Err(_) => continue,
        };

        if zone_name != "dram" {
            continue;
        }

        let energy_path = path.join("energy_uj");
        if !energy_path.exists() {
            continue;
        }

        match RaplSource::new("dram", socket, path, Component::Memory) {
            Ok(source) => sources.push(source),
            Err(e) => log::warn!("Failed to init RAPL dram:{}: {}", socket, e),
        }
    }

    if sources.is_empty() {
        return Err(SourceError::Unavailable("No RAPL DRAM domains found".into()));
    }

    Ok(sources)
}

/// Read a u64 from a sysfs file.
fn read_u64_file(path: &Path) -> Result<u64, SourceError> {
    let content = fs::read_to_string(path)?;
    content
        .trim()
        .parse::<u64>()
        .map_err(|e| SourceError::Parse(format!("{}: {}", path.display(), e)))
}
