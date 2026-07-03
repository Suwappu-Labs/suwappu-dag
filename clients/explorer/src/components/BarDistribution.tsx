// BarDistribution — a single-series vertical bar chart (SVG, hand-drawn)
// for a ring's stake distribution. Bars anchored to the baseline with a
// 4px rounded top, --series-1 fill, recessive baseline. Nearest-column
// hover tooltip (hit target is the full column, not just the thin bar).
//
// Single series ⇒ no legend; the panel title names it.

import { useCallback, useLayoutEffect, useRef, useState } from "react";

export interface Bar {
  /** Slot id (label). */
  id: number;
  /** Bar height as a fraction 0..1 of the ring max. */
  fraction: number;
  /** Pre-formatted stake string for the tooltip. */
  label: string;
}

const H = 120;
const PAD_T = 10;
const PAD_B = 18;

export function BarDistribution({
  bars,
  ariaLabel,
}: {
  bars: Bar[];
  ariaLabel: string;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(640);
  const [hover, setHover] = useState<number | null>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w && w > 0) setWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const n = bars.length;
  const innerH = H - PAD_T - PAD_B;
  const slot = n > 0 ? width / n : width;
  const barW = Math.max(1.5, Math.min(slot * 0.7, 22));

  const onMove = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (n === 0) return;
      const rect = e.currentTarget.getBoundingClientRect();
      const px = e.clientX - rect.left;
      const i = Math.max(0, Math.min(n - 1, Math.floor(px / slot)));
      setHover(i);
    },
    [n, slot],
  );

  if (n === 0) return null;

  const hb = hover != null ? bars[hover] : undefined;
  const hcx = hover != null ? hover * slot + slot / 2 : 0;
  const tipRight = hcx > width - 130;

  return (
    <div
      className="bar-dist"
      ref={ref}
      onMouseMove={onMove}
      onMouseLeave={() => setHover(null)}
    >
      <svg
        width={width}
        height={H}
        viewBox={`0 0 ${width} ${H}`}
        role="img"
        aria-label={ariaLabel}
      >
        <line
          x1={0}
          x2={width}
          y1={H - PAD_B}
          y2={H - PAD_B}
          className="chart-grid"
        />
        {bars.map((b, i) => {
          const h = Math.max(1, b.fraction * innerH);
          const cx = i * slot + slot / 2;
          const active = i === hover;
          return (
            <rect
              key={b.id}
              x={cx - barW / 2}
              y={H - PAD_B - h}
              width={barW}
              height={h}
              rx="2.5"
              className={`chart-bar${active ? " active" : ""}`}
            />
          );
        })}
      </svg>
      {hb && (
        <div
          className="chart-tooltip"
          style={{
            left: tipRight ? undefined : hcx,
            right: tipRight ? width - hcx : undefined,
            top: PAD_T,
          }}
        >
          <div className="tt-round">slot {hb.id}</div>
          <div className="tt-value">{hb.label}</div>
        </div>
      )}
    </div>
  );
}
