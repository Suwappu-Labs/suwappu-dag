// Hash — monospace hash display with a copy-to-clipboard affordance.
//
// `value` is the full string that gets copied; `display` (optional)
// overrides the visible text (e.g. a truncated form). When `truncate`
// is set and no `display` is given, the value is shortened as
// `head…tail`.

import { useState } from "react";

function shorten(hex: string, head = 10, tail = 8): string {
  if (hex.length <= head + tail + 1) return hex;
  return `${hex.slice(0, head)}…${hex.slice(-tail)}`;
}

export function Hash({
  value,
  display,
  truncate = false,
  head = 10,
  tail = 8,
  label,
}: {
  value: string;
  display?: string;
  truncate?: boolean;
  head?: number;
  tail?: number;
  /** Accessible label for the copy button, e.g. "cert hash". */
  label?: string;
}) {
  const [copied, setCopied] = useState(false);
  const text = display ?? (truncate ? shorten(value, head, tail) : value);

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard API unavailable (insecure context) — no-op; the
      // value is still visible and selectable.
    }
  }

  return (
    <span className="hash-wrap">
      <span className="hash" title={value}>
        {text}
      </span>
      <button
        type="button"
        className="copy-btn"
        onClick={copy}
        aria-label={copied ? "Copied" : `Copy ${label ?? "value"}`}
        title={copied ? "Copied" : "Copy"}
      >
        {copied ? (
          <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true">
            <path
              d="M20 6L9 17l-5-5"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        ) : (
          <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true">
            <rect
              x="9"
              y="9"
              width="11"
              height="11"
              rx="2"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
            />
            <path
              d="M5 15V5a2 2 0 012-2h8"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
            />
          </svg>
        )}
      </button>
    </span>
  );
}
