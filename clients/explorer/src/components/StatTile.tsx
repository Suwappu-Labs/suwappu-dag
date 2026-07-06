// StatTile — a hero number with a small label. NOT a chart: no axes,
// no plot. Used for the network-overview KPI row.

import type { ReactNode } from "react";

export function StatTile({
  label,
  value,
  hint,
  capacity,
  atCapacity = false,
}: {
  label: string;
  value: ReactNode;
  /** Optional secondary line under the value (e.g. "of 40"). */
  hint?: ReactNode;
  /** When set, renders "value / capacity" with the capacity recessive. */
  capacity?: ReactNode;
  /** Flags the tile as at-capacity (uses a status color on the hint). */
  atCapacity?: boolean;
}) {
  return (
    <div className="stat-tile">
      <div className="stat-label">{label}</div>
      <div className="stat-value">
        {value}
        {capacity != null && (
          <span className="stat-capacity"> / {capacity}</span>
        )}
      </div>
      {hint != null && (
        <div className={`stat-hint${atCapacity ? " at-capacity" : ""}`}>
          {hint}
        </div>
      )}
    </div>
  );
}
