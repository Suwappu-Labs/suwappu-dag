// LineChart — a single-series SVG line chart, hand-drawn (no chart
// library). Dataviz-compliant: one y-axis, recessive grid/axes, 2px
// line in --series-1, a hover crosshair + tooltip, and an accessible
// summary (the underlying table is the "table view" fallback).
//
// Single series ⇒ no legend; the panel title names the series.

import { useCallback, useLayoutEffect, useRef, useState } from "react";

export interface LinePoint {
  /** X value (e.g. DAG round). */
  x: number;
  /** Y value (e.g. intents in the block). */
  y: number;
  /** Optional extra line shown in the tooltip (e.g. short cert hash). */
  meta?: string;
}

const H = 200;
const PAD_L = 40;
const PAD_R = 14;
const PAD_T = 16;
const PAD_B = 28;

export function LineChart({
  points,
  yLabel,
  ariaLabel,
}: {
  points: LinePoint[];
  /** Short unit label for the tooltip value (e.g. "intents"). */
  yLabel: string;
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

  const n = points.length;
  const innerW = Math.max(1, width - PAD_L - PAD_R);
  const innerH = H - PAD_T - PAD_B;
  const yMax = Math.max(1, ...points.map((p) => p.y));

  const xAt = useCallback(
    (i: number) => (n <= 1 ? PAD_L + innerW / 2 : PAD_L + (i / (n - 1)) * innerW),
    [n, innerW],
  );
  const yAt = useCallback(
    (y: number) => PAD_T + innerH * (1 - y / yMax),
    [innerH, yMax],
  );

  const onMove = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      if (n === 0) return;
      const rect = e.currentTarget.getBoundingClientRect();
      const px = e.clientX - rect.left;
      const t = n <= 1 ? 0 : (px - PAD_L) / innerW;
      const i = Math.max(0, Math.min(n - 1, Math.round(t * (n - 1))));
      setHover(i);
    },
    [n, innerW],
  );

  if (n === 0) {
    return (
      <div className="chart-empty">No committed blocks in range yet.</div>
    );
  }

  const linePath = points
    .map((p, i) => `${i === 0 ? "M" : "L"}${xAt(i).toFixed(1)},${yAt(p.y).toFixed(1)}`)
    .join(" ");

  // Two recessive y gridlines: baseline (0) and yMax.
  const gridYs = [0, yMax];

  const hp = hover != null ? points[hover] : undefined;
  const hx = hover != null ? xAt(hover) : 0;
  const hy = hp ? yAt(hp.y) : 0;
  // Tooltip flips to the left of the crosshair near the right edge.
  const tipRight = hx > width - 130;

  return (
    <div
      className="line-chart"
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
        {/* Recessive grid + y labels */}
        {gridYs.map((gy) => (
          <g key={gy}>
            <line
              x1={PAD_L}
              x2={width - PAD_R}
              y1={yAt(gy)}
              y2={yAt(gy)}
              className="chart-grid"
            />
            <text x={PAD_L - 8} y={yAt(gy) + 4} className="chart-axis-label" textAnchor="end">
              {gy}
            </text>
          </g>
        ))}

        {/* X end labels (first / last round) */}
        <text x={xAt(0)} y={H - 8} className="chart-axis-label" textAnchor="start">
          {points[0]!.x}
        </text>
        {n > 1 && (
          <text
            x={xAt(n - 1)}
            y={H - 8}
            className="chart-axis-label"
            textAnchor="end"
          >
            {points[n - 1]!.x}
          </text>
        )}

        {/* The single data series */}
        <path d={linePath} className="chart-line" fill="none" />
        {n === 1 && <circle cx={xAt(0)} cy={yAt(points[0]!.y)} r="3.5" className="chart-dot" />}

        {/* Hover crosshair + marker */}
        {hp && (
          <g aria-hidden="true">
            <line x1={hx} x2={hx} y1={PAD_T} y2={H - PAD_B} className="chart-crosshair" />
            <circle cx={hx} cy={hy} r="4.5" className="chart-dot" />
          </g>
        )}
      </svg>

      {hp && (
        <div
          className="chart-tooltip"
          style={{
            left: tipRight ? undefined : hx,
            right: tipRight ? width - hx : undefined,
            top: PAD_T,
          }}
        >
          <div className="tt-round">round {hp.x}</div>
          <div className="tt-value">
            {hp.y} {yLabel}
          </div>
          {hp.meta && <div className="tt-meta">{hp.meta}</div>}
        </div>
      )}
    </div>
  );
}
