// SPDX-License-Identifier: Apache-2.0

interface StatCardProps {
  title: string;
  value: string;
  subtitle?: string;
  color?: string;
}

export function StatCard({ title, value, subtitle, color }: StatCardProps) {
  return (
    <div className="card">
      <div className="card-title">{title}</div>
      <div className="card-value" style={color ? { color } : undefined}>
        {value}
      </div>
      {subtitle && <div className="card-subtitle">{subtitle}</div>}
    </div>
  );
}
