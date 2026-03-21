// SPDX-License-Identifier: Apache-2.0

//! Platform power source: PSU input power via Redfish or IPMI.
//!
//! This is the GROUND TRUTH anchor. The PSU input power is the only
//! number that represents actual electricity consumed by the server.
//! Everything else (RAPL, hwmon, GPU) is a component measurement that
//! should sum to less than or equal to the PSU reading.
//!
//! Sources:
//! - Redfish PowerSubsystem API (modern BMCs)
//! - Redfish Power API (older BMCs, deprecated but common)
//! - IPMI DCMI power reading (legacy fallback)
//!
//! Platform sources are slow (HTTP/IPMI roundtrip: 100ms-2s) and should
//! be polled on the slow tier (2-5s intervals). They are read at every
//! heartbeat for reconciliation.

use std::time::{Duration, Instant};

use super::{
    Component, Granularity, PowerReading, PowerSource, ReadingType, SourceError, SourceId,
};

/// Redfish PSU power source.
pub struct RedfishSource {
    id: SourceId,
    display_name: String,
    endpoint: String,
    // TODO: auth credentials, HTTP client
}

impl PowerSource for RedfishSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn component(&self) -> Component {
        Component::Platform
    }

    fn granularity(&self) -> Granularity {
        Granularity::Node
    }

    fn reading_type(&self) -> ReadingType {
        ReadingType::Measured // PSU shunt = real measurement
    }

    fn read(&self) -> Result<PowerReading, SourceError> {
        // TODO: Implement Redfish API call
        //
        // Strategy: try PowerSubsystem (modern) first, fall back to Power (legacy)
        //
        // Modern: GET /redfish/v1/Chassis/1/PowerSubsystem/PowerSupplies
        //   → PowerSupplies[].Metrics.InputPowerWatts
        //
        // Legacy: GET /redfish/v1/Chassis/1/Power
        //   → PowerControl[].PowerConsumedWatts
        //
        // Convert watts to microwatts for our internal representation.

        Err(SourceError::Unavailable(
            "Redfish support not yet implemented".into(),
        ))
    }

    fn default_poll_interval(&self) -> Duration {
        // Redfish is an HTTP call to the BMC. 2-5 second intervals are typical.
        Duration::from_secs(3)
    }
}

/// IPMI DCMI power reading (legacy fallback).
pub struct IpmiSource {
    id: SourceId,
    display_name: String,
}

impl PowerSource for IpmiSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.display_name
    }

    fn component(&self) -> Component {
        Component::Platform
    }

    fn granularity(&self) -> Granularity {
        Granularity::Node
    }

    fn reading_type(&self) -> ReadingType {
        ReadingType::Measured
    }

    fn read(&self) -> Result<PowerReading, SourceError> {
        // TODO: Implement IPMI DCMI power reading
        //
        // Option A: Parse output of `ipmitool dcmi power reading`
        //   Instantaneous power reading:    210 Watts
        //
        // Option B: Use ipmi-rs crate for direct /dev/ipmi0 access
        //   (avoids shell-out, lower latency)
        //
        // DCMI (Data Center Manageability Interface) provides platform
        // power readings from the BMC's power monitoring circuitry.

        Err(SourceError::Unavailable(
            "IPMI support not yet implemented".into(),
        ))
    }

    fn default_poll_interval(&self) -> Duration {
        Duration::from_secs(5)
    }
}

/// Discover platform power sources.
///
/// Tries Redfish first (configured via environment or config file),
/// then falls back to IPMI DCMI if /dev/ipmi0 exists.
pub fn discover() -> Result<Vec<Box<dyn PowerSource>>, SourceError> {
    let mut sources: Vec<Box<dyn PowerSource>> = Vec::new();

    // Try Redfish (from environment or config)
    if let Ok(endpoint) = std::env::var("REDFISH_ENDPOINT") {
        log::info!("Redfish endpoint configured: {}", endpoint);
        sources.push(Box::new(RedfishSource {
            id: SourceId("redfish:psu:0".into()),
            display_name: format!("Redfish PSU ({})", endpoint),
            endpoint,
        }));
    }

    // Try IPMI as fallback
    if std::path::Path::new("/dev/ipmi0").exists() {
        log::info!("IPMI device found: /dev/ipmi0");
        sources.push(Box::new(IpmiSource {
            id: SourceId("ipmi:dcmi:0".into()),
            display_name: "IPMI DCMI power reading".into(),
        }));
    }

    if sources.is_empty() {
        return Err(SourceError::Unavailable(
            "No platform power source found (no Redfish, no IPMI)".into(),
        ));
    }

    Ok(sources)
}
