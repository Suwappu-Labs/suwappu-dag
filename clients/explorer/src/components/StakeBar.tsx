// StakeBar — an inline horizontal bar (SVG) sized to a fraction of the
// ring's max stake. Single --series-1 fill, 4px rounded data-end
// anchored to the baseline (left). Used in per-member validator rows.

export function StakeBar({
  fraction,
  ariaLabel,
}: {
  /** 0..1 — this member's stake relative to the ring's max stake. */
  fraction: number;
  ariaLabel: string;
}) {
  const pct = Math.max(0, Math.min(1, fraction)) * 100;
  return (
    <div className="stake-bar" role="img" aria-label={ariaLabel}>
      <div className="stake-bar-track">
        <div
          className="stake-bar-fill"
          style={{ width: `${pct.toFixed(1)}%` }}
        />
      </div>
    </div>
  );
}
