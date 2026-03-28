// SPDX-License-Identifier: Apache-2.0

//! Platform power sources via Redfish — vendor-agnostic discovery.
//!
//! Probes the BMC's Redfish API to discover available power metrics:
//!
//! Level 1 (best): TelemetryService MetricDefinitions
//!   → TotalCPUPower, TotalMemoryPower, TotalFanPower, TotalStoragePower, TotalPciePower
//!   → Available on Dell iDRAC9+, potentially HP iLO6+
//!
//! Level 2: Sensors API with percentage-of-board readings
//!   → SystemBoardCPUUsage, SystemBoardMEMUsage, SystemBoardIOUsage
//!   → Available on Dell iDRAC9 (some models)
//!
//! Level 3 (always): Power API
//!   → PowerConsumedWatts (PSU total — all vendors)
//!
//! Whatever is not covered by Redfish falls back to RAPL/hwmon.

use std::time::{Duration, Instant};

use super::{
    Component, Granularity, PowerReading, PowerSource, ReadingType, SourceError, SourceId,
};

/// Redfish power source — reads a specific metric from the BMC.
pub struct RedfishSource {
    id: SourceId,
    display_name: String,
    component: Component,
    endpoint: String,
    /// How to read this metric
    read_method: ReadMethod,
    username: String,
    password: String,
    client: reqwest::blocking::Client,
}

enum ReadMethod {
    /// Read "Reading" field from a Sensors API endpoint
    SensorReading { path: String },
    /// Read a percentage sensor × board total power
    SensorPercentage { pct_path: String, total_path: String },
    /// Read PowerConsumedWatts from Power API
    PowerApi { path: String },
    /// Read from TelemetryService MetricReports (value in watts directly)
    TelemetryMetric { metric_id: String },
}

impl PowerSource for RedfishSource {
    fn id(&self) -> &SourceId { &self.id }
    fn name(&self) -> &str { &self.display_name }
    fn component(&self) -> Component { self.component }
    fn granularity(&self) -> Granularity { Granularity::Node }
    fn reading_type(&self) -> ReadingType { ReadingType::Measured }

    fn read(&self) -> Result<PowerReading, SourceError> {
        let watts = match &self.read_method {
            ReadMethod::SensorReading { path } => {
                self.read_json_field(&format!("{}{}", self.endpoint, path), "Reading")?
            }
            ReadMethod::SensorPercentage { pct_path, total_path } => {
                let pct = self.read_json_field(&format!("{}{}", self.endpoint, pct_path), "Reading")?;
                let total = self.read_json_field(&format!("{}{}", self.endpoint, total_path), "Reading")?;
                total * pct / 100.0
            }
            ReadMethod::PowerApi { path } => {
                let url = format!("{}{}", self.endpoint, path);
                let resp = self.client.get(&url)
                    .basic_auth(&self.username, Some(&self.password))
                    .send()
                    .map_err(|e| SourceError::Unavailable(format!("Redfish: {}", e)))?;
                let body: serde_json::Value = resp.json()
                    .map_err(|e| SourceError::Parse(format!("JSON: {}", e)))?;
                body.get("PowerControl")
                    .and_then(|pc| pc.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|ctrl| ctrl.get("PowerConsumedWatts"))
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| SourceError::Parse("PowerConsumedWatts not found".into()))?
            }
            ReadMethod::TelemetryMetric { metric_id } => {
                // Read from MetricReports — the value is the metric reading
                let url = format!(
                    "{}/redfish/v1/TelemetryService/MetricDefinitions/{}",
                    self.endpoint, metric_id
                );
                self.read_json_field(&url, "Reading")
                    .or_else(|_| {
                        // Some Dell firmware puts the value in a different location
                        // Try the sensor with same name
                        let sensor_url = format!(
                            "{}/redfish/v1/Chassis/System.Embedded.1/Sensors/{}",
                            self.endpoint, metric_id
                        );
                        self.read_json_field(&sensor_url, "Reading")
                    })?
            }
        };

        Ok(PowerReading {
            source_id: self.id.clone(),
            timestamp: Instant::now(),
            component: self.component,
            granularity: Granularity::Node,
            reading_type: ReadingType::Measured,
            energy_uj: None,
            power_uw: Some((watts * 1_000_000.0) as u64),
            max_energy_uj: 0,
        })
    }

    fn default_poll_interval(&self) -> Duration {
        Duration::from_secs(3)
    }
}

impl RedfishSource {
    fn read_json_field(&self, url: &str, field: &str) -> Result<f64, SourceError> {
        let resp = self.client.get(url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .map_err(|e| SourceError::Unavailable(format!("Redfish: {}", e)))?;
        if !resp.status().is_success() {
            return Err(SourceError::Unavailable(format!("HTTP {}", resp.status())));
        }
        let body: serde_json::Value = resp.json()
            .map_err(|e| SourceError::Parse(format!("JSON: {}", e)))?;
        body.get(field)
            .and_then(|v| v.as_f64())
            .ok_or_else(|| SourceError::Parse(format!("Field '{}' not found", field)))
    }
}

/// Probe a Redfish URL — returns true if it exists and returns 200.
fn probe(client: &reqwest::blocking::Client, url: &str, user: &str, pass: &str) -> bool {
    client.get(url)
        .basic_auth(user, Some(pass))
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Probe a Redfish URL and check if a specific field exists with a numeric value.
fn probe_reading(client: &reqwest::blocking::Client, url: &str, user: &str, pass: &str) -> Option<f64> {
    let resp = client.get(url).basic_auth(user, Some(pass)).send().ok()?;
    if !resp.status().is_success() { return None; }
    let body: serde_json::Value = resp.json().ok()?;
    body.get("Reading").and_then(|v| v.as_f64())
}

/// Discover Redfish power sources using vendor-agnostic capability probing.
///
/// Discovery order (best first):
/// 1. TelemetryService MetricDefinitions (Dell iDRAC, potentially others)
/// 2. Sensors API percentage readings (some BMCs)
/// 3. Power API total (all BMCs)
pub fn discover() -> Result<Vec<Box<dyn PowerSource>>, SourceError> {
    // Resolve endpoint
    let endpoint = if let Ok(ep) = std::env::var("REDFISH_ENDPOINT") {
        ep
    } else if let Ok(map) = std::env::var("REDFISH_MAP") {
        let sysfs = super::sysfs_root();
        let serial_path = format!("{}/class/dmi/id/product_serial", sysfs);
        let serial = std::fs::read_to_string(&serial_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if serial.is_empty() {
            return Err(SourceError::Unavailable("Cannot read serial".into()));
        }
        log::info!("Node serial: {}", serial);
        match map.split(',').find_map(|entry| {
            let parts: Vec<&str> = entry.splitn(2, '=').collect();
            if parts.len() == 2 && parts[0].trim() == serial {
                Some(parts[1].trim().to_string())
            } else { None }
        }) {
            Some(ep) => ep,
            None => return Err(SourceError::Unavailable(format!("Serial {} not in REDFISH_MAP", serial))),
        }
    } else {
        return Err(SourceError::Unavailable("REDFISH_ENDPOINT or REDFISH_MAP not set".into()));
    };

    let username = std::env::var("REDFISH_USERNAME")
        .map_err(|_| SourceError::Unavailable("REDFISH_USERNAME not set".into()))?;
    let password = std::env::var("REDFISH_PASSWORD")
        .map_err(|_| SourceError::Unavailable("REDFISH_PASSWORD not set".into()))?;

    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| SourceError::Unavailable(format!("HTTP client: {}", e)))?;

    // Test basic connectivity
    let power_url = format!("{}/redfish/v1/Chassis/System.Embedded.1/Power", endpoint);
    if !probe(&client, &power_url, &username, &password) {
        return Err(SourceError::Unavailable("Redfish not reachable".into()));
    }

    log::info!("Redfish connected: {}", endpoint);

    let mut sources: Vec<Box<dyn PowerSource>> = Vec::new();

    // Component discovery — for each component, try best method first

    // ─── CPU Power ───────────────────────────────────────────────
    let cpu_source = try_telemetry_metric(&client, &endpoint, &username, &password, "TotalCPUPower", Component::Cpu, "Redfish Telemetry CPU")
        .or_else(|| try_sensor_pct(&client, &endpoint, &username, &password, "SystemBoardCPUUsage", Component::Cpu, "Redfish Sensor CPU"))
        .or_else(|| try_telemetry_metric(&client, &endpoint, &username, &password, "CPUPower", Component::Cpu, "Redfish Telemetry CPUPower"));

    if let Some(src) = cpu_source {
        log::info!("  CPU: {} (measured)", src.name());
        sources.push(Box::new(src));
    } else {
        log::info!("  CPU: no Redfish source (will use RAPL)");
    }

    // ─── Memory Power ────────────────────────────────────────────
    let mem_source = try_telemetry_metric(&client, &endpoint, &username, &password, "TotalMemoryPower", Component::Memory, "Redfish Telemetry Memory")
        .or_else(|| try_telemetry_metric(&client, &endpoint, &username, &password, "DRAMPwr", Component::Memory, "Redfish Telemetry DRAM"))
        .or_else(|| try_sensor_pct(&client, &endpoint, &username, &password, "SystemBoardMEMUsage", Component::Memory, "Redfish Sensor Memory"));

    if let Some(src) = mem_source {
        log::info!("  Memory: {} (measured)", src.name());
        sources.push(Box::new(src));
    } else {
        log::info!("  Memory: no Redfish source (will use RAPL DRAM)");
    }

    // ─── Fan Power ───────────────────────────────────────────────
    if let Some(src) = try_telemetry_metric(&client, &endpoint, &username, &password, "TotalFanPower", Component::Fan, "Redfish Telemetry Fans") {
        log::info!("  Fans: {} (measured)", src.name());
        sources.push(Box::new(src));
    }

    // ─── Storage Power ───────────────────────────────────────────
    if let Some(src) = try_telemetry_metric(&client, &endpoint, &username, &password, "TotalStoragePower", Component::Storage, "Redfish Telemetry Storage") {
        log::info!("  Storage: {} (measured)", src.name());
        sources.push(Box::new(src));
    }

    // ─── I/O / PCIe Power ──────────────────────────────────────
    if let Some(src) = try_sensor_pct(&client, &endpoint, &username, &password, "SystemBoardIOUsage", Component::Nic, "Redfish Sensor I/O") {
        log::info!("  I/O: {} (measured)", src.name());
        sources.push(Box::new(src));
    } else if let Some(src) = try_telemetry_metric(&client, &endpoint, &username, &password, "TotalPciePower", Component::Nic, "Redfish Telemetry PCIe") {
        log::info!("  PCIe: {} (measured)", src.name());
        sources.push(Box::new(src));
    } else {
        log::info!("  I/O: no Redfish source");
    }

    // ─── Platform/Chipset Power (from sensor %) ──────────────────
    // SYS usage = chipset + VRM + misc board power (infrastructure, not workload)
    if let Some(src) = try_sensor_pct(&client, &endpoint, &username, &password, "SystemBoardSYSUsage", Component::Platform, "Redfish Sensor Platform") {
        log::info!("  Platform subsystem: {} (measured)", src.name());
        sources.push(Box::new(src));
    }

    // ─── Per-PSU Input/Output/Efficiency ──────────────────────────
    let power_url = format!("{}/redfish/v1/Chassis/System.Embedded.1/Power", endpoint);
    if let Ok(psu_sources) = discover_psu_detail(&client, &power_url, &username, &password, &endpoint) {
        for src in &psu_sources {
            log::info!("  PSU: {} (measured)", src.name());
        }
        sources.extend(psu_sources.into_iter().map(|s| Box::new(s) as Box<dyn PowerSource>));
    }

    // ─── PSU Total (always) ───────────────────────────────────────
    sources.push(Box::new(RedfishSource {
        id: SourceId(format!("redfish:platform:{}", endpoint)),
        display_name: format!("Redfish PSU Total ({})", endpoint),
        component: Component::Platform,
        read_method: ReadMethod::PowerApi {
            path: "/redfish/v1/Chassis/System.Embedded.1/Power".into(),
        },
        endpoint: endpoint.clone(),
        username: username.clone(),
        password: password.clone(),
        client: client.clone(),
    }));
    log::info!("  PSU Total: measured");

    Ok(sources)
}

/// Try to create a source from TelemetryService MetricDefinitions.
fn try_telemetry_metric(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    username: &str,
    password: &str,
    metric_id: &str,
    component: Component,
    display_prefix: &str,
) -> Option<RedfishSource> {
    let url = format!("{}/redfish/v1/TelemetryService/MetricDefinitions/{}", endpoint, metric_id);
    if !probe(client, &url, username, password) {
        return None;
    }

    // Try to read from corresponding sensor (Dell often mirrors to Sensors)
    let sensor_url = format!("{}/redfish/v1/Chassis/System.Embedded.1/Sensors/{}", endpoint, metric_id);
    if let Some(_reading) = probe_reading(client, &sensor_url, username, password) {
        // Sensor exists and returns a numeric reading — use it
        return Some(RedfishSource {
            id: SourceId(format!("redfish:{}:{}", metric_id, endpoint)),
            display_name: format!("{} ({})", display_prefix, endpoint),
            component,
            read_method: ReadMethod::SensorReading {
                path: format!("/redfish/v1/Chassis/System.Embedded.1/Sensors/{}", metric_id),
            },
            endpoint: endpoint.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            client: client.clone(),
        });
    }

    // MetricDefinition exists but no readable sensor — skip
    // (definition without data is not useful)
    log::debug!("  {} MetricDefinition exists but no sensor reading available", metric_id);
    None
}

/// Discover per-PSU input/output power from the Power API.
fn discover_psu_detail(
    client: &reqwest::blocking::Client,
    power_url: &str,
    username: &str,
    password: &str,
    endpoint: &str,
) -> Result<Vec<RedfishSource>, SourceError> {
    let resp = client.get(power_url)
        .basic_auth(username, Some(password))
        .send()
        .map_err(|e| SourceError::Unavailable(format!("Redfish: {}", e)))?;
    let body: serde_json::Value = resp.json()
        .map_err(|e| SourceError::Parse(format!("JSON: {}", e)))?;

    let mut sources = Vec::new();

    if let Some(psus) = body.get("PowerSupplies").and_then(|v| v.as_array()) {
        for (i, psu) in psus.iter().enumerate() {
            let name = psu.get("Name").and_then(|v| v.as_str()).unwrap_or("PSU");
            let has_input = psu.get("PowerInputWatts").and_then(|v| v.as_f64()).is_some();
            let has_output = psu.get("PowerOutputWatts").and_then(|v| v.as_f64()).is_some();

            if has_input {
                // Use per-PSU sensor endpoints if available
                let input_sensor = format!("PSU.Slot.{}_InputPower", i + 1);
                let sensor_url = format!("{}/redfish/v1/Chassis/System.Embedded.1/Sensors/{}", endpoint, input_sensor);
                if probe_reading(client, &sensor_url, username, password).is_some() {
                    sources.push(RedfishSource {
                        id: SourceId(format!("redfish:psu{}:input:{}", i + 1, endpoint)),
                        display_name: format!("{} Input ({})", name, endpoint),
                        component: Component::Platform,
                        read_method: ReadMethod::SensorReading {
                            path: format!("/redfish/v1/Chassis/System.Embedded.1/Sensors/{}", input_sensor),
                        },
                        endpoint: endpoint.to_string(),
                        username: username.to_string(),
                        password: password.to_string(),
                        client: client.clone(),
                    });
                }
            }

            if has_output {
                let output_sensor = format!("PSU.Slot.{}_OutputPower", i + 1);
                let sensor_url = format!("{}/redfish/v1/Chassis/System.Embedded.1/Sensors/{}", endpoint, output_sensor);
                if probe_reading(client, &sensor_url, username, password).is_some() {
                    sources.push(RedfishSource {
                        id: SourceId(format!("redfish:psu{}:output:{}", i + 1, endpoint)),
                        display_name: format!("{} Output ({})", name, endpoint),
                        component: Component::Platform,
                        read_method: ReadMethod::SensorReading {
                            path: format!("/redfish/v1/Chassis/System.Embedded.1/Sensors/{}", output_sensor),
                        },
                        endpoint: endpoint.to_string(),
                        username: username.to_string(),
                        password: password.to_string(),
                        client: client.clone(),
                    });
                }
            }
        }
    }

    Ok(sources)
}

/// Try to create a source from Sensors API percentage reading.
fn try_sensor_pct(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    username: &str,
    password: &str,
    sensor_name: &str,
    component: Component,
    display_prefix: &str,
) -> Option<RedfishSource> {
    let sensor_url = format!("{}/redfish/v1/Chassis/System.Embedded.1/Sensors/{}", endpoint, sensor_name);
    probe_reading(client, &sensor_url, username, password)?;

    let total_url = format!("{}/redfish/v1/Chassis/System.Embedded.1/Sensors/SystemBoardPwrConsumption", endpoint);
    probe_reading(client, &total_url, username, password)?;

    Some(RedfishSource {
        id: SourceId(format!("redfish:{}:{}", sensor_name, endpoint)),
        display_name: format!("{} ({})", display_prefix, endpoint),
        component,
        read_method: ReadMethod::SensorPercentage {
            pct_path: format!("/redfish/v1/Chassis/System.Embedded.1/Sensors/{}", sensor_name),
            total_path: "/redfish/v1/Chassis/System.Embedded.1/Sensors/SystemBoardPwrConsumption".into(),
        },
        endpoint: endpoint.to_string(),
        username: username.to_string(),
        password: password.to_string(),
        client: client.clone(),
    })
}
