// SPDX-License-Identifier: Apache-2.0

export function formatWatts(watts: number): string {
  if (watts >= 1_000_000) return `${(watts / 1_000_000).toFixed(1)} MW`;
  if (watts >= 1_000) return `${(watts / 1_000).toFixed(1)} kW`;
  if (watts >= 1) return `${watts.toFixed(1)} W`;
  return `${(watts * 1000).toFixed(0)} mW`;
}

export function formatCarbon(gramsPerHour: number): string {
  const kgPerDay = (gramsPerHour * 24) / 1000;
  if (kgPerDay >= 1000) return `${(kgPerDay / 1000).toFixed(1)} t/day`;
  if (kgPerDay >= 1) return `${kgPerDay.toFixed(1)} kg/day`;
  return `${(gramsPerHour * 24).toFixed(0)} g/day`;
}

export function formatCost(costPerHour: number, currency: string = "USD"): string {
  const perMonth = costPerHour * 24 * 30.44;
  const symbol = currency === "USD" ? "$" : currency === "EUR" ? "\u20AC" : currency;
  if (perMonth >= 1000) return `${symbol}${(perMonth / 1000).toFixed(1)}k/mo`;
  return `${symbol}${perMonth.toFixed(0)}/mo`;
}

export function formatErrorRatio(ratio: number): string {
  return `${(ratio * 100).toFixed(1)}%`;
}

export type StatusType = "success" | "warning" | "danger";

export function errorStatus(ratio: number): StatusType {
  if (ratio <= 0.05) return "success";
  if (ratio <= 0.15) return "warning";
  return "danger";
}
