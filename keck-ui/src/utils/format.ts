// SPDX-License-Identifier: Apache-2.0

// Formatting utilities for power, energy, carbon, and cost values.

/** Format watts with appropriate unit (W, kW, MW) */
export function formatWatts(watts: number): string {
  if (watts >= 1_000_000) return `${(watts / 1_000_000).toFixed(1)} MW`;
  if (watts >= 1_000) return `${(watts / 1_000).toFixed(1)} kW`;
  if (watts >= 1) return `${watts.toFixed(1)} W`;
  return `${(watts * 1000).toFixed(0)} mW`;
}

/** Format carbon emissions (grams, kg, tonnes) */
export function formatCarbon(gramsPerHour: number): string {
  const kgPerDay = (gramsPerHour * 24) / 1000;
  if (kgPerDay >= 1000) return `${(kgPerDay / 1000).toFixed(1)} t/day`;
  if (kgPerDay >= 1) return `${kgPerDay.toFixed(1)} kg/day`;
  return `${(gramsPerHour * 24).toFixed(0)} g/day`;
}

/** Format carbon intensity */
export function formatIntensity(gramsPerKwh: number): string {
  return `${gramsPerKwh.toFixed(0)} gCO\u2082/kWh`;
}

/** Format cost */
export function formatCost(costPerHour: number, currency: string = "USD"): string {
  const perDay = costPerHour * 24;
  const perMonth = perDay * 30.44;
  const symbol = currency === "USD" ? "$" : currency === "EUR" ? "\u20AC" : currency;

  if (perMonth >= 1000) return `${symbol}${(perMonth / 1000).toFixed(1)}k/mo`;
  if (perMonth >= 1) return `${symbol}${perMonth.toFixed(0)}/mo`;
  return `${symbol}${(perDay).toFixed(2)}/day`;
}

/** Format error ratio as percentage */
export function formatErrorRatio(ratio: number): string {
  return `${(ratio * 100).toFixed(1)}%`;
}

/** Format frequency in GHz */
export function formatFrequency(khz: number): string {
  return `${(khz / 1_000_000).toFixed(1)} GHz`;
}

/** Color for error ratio (green → yellow → red) */
export function errorColor(ratio: number): string {
  if (ratio <= 0.05) return "#22c55e"; // green
  if (ratio <= 0.15) return "#eab308"; // yellow
  return "#ef4444"; // red
}

/** Color for carbon intensity (green → yellow → red) */
export function carbonColor(gramsPerKwh: number): string {
  if (gramsPerKwh <= 100) return "#22c55e";
  if (gramsPerKwh <= 300) return "#eab308";
  return "#ef4444";
}
