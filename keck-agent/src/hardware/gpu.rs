// SPDX-License-Identifier: Apache-2.0

//! GPU power source reader.
//!
//! Supports:
//! - NVIDIA via NVML (nvml-wrapper crate, compile-time feature)
//! - AMD via ROCm SMI (future)
//! - Intel via Level Zero (future)
//!
//! GPU power is a direct electrical measurement from on-board shunt
//! resistors — tagged as ReadingType::Measured.
//!
//! When the `gpu` feature is not enabled, discover() returns empty.

use std::time::{Duration, Instant};

use super::{
    Component, Granularity, PowerReading, PowerSource, ReadingType, SourceError, SourceId,
};

/// A GPU power source (one per physical GPU device).
pub struct GpuSource {
    id: SourceId,
    display_name: String,
    device_index: u8,
    /// Cached UUID for device identification across reboots
    uuid: String,
}

impl PowerSource for GpuSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn component(&self) -> Component {
        Component::Gpu
    }

    fn granularity(&self) -> Granularity {
        Granularity::Device(self.device_index)
    }

    fn reading_type(&self) -> ReadingType {
        ReadingType::Measured
    }

    fn read(&self) -> Result<PowerReading, SourceError> {
        // TODO: Implement NVML power reading
        // nvml::Device::power_usage() returns milliwatts
        //
        // let device = nvml.device_by_index(self.device_index as u32)?;
        // let power_mw = device.power_usage()?;
        // let power_uw = power_mw as u64 * 1000;
        //
        // Some GPUs also support energy counters:
        // let energy_uj = device.total_energy_consumption()?; // microjoules

        Err(SourceError::Unavailable(
            "GPU support not yet implemented".into(),
        ))
    }

    fn default_poll_interval(&self) -> Duration {
        Duration::from_millis(500)
    }
}

/// Discover available GPU power sources.
///
/// Currently a stub — returns empty unless the `gpu` feature is enabled
/// and NVML is available.
pub fn discover() -> Result<Vec<GpuSource>, SourceError> {
    // TODO: Implement NVML discovery
    //
    // let nvml = Nvml::init()?;
    // let count = nvml.device_count()?;
    // for i in 0..count {
    //     let device = nvml.device_by_index(i)?;
    //     let name = device.name()?;
    //     let uuid = device.uuid()?;
    //     sources.push(GpuSource {
    //         id: SourceId(format!("nvml:gpu:{}", i)),
    //         display_name: format!("GPU {} ({})", i, name),
    //         device_index: i as u8,
    //         uuid,
    //     });
    // }

    // For now, check if any NVIDIA GPUs exist via sysfs
    let nvidia_path = std::path::Path::new("/proc/driver/nvidia");
    if nvidia_path.exists() {
        log::info!("NVIDIA driver detected but NVML support not yet implemented");
    }

    Err(SourceError::Unavailable("GPU discovery not implemented".into()))
}
