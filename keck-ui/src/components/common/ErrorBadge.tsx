// SPDX-License-Identifier: Apache-2.0

import { formatErrorRatio, errorColor } from "@/utils/format";

interface ErrorBadgeProps {
  ratio: number;
}

export function ErrorBadge({ ratio }: ErrorBadgeProps) {
  const color = errorColor(ratio);
  const className =
    ratio <= 0.05 ? "badge badge-green" :
    ratio <= 0.15 ? "badge badge-yellow" :
    "badge badge-red";

  return (
    <span className={className}>
      {formatErrorRatio(ratio)}
    </span>
  );
}
