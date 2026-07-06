// Badge — a compact labelled pill. `tone` maps to a role token:
//   - "neutral"  → surface/border (default, factual statements)
//   - "muted"    → dimmed (an un-measured / not-yet / gated marker; NOT an
//                  error — distinct from and quieter than "neutral")
//   - "accent"   → brand accent (differentiators)
//   - "pq"       → post-quantum marker (accent-tinted, with a lock glyph)
//   - status tones ("good" | "warning" | "serious" | "critical") are
//     reserved for state, never decoration.

import type { ReactNode } from "react";

export type BadgeTone =
  | "neutral"
  | "muted"
  | "accent"
  | "pq"
  | "good"
  | "warning"
  | "serious"
  | "critical";

export function Badge({
  tone = "neutral",
  children,
  title,
}: {
  tone?: BadgeTone;
  children: ReactNode;
  title?: string;
}) {
  return (
    <span className={`badge badge-${tone}`} title={title}>
      {tone === "pq" && (
        <svg
          className="badge-glyph"
          width="12"
          height="12"
          viewBox="0 0 24 24"
          aria-hidden="true"
        >
          <rect
            x="4"
            y="11"
            width="16"
            height="10"
            rx="2"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          />
          <path
            d="M8 11V8a4 4 0 018 0v3"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          />
        </svg>
      )}
      {children}
    </span>
  );
}
