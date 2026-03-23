// SPDX-License-Identifier: Apache-2.0

//! GPU power source via DCGM exporter.
//!
//! Reads GPU power from the NVIDIA DCGM exporter Prometheus endpoint.
//! DCGM runs as part of the NVIDIA GPU Operator on OpenShift and exposes
//! per-GPU, per-pod power draw measured from the GPU's on-board sensor.
//!
//! This is a MEASURED value — actual watts from the GPU hardware.
//!
//! The agent discovers DCGM exporters running on the same node by querying
//! the K8s API or using a well-known pod IP pattern.

use std::time::{Duration, Instant};

use super::{
    Component, Granularity, PowerReading, PowerSource, ReadingType, SourceError, SourceId,
};

/// GPU power source backed by DCGM exporter.
pub struct DcgmGpuSource {
    id: SourceId,
    display_name: String,
    device_index: u8,
    dcgm_url: String,
    gpu_uuid: String,
    client: reqwest::blocking::Client,
}

impl PowerSource for DcgmGpuSource {
    fn id(&self) -> &SourceId { &self.id }
    fn name(&self) -> &str { &self.display_name }
    fn component(&self) -> Component { Component::Gpu }
    fn granularity(&self) -> Granularity { Granularity::Device(self.device_index) }
    fn reading_type(&self) -> ReadingType { ReadingType::Measured }

    fn read(&self) -> Result<PowerReading, SourceError> {
        let resp = self.client
            .get(&self.dcgm_url)
            .send()
            .map_err(|e| SourceError::Unavailable(format!("DCGM request failed: {}", e)))?;

        let body = resp.text()
            .map_err(|e| SourceError::Parse(format!("DCGM response read failed: {}", e)))?;

        // Parse Prometheus text format for DCGM_FI_DEV_POWER_USAGE
        // Filter by hostname to only get this node's GPU(s)
        let hostname = std::env::var("NODE_NAME").unwrap_or_default();
        let mut total_watts = 0.0f64;
        for line in body.lines() {
            if !line.starts_with("DCGM_FI_DEV_POWER_USAGE{") {
                continue;
            }
            // Filter by this node's hostname
            if !hostname.is_empty() && !line.contains(&format!("Hostname=\"{}\"", hostname)) {
                continue;
            }
            // Also filter by GPU UUID if set
            if !self.gpu_uuid.is_empty() && !line.contains(&self.gpu_uuid) {
                continue;
            }
            // Extract the value (last space-separated field)
            if let Some(val_str) = line.rsplit_once(' ').map(|(_, v)| v) {
                if let Ok(watts) = val_str.parse::<f64>() {
                    total_watts += watts;
                }
            }
        }

        if total_watts == 0.0 {
            return Err(SourceError::Unavailable("No GPU power reading from DCGM".into()));
        }

        let power_uw = (total_watts * 1_000_000.0) as u64;

        Ok(PowerReading {
            source_id: self.id.clone(),
            timestamp: Instant::now(),
            component: Component::Gpu,
            granularity: Granularity::Device(self.device_index),
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

/// Discover GPU power sources via DCGM exporter.
///
/// Finds DCGM exporter pods on this node and creates sources for each GPU.
/// DCGM exporter URL from env var or well-known default.
pub fn discover() -> Result<Vec<DcgmGpuSource>, SourceError> {
    // Check if NVIDIA driver is present
    let procfs = super::procfs_root();
    let nvidia_path = format!("{}/driver/nvidia", procfs);
    if !std::path::Path::new(&nvidia_path).exists() {
        return Err(SourceError::Unavailable("No NVIDIA driver found".into()));
    }

    // DCGM exporter URL — auto-discover from K8s API (node-local pod IP)
    // or use env var as override
    let dcgm_url = if let Ok(url) = std::env::var("DCGM_EXPORTER_URL") {
        // If it's a service URL, try to resolve to the local pod instead
        if url.contains(".svc") {
            log::info!("DCGM_EXPORTER_URL is a service — trying to discover node-local pod");
            discover_dcgm_url().unwrap_or(url)
        } else {
            url
        }
    } else {
        match discover_dcgm_url() {
            Some(url) => url,
            None => {
                return Err(SourceError::Unavailable(
                    "DCGM exporter not found on this node.".into(),
                ));
            }
        }
    };
    log::info!("DCGM URL: {}", dcgm_url);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| SourceError::Unavailable(format!("HTTP client error: {}", e)))?;

    // Test DCGM connection
    let resp = client.get(&dcgm_url).send()
        .map_err(|e| SourceError::Unavailable(format!("DCGM connection failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(SourceError::Unavailable(format!("DCGM returned {}", resp.status())));
    }

    let body = resp.text()
        .map_err(|e| SourceError::Parse(format!("DCGM response failed: {}", e)))?;

    // Parse GPU info from DCGM metrics, filtered to this node
    let hostname = std::env::var("NODE_NAME").unwrap_or_default();
    log::info!("DCGM: scanning for GPUs on node '{}'", hostname);

    let mut sources = Vec::new();
    let mut seen_uuids = std::collections::HashSet::new();

    for line in body.lines() {
        if !line.starts_with("DCGM_FI_DEV_POWER_USAGE{") {
            continue;
        }

        // Only match GPUs on this node
        if !hostname.is_empty() {
            if let Some(h) = extract_label(line, "Hostname") {
                if h != hostname {
                    continue;
                }
            }
        }

        // Extract UUID
        let uuid = extract_label(line, "UUID").unwrap_or_default();
        if uuid.is_empty() || seen_uuids.contains(&uuid) {
            continue;
        }
        seen_uuids.insert(uuid.clone());

        let model = extract_label(line, "modelName").unwrap_or_else(|| "NVIDIA GPU".into());
        let gpu_index = extract_label(line, "gpu")
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(sources.len() as u8);

        log::info!("  DCGM GPU {}: {} ({})", gpu_index, model, uuid);

        sources.push(DcgmGpuSource {
            id: SourceId(format!("dcgm:gpu:{}", gpu_index)),
            display_name: format!("DCGM {} (GPU {})", model, gpu_index),
            device_index: gpu_index,
            dcgm_url: dcgm_url.clone(),
            gpu_uuid: uuid,
            client: client.clone(),
        });
    }

    if sources.is_empty() {
        return Err(SourceError::Unavailable(
            "NVIDIA driver present but DCGM exporter not reachable. Set DCGM_EXPORTER_URL.".into(),
        ));
    }

    log::info!("DCGM connected: {} GPU(s) discovered", sources.len());
    Ok(sources)
}

/// Auto-discover DCGM exporter pod IP on this node via K8s API (public).
pub fn discover_dcgm_url_pub() -> Option<String> {
    discover_dcgm_url()
}

/// Auto-discover DCGM exporter pod IP on this node via K8s API.
fn discover_dcgm_url() -> Option<String> {
    let token = std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/token").ok()?;
    let node_name = std::env::var("NODE_NAME").ok()?;

    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;

    // Query pods with dcgm-exporter label on this node
    let url = format!(
        "https://kubernetes.default.svc/api/v1/namespaces/nvidia-gpu-operator/pods?labelSelector=app=nvidia-dcgm-exporter&fieldSelector=spec.nodeName={}",
        node_name,
    );

    let resp = client.get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .ok()?;

    if !resp.status().is_success() {
        log::debug!("K8s API returned {} for DCGM pod lookup", resp.status());
        return None;
    }

    let body: serde_json::Value = resp.json().ok()?;
    let pod_ip = body.get("items")?
        .as_array()?
        .first()?
        .get("status")?
        .get("podIP")?
        .as_str()?;

    let url = format!("http://{}:9400/metrics", pod_ip);
    log::info!("Auto-discovered DCGM exporter at {}", url);
    Some(url)
}

/// Extract a label value from a Prometheus metric line.
fn extract_label(line: &str, label: &str) -> Option<String> {
    let pattern = format!("{}=\"", label);
    let start = line.find(&pattern)? + pattern.len();
    let end = start + line[start..].find('"')?;
    Some(line[start..end].to_string())
}
