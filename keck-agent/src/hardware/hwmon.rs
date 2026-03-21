// SPDX-License-Identifier: Apache-2.0

//! hwmon power sensor reader.
//!
//! Reads power sensors from /sys/class/hwmon/hwmon*/
//! These are direct measurements from on-board shunt resistors,
//! voltage regulators, or power monitoring ICs.
//!
//! Unlike RAPL (estimated), hwmon power sensors are actual electrical
//! measurements — tagged as ReadingType::Measured.
//!
//! We discover three types of hwmon power sources:
//! 1. Direct power sensors (power1_input in microwatts)
//! 2. Energy counters (energy1_input in microjoules)
//! 3. Voltage × current pairs (in1_input × curr1_input)
//!
//! This implementation focuses on direct power and energy sensors.
//! Voltage × current pairing (the complex part of Kepler's hwmon) is
//! deferred — it requires chip-specific rules and is fragile.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{
    Component, Granularity, PowerReading, PowerSource, ReadingType, SourceError, SourceId,
};

/// An hwmon power or energy sensor.
pub struct HwmonSource {
    id: SourceId,
    display_name: String,
    chip_name: String,
    component: Component,
    sensor: HwmonSensor,
}

/// What kind of hwmon sensor this is.
enum HwmonSensor {
    /// power{N}_input — reads microwatts directly
    Power { path: PathBuf },
    /// energy{N}_input — reads microjoules (cumulative counter)
    Energy { path: PathBuf, max_path: Option<PathBuf> },
}

impl PowerSource for HwmonSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn component(&self) -> Component {
        self.component
    }

    fn granularity(&self) -> Granularity {
        Granularity::Node // Most hwmon sensors are node-level
    }

    fn reading_type(&self) -> ReadingType {
        ReadingType::Measured // hwmon = actual electrical measurement
    }

    fn read(&self) -> Result<PowerReading, SourceError> {
        match &self.sensor {
            HwmonSensor::Power { path } => {
                let power_uw = read_u64_file(path)?;

                Ok(PowerReading {
                    source_id: self.id.clone(),
                    timestamp: Instant::now(),
                    component: self.component,
                    granularity: Granularity::Node,
                    reading_type: ReadingType::Measured,
                    energy_uj: None,
                    power_uw: Some(power_uw),
                    max_energy_uj: 0,
                })
            }
            HwmonSensor::Energy { path, max_path } => {
                let energy_uj = read_u64_file(path)?;
                let max_energy = max_path
                    .as_ref()
                    .and_then(|p| read_u64_file(p).ok())
                    .unwrap_or(0);

                Ok(PowerReading {
                    source_id: self.id.clone(),
                    timestamp: Instant::now(),
                    component: self.component,
                    granularity: Granularity::Node,
                    reading_type: ReadingType::Measured,
                    energy_uj: Some(energy_uj),
                    power_uw: None,
                    max_energy_uj: max_energy,
                })
            }
        }
    }

    fn default_poll_interval(&self) -> Duration {
        Duration::from_millis(500)
    }
}

/// Discover hwmon power and energy sensors.
///
/// Scans /sys/class/hwmon/hwmon* for:
/// - power{N}_input files (direct power reading)
/// - energy{N}_input files (energy counter)
///
/// Classifies component type from the chip name and label.
pub fn discover() -> Result<Vec<HwmonSource>, SourceError> {
    let hwmon_base = Path::new("/sys/class/hwmon");
    if !hwmon_base.exists() {
        return Err(SourceError::Unavailable("no /sys/class/hwmon".into()));
    }

    let mut sources = Vec::new();

    let entries = fs::read_dir(hwmon_base)?;
    for entry in entries {
        let entry = entry?;
        let hwmon_path = entry.path();

        // Read chip name
        let chip_name = fs::read_to_string(hwmon_path.join("name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let hwmon_name = entry.file_name().to_string_lossy().to_string();

        // Scan for power{N}_input sensors
        for n in 1..=16 {
            let power_path = hwmon_path.join(format!("power{}_input", n));
            if !power_path.exists() {
                continue;
            }

            // Try to read label for better identification
            let label = fs::read_to_string(hwmon_path.join(format!("power{}_label", n)))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            // Check if sensor is enabled
            let enable_path = hwmon_path.join(format!("power{}_enable", n));
            if enable_path.exists() {
                if let Ok(val) = fs::read_to_string(&enable_path) {
                    if val.trim() == "0" {
                        continue; // Disabled
                    }
                }
            }

            let component = classify_component(&chip_name, &label);
            let id_str = format!("hwmon:{}:power{}:{}", hwmon_name, n, chip_name);

            sources.push(HwmonSource {
                id: SourceId(id_str),
                display_name: format!(
                    "hwmon {} power{} ({}{})",
                    chip_name,
                    n,
                    if label.is_empty() { "unlabeled" } else { &label },
                    ""
                ),
                chip_name: chip_name.clone(),
                component,
                sensor: HwmonSensor::Power {
                    path: power_path,
                },
            });
        }

        // Scan for energy{N}_input sensors
        for n in 1..=16 {
            let energy_path = hwmon_path.join(format!("energy{}_input", n));
            if !energy_path.exists() {
                continue;
            }

            let label = fs::read_to_string(hwmon_path.join(format!("energy{}_label", n)))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            let max_path = {
                let p = hwmon_path.join(format!("energy{}_max", n));
                if p.exists() { Some(p) } else { None }
            };

            let component = classify_component(&chip_name, &label);
            let id_str = format!("hwmon:{}:energy{}:{}", hwmon_name, n, chip_name);

            sources.push(HwmonSource {
                id: SourceId(id_str),
                display_name: format!("hwmon {} energy{} ({})", chip_name, n, label),
                chip_name: chip_name.clone(),
                component,
                sensor: HwmonSensor::Energy {
                    path: energy_path,
                    max_path,
                },
            });
        }
    }

    if sources.is_empty() {
        return Err(SourceError::Unavailable("No hwmon power sensors found".into()));
    }

    Ok(sources)
}

/// Classify which component an hwmon sensor belongs to based on chip name and label.
fn classify_component(chip_name: &str, label: &str) -> Component {
    let name_lower = chip_name.to_lowercase();
    let label_lower = label.to_lowercase();

    // GPU chips
    if name_lower.contains("nvidia")
        || name_lower.contains("amdgpu")
        || name_lower.contains("radeon")
        || name_lower.contains("xe")
    {
        return Component::Gpu;
    }

    // NIC chips
    if name_lower.contains("mlx")
        || name_lower.contains("ice")
        || name_lower.contains("bnxt")
        || name_lower.contains("i40e")
        || label_lower.contains("network")
        || label_lower.contains("nic")
    {
        return Component::Nic;
    }

    // Storage
    if name_lower.contains("nvme")
        || name_lower.contains("drivetemp")
        || label_lower.contains("disk")
        || label_lower.contains("ssd")
    {
        return Component::Storage;
    }

    // CPU VR sensors
    if label_lower.contains("cpu")
        || label_lower.contains("vcore")
        || label_lower.contains("package")
    {
        return Component::Cpu;
    }

    // Memory
    if label_lower.contains("dram")
        || label_lower.contains("dimm")
        || label_lower.contains("memory")
    {
        return Component::Memory;
    }

    // Default: CPU (most hwmon power sensors are CPU-related)
    Component::Cpu
}

fn read_u64_file(path: &Path) -> Result<u64, SourceError> {
    let content = fs::read_to_string(path)?;
    content
        .trim()
        .parse::<u64>()
        .map_err(|e| SourceError::Parse(format!("{}: {}", path.display(), e)))
}
