// Performance metrics document, served as a committed static file at
// `/perf.json` (Vite serves `public/` at the site root). These are
// honest, dated figures from the last published devnet campaign — the
// values are authored by hand and this module only types + fetches them.

/** Provenance of a metric. `pending` is *not* an error — just unmeasured. */
export type PerfStatus = "published" | "observed" | "pending";

export interface PerfMetric {
  key: string;
  label: string;
  /** Display value as a string ("100", "5.75", "28–73"), or null when unmeasured. */
  value: string | null;
  unit: string;
  status: PerfStatus;
  /** ISO date this specific figure was measured (optional). */
  asOf?: string;
  note?: string;
}

export interface PerfDoc {
  asOf: string;
  campaign?: string;
  source?: string;
  sourceUrl?: string;
  note?: string;
  metrics: PerfMetric[];
}

/**
 * Fetch the committed `/perf.json`. Throws on a network failure or a
 * non-2xx response; the caller renders that as an error state and falls
 * back gracefully. No values are computed here — the JSON is authoritative.
 */
export async function fetchPerf(): Promise<PerfDoc> {
  const resp = await fetch("/perf.json", { cache: "no-store" });
  if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
  return (await resp.json()) as PerfDoc;
}
