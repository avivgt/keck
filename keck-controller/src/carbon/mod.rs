// SPDX-License-Identifier: Apache-2.0

//! Carbon intensity tracker.
//!
//! Converts power consumption into carbon emissions by combining
//! energy data with grid carbon intensity (gCO2eq/kWh).
//!
//! Data sources:
//! - Electricity Maps API (https://api.electricitymap.org)
//! - WattTime API (https://api.watttime.org)
//! - Static configuration (for air-gapped environments)
//!
//! The carbon intensity varies by:
//! - Region (California vs. Poland)
//! - Time of day (solar noon vs. midnight)
//! - Grid mix (renewable % at this moment)
//!
//! This data enables:
//! - Carbon-aware scheduling (place workloads where/when grid is cleanest)
//! - ESG reporting (total gCO2eq per namespace/team)
//! - Cost optimization (energy is cheaper when renewables are abundant)

use std::time::{Duration, SystemTime};

/// Carbon intensity value at a point in time.
#[derive(Clone, Debug, serde::Serialize)]
pub struct CarbonIntensity {
    /// Grams of CO2 equivalent per kWh
    pub grams_co2_per_kwh: f64,

    /// Data source name
    pub source: String,

    /// Region/zone this intensity applies to
    pub region: String,

    /// When this intensity was measured/forecast
    pub timestamp: SystemTime,

    /// Whether this is a forecast or actual measurement
    pub is_forecast: bool,
}

/// Cost configuration for energy pricing.
#[derive(Clone, Debug, serde::Serialize)]
pub struct EnergyCost {
    /// Currency code (USD, EUR, etc.)
    pub currency: String,

    /// Cost per kWh in the given currency
    pub cost_per_kwh: f64,

    /// Region/zone this pricing applies to
    pub region: String,
}

/// Carbon and cost calculations for a power reading.
#[derive(Clone, Debug, serde::Serialize)]
pub struct CarbonReport {
    /// Power in watts
    pub power_watts: f64,

    /// Energy consumed in this interval (watt-hours)
    pub energy_wh: f64,

    /// Carbon emissions in grams CO2eq for this interval
    pub carbon_grams: f64,

    /// Annualized carbon emissions in kg CO2eq
    pub carbon_kg_per_year: f64,

    /// Cost for this interval
    pub cost: f64,

    /// Annualized cost
    pub cost_per_year: f64,

    /// Carbon intensity used for calculation
    pub intensity: CarbonIntensity,

    /// Energy cost used for calculation
    pub energy_cost: EnergyCost,
}

/// Tracks carbon intensity and computes emissions.
pub struct CarbonTracker {
    /// Current carbon intensity
    current_intensity: Option<CarbonIntensity>,

    /// Current energy cost
    current_cost: EnergyCost,

    /// API endpoint for carbon intensity data
    api_endpoint: Option<String>,

    /// Region for this cluster
    region: String,

    /// How often to refresh carbon data
    refresh_interval: Duration,
}

impl CarbonTracker {
    pub fn new() -> Self {
        Self {
            current_intensity: None,
            current_cost: EnergyCost {
                currency: "USD".into(),
                cost_per_kwh: 0.10, // Default $0.10/kWh
                region: "default".into(),
            },
            api_endpoint: std::env::var("CARBON_API_ENDPOINT").ok(),
            region: std::env::var("CARBON_REGION").unwrap_or_else(|_| "unknown".into()),
            refresh_interval: Duration::from_secs(300), // 5 minutes
        }
    }

    /// Get current carbon intensity.
    pub fn current(&self) -> Option<&CarbonIntensity> {
        self.current_intensity.as_ref()
    }

    /// Calculate carbon report for a given power level over an interval.
    pub fn calculate(
        &self,
        power_uw: u64,
        interval: Duration,
    ) -> Option<CarbonReport> {
        let intensity = self.current_intensity.as_ref()?;

        let power_watts = power_uw as f64 / 1e6;
        let hours = interval.as_secs_f64() / 3600.0;
        let energy_wh = power_watts * hours;
        let energy_kwh = energy_wh / 1000.0;

        let carbon_grams = energy_kwh * intensity.grams_co2_per_kwh;
        let cost = energy_kwh * self.current_cost.cost_per_kwh;

        // Annualize
        let intervals_per_year = 365.25 * 24.0 * 3600.0 / interval.as_secs_f64();

        Some(CarbonReport {
            power_watts,
            energy_wh,
            carbon_grams,
            carbon_kg_per_year: carbon_grams * intervals_per_year / 1000.0,
            cost,
            cost_per_year: cost * intervals_per_year,
            intensity: intensity.clone(),
            energy_cost: self.current_cost.clone(),
        })
    }

    /// Refresh carbon intensity from external API.
    async fn refresh(&mut self) -> Result<(), String> {
        let endpoint = match &self.api_endpoint {
            Some(ep) => ep.clone(),
            None => {
                // No API configured — use static fallback
                self.current_intensity = Some(CarbonIntensity {
                    grams_co2_per_kwh: 400.0, // Global average ~400 gCO2/kWh
                    source: "static_default".into(),
                    region: self.region.clone(),
                    timestamp: SystemTime::now(),
                    is_forecast: false,
                });
                return Ok(());
            }
        };

        // TODO: Implement actual API calls
        //
        // Electricity Maps:
        //   GET https://api.electricitymap.org/v3/carbon-intensity/latest
        //     ?zone={region}
        //   Response: { "carbonIntensity": 123.45, "datetime": "..." }
        //
        // WattTime:
        //   GET https://api.watttime.org/v3/signal-index
        //     ?region={region}
        //   Response: { "signal_type": "co2_moer", "value": 789.0 }
        //
        // For now, use static value
        self.current_intensity = Some(CarbonIntensity {
            grams_co2_per_kwh: 400.0,
            source: format!("static (endpoint configured: {})", endpoint),
            region: self.region.clone(),
            timestamp: SystemTime::now(),
            is_forecast: false,
        });

        Ok(())
    }
}

/// Background task that periodically refreshes carbon intensity data.
pub async fn run_updater(carbon: std::sync::Arc<tokio::sync::RwLock<CarbonTracker>>) {
    loop {
        {
            let mut tracker = carbon.write().await;
            if let Err(e) = tracker.refresh().await {
                log::warn!("Failed to refresh carbon intensity: {}", e);
            } else if let Some(intensity) = tracker.current() {
                log::info!(
                    "Carbon intensity: {:.0} gCO2/kWh (region: {}, source: {})",
                    intensity.grams_co2_per_kwh,
                    intensity.region,
                    intensity.source,
                );
            }
        }

        let interval = {
            let tracker = carbon.read().await;
            tracker.refresh_interval
        };

        tokio::time::sleep(interval).await;
    }
}
