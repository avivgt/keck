// SPDX-License-Identifier: Apache-2.0

//! Platform power source: PSU input power via Redfish.
//!
//! This is the GROUND TRUTH — actual watts measured at the PSU.
//! Used to validate RAPL estimates and compute the error ratio.
//!
//! Configuration via environment variables:
//!   REDFISH_ENDPOINT — iDRAC/BMC URL (e.g., https://192.168.52.172)
//!   REDFISH_USERNAME — default: root
//!   REDFISH_PASSWORD — default: calvin

use std::time::{Duration, Instant};

use super::{
    Component, Granularity, PowerReading, PowerSource, ReadingType, SourceError, SourceId,
};

/// Redfish PSU power source — reads real measured watts from the BMC.
pub struct RedfishSource {
    id: SourceId,
    display_name: String,
    endpoint: String,
    username: String,
    password: String,
    client: reqwest::blocking::Client,
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
        ReadingType::Measured
    }

    fn read(&self) -> Result<PowerReading, SourceError> {
        let url = format!(
            "{}/redfish/v1/Chassis/System.Embedded.1/Power",
            self.endpoint
        );

        let resp = self
            .client
            .get(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .map_err(|e| SourceError::Unavailable(format!("Redfish request failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(SourceError::Unavailable(format!(
                "Redfish returned {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .map_err(|e| SourceError::Parse(format!("Redfish JSON parse failed: {}", e)))?;

        // Extract PowerConsumedWatts from PowerControl array
        let watts = body
            .get("PowerControl")
            .and_then(|pc| pc.as_array())
            .and_then(|arr| arr.first())
            .and_then(|ctrl| ctrl.get("PowerConsumedWatts"))
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                SourceError::Parse("PowerConsumedWatts not found in Redfish response".into())
            })?;

        // Convert watts to microwatts
        let power_uw = (watts * 1_000_000.0) as u64;

        Ok(PowerReading {
            source_id: self.id.clone(),
            timestamp: Instant::now(),
            component: Component::Platform,
            granularity: Granularity::Node,
            reading_type: ReadingType::Measured,
            energy_uj: None,
            power_uw: Some(power_uw),
            max_energy_uj: 0,
        })
    }

    fn default_poll_interval(&self) -> Duration {
        Duration::from_secs(3)
    }
}

/// Discover Redfish platform power source.
///
/// Configured via environment variables:
///   REDFISH_ENDPOINT — required (e.g., https://192.168.52.172)
///   REDFISH_USERNAME — optional (default: root)
///   REDFISH_PASSWORD — optional (default: calvin)
pub fn discover() -> Result<Vec<Box<dyn PowerSource>>, SourceError> {
    // Option 1: Direct endpoint (for single-server or when set per-node)
    // Option 2: Serial-to-endpoint mapping for DaemonSet deployments
    //   REDFISH_MAP="SERIAL1=https://ip1,SERIAL2=https://ip2"
    let endpoint = if let Ok(ep) = std::env::var("REDFISH_ENDPOINT") {
        ep
    } else if let Ok(map) = std::env::var("REDFISH_MAP") {
        // Read this node's serial number
        let sysfs = super::sysfs_root();
        let serial_path = format!("{}/class/dmi/id/product_serial", sysfs);
        let serial = std::fs::read_to_string(&serial_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        if serial.is_empty() {
            return Err(SourceError::Unavailable("Cannot read node serial number".into()));
        }

        log::info!("Node serial: {}, looking up in REDFISH_MAP", serial);

        // Parse "SERIAL1=https://ip1,SERIAL2=https://ip2"
        match map.split(',').find_map(|entry| {
            let parts: Vec<&str> = entry.splitn(2, '=').collect();
            if parts.len() == 2 && parts[0].trim() == serial {
                Some(parts[1].trim().to_string())
            } else {
                None
            }
        }) {
            Some(ep) => ep,
            None => {
                return Err(SourceError::Unavailable(
                    format!("Serial {} not found in REDFISH_MAP", serial),
                ));
            }
        }
    } else {
        return Err(SourceError::Unavailable(
            "REDFISH_ENDPOINT or REDFISH_MAP not set".into(),
        ));
    };

    let username = std::env::var("REDFISH_USERNAME").unwrap_or_else(|_| "root".into());
    let password = std::env::var("REDFISH_PASSWORD").unwrap_or_else(|_| "calvin".into());

    // Build HTTP client that accepts self-signed certs (iDRAC uses self-signed)
    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| SourceError::Unavailable(format!("Failed to create HTTP client: {}", e)))?;

    // Test the connection
    let test_url = format!("{}/redfish/v1/Chassis/System.Embedded.1/Power", endpoint);
    let resp = client
        .get(&test_url)
        .basic_auth(&username, Some(&password))
        .send()
        .map_err(|e| SourceError::Unavailable(format!("Redfish connection failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(SourceError::Unavailable(format!(
            "Redfish returned {} — check credentials",
            resp.status()
        )));
    }

    log::info!("Redfish connected: {} (authenticated as {})", endpoint, username);

    let source = RedfishSource {
        id: SourceId(format!("redfish:{}", endpoint)),
        display_name: format!("Redfish PSU ({})", endpoint),
        endpoint,
        username,
        password,
        client,
    };

    Ok(vec![Box::new(source)])
}
