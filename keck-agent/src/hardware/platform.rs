// SPDX-License-Identifier: Apache-2.0

//! Platform power sources via Redfish.
//!
//! Creates multiple sources from a single iDRAC:
//! - Platform total (PowerConsumedWatts — PSU level)
//! - CPU power (SystemBoardCPUUsage % × total)
//! - Memory power (SystemBoardMEMUsage % × total)
//!
//! These are MEASURED values from voltage regulators — more accurate than RAPL.

use std::time::{Duration, Instant};

use super::{
    Component, Granularity, PowerReading, PowerSource, ReadingType, SourceError, SourceId,
};

/// Redfish sensor source — reads a specific sensor from the BMC.
pub struct RedfishSource {
    id: SourceId,
    display_name: String,
    component: Component,
    endpoint: String,
    sensor_path: String,
    /// For percentage sensors: multiply by board total power
    is_percentage: bool,
    /// Path to read board total (for percentage conversion)
    board_total_path: String,
    username: String,
    password: String,
    client: reqwest::blocking::Client,
}

impl PowerSource for RedfishSource {
    fn id(&self) -> &SourceId { &self.id }
    fn name(&self) -> &str { &self.display_name }
    fn component(&self) -> Component { self.component }
    fn granularity(&self) -> Granularity { Granularity::Node }
    fn reading_type(&self) -> ReadingType { ReadingType::Measured }

    fn read(&self) -> Result<PowerReading, SourceError> {
        let watts = if self.is_percentage {
            // Read percentage sensor and board total, compute watts
            let pct = self.read_sensor_value(&self.sensor_path)?;
            let total = self.read_sensor_value(&self.board_total_path)?;
            total * pct / 100.0
        } else {
            self.read_sensor_value(&self.sensor_path)?
        };

        let power_uw = (watts * 1_000_000.0) as u64;

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

    fn default_poll_interval(&self) -> Duration {
        Duration::from_secs(3)
    }
}

impl RedfishSource {
    fn read_sensor_value(&self, path: &str) -> Result<f64, SourceError> {
        let url = format!("{}{}", self.endpoint, path);
        let resp = self.client
            .get(&url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .map_err(|e| SourceError::Unavailable(format!("Redfish request failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(SourceError::Unavailable(format!("Redfish returned {}", resp.status())));
        }

        let body: serde_json::Value = resp.json()
            .map_err(|e| SourceError::Parse(format!("JSON parse failed: {}", e)))?;

        // Try Reading field (Sensors API)
        if let Some(v) = body.get("Reading").and_then(|v| v.as_f64()) {
            return Ok(v);
        }

        // Try PowerConsumedWatts (Power API)
        if let Some(v) = body.get("PowerControl")
            .and_then(|pc| pc.as_array())
            .and_then(|arr| arr.first())
            .and_then(|ctrl| ctrl.get("PowerConsumedWatts"))
            .and_then(|v| v.as_f64())
        {
            return Ok(v);
        }

        Err(SourceError::Parse("No reading found in Redfish response".into()))
    }
}

/// Discover Redfish power sources.
///
/// Creates sources for platform total, CPU, and memory from iDRAC sensors.
pub fn discover() -> Result<Vec<Box<dyn PowerSource>>, SourceError> {
    let endpoint = if let Ok(ep) = std::env::var("REDFISH_ENDPOINT") {
        ep
    } else if let Ok(map) = std::env::var("REDFISH_MAP") {
        let sysfs = super::sysfs_root();
        let serial_path = format!("{}/class/dmi/id/product_serial", sysfs);
        let serial = std::fs::read_to_string(&serial_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        if serial.is_empty() {
            return Err(SourceError::Unavailable("Cannot read node serial number".into()));
        }

        log::info!("Node serial: {}, looking up in REDFISH_MAP", serial);

        match map.split(',').find_map(|entry| {
            let parts: Vec<&str> = entry.splitn(2, '=').collect();
            if parts.len() == 2 && parts[0].trim() == serial {
                Some(parts[1].trim().to_string())
            } else {
                None
            }
        }) {
            Some(ep) => ep,
            None => return Err(SourceError::Unavailable(
                format!("Serial {} not found in REDFISH_MAP", serial),
            )),
        }
    } else {
        return Err(SourceError::Unavailable("REDFISH_ENDPOINT or REDFISH_MAP not set".into()));
    };

    let username = std::env::var("REDFISH_USERNAME").unwrap_or_else(|_| "root".into());
    let password = std::env::var("REDFISH_PASSWORD").unwrap_or_else(|_| "calvin".into());

    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| SourceError::Unavailable(format!("HTTP client error: {}", e)))?;

    // Test connection
    let test_url = format!("{}/redfish/v1/Chassis/System.Embedded.1/Power", endpoint);
    let resp = client.get(&test_url).basic_auth(&username, Some(&password)).send()
        .map_err(|e| SourceError::Unavailable(format!("Redfish connection failed: {}", e)))?;
    if !resp.status().is_success() {
        return Err(SourceError::Unavailable(format!("Redfish returned {}", resp.status())));
    }

    log::info!("Redfish connected: {}", endpoint);

    let board_total_path = "/redfish/v1/Chassis/System.Embedded.1/Sensors/SystemBoardPwrConsumption".to_string();

    let mut sources: Vec<Box<dyn PowerSource>> = Vec::new();

    // Platform total (PSU power)
    sources.push(Box::new(RedfishSource {
        id: SourceId(format!("redfish:platform:{}", endpoint)),
        display_name: format!("Redfish PSU Total ({})", endpoint),
        component: Component::Platform,
        sensor_path: "/redfish/v1/Chassis/System.Embedded.1/Power".into(),
        is_percentage: false,
        board_total_path: String::new(),
        endpoint: endpoint.clone(),
        username: username.clone(),
        password: password.clone(),
        client: client.clone(),
    }));

    // CPU power (SystemBoardCPUUsage % × board total) — MEASURED
    let cpu_sensor = format!("{}/redfish/v1/Chassis/System.Embedded.1/Sensors/SystemBoardCPUUsage", endpoint);
    if client.get(&cpu_sensor).basic_auth(&username, Some(&password)).send().map(|r| r.status().is_success()).unwrap_or(false) {
        sources.push(Box::new(RedfishSource {
            id: SourceId(format!("redfish:cpu:{}", endpoint)),
            display_name: format!("Redfish CPU ({})", endpoint),
            component: Component::Cpu,
            sensor_path: "/redfish/v1/Chassis/System.Embedded.1/Sensors/SystemBoardCPUUsage".into(),
            is_percentage: true,
            board_total_path: board_total_path.clone(),
            endpoint: endpoint.clone(),
            username: username.clone(),
            password: password.clone(),
            client: client.clone(),
        }));
        log::info!("  Redfish CPU sensor available (measured)");
    }

    // Memory power (SystemBoardMEMUsage % × board total) — MEASURED
    let mem_sensor = format!("{}/redfish/v1/Chassis/System.Embedded.1/Sensors/SystemBoardMEMUsage", endpoint);
    if client.get(&mem_sensor).basic_auth(&username, Some(&password)).send().map(|r| r.status().is_success()).unwrap_or(false) {
        sources.push(Box::new(RedfishSource {
            id: SourceId(format!("redfish:memory:{}", endpoint)),
            display_name: format!("Redfish Memory ({})", endpoint),
            component: Component::Memory,
            sensor_path: "/redfish/v1/Chassis/System.Embedded.1/Sensors/SystemBoardMEMUsage".into(),
            is_percentage: true,
            board_total_path: board_total_path.clone(),
            endpoint: endpoint.clone(),
            username: username.clone(),
            password: password.clone(),
            client: client.clone(),
        }));
        log::info!("  Redfish Memory sensor available (measured)");
    }

    Ok(sources)
}
