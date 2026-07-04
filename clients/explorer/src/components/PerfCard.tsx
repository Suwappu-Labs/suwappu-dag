// PerfCard — the data-driven performance panel for the Overview.
//
// Fetches the committed `/perf.json` once on mount and renders each
// metric as a compact tile. Honest by construction: it renders exactly
// what the JSON says — a null value shows as an em-dash with the metric's
// note, never an invented number. `pending` is styled muted, NOT as an
// error color: it means "not yet measured", not "broken".

import { useEffect, useState } from "react";
import { Card } from "./Card.js";
import { Badge } from "./Badge.js";
import type { BadgeTone } from "./Badge.js";
import { Loading, EmptyState, ErrorState } from "./states.js";
import { fetchPerf } from "../lib/perf.js";
import type { PerfDoc, PerfStatus } from "../lib/perf.js";

const STATUS_TONE: Record<PerfStatus, BadgeTone> = {
  published: "neutral", // a factual, published figure
  observed: "accent", // live / measured on this surface
  pending: "muted", // not yet measured — not an error
};

type Load =
  | { status: "loading" }
  | { status: "ok"; doc: PerfDoc }
  | { status: "error"; message: string };

export function PerfCard() {
  const [load, setLoad] = useState<Load>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;
    fetchPerf()
      .then((doc) => {
        if (!cancelled) setLoad({ status: "ok", doc });
      })
      .catch((e) => {
        if (!cancelled) {
          setLoad({
            status: "error",
            message: e instanceof Error ? e.message : String(e),
          });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <Card
      title="Performance"
      subtitle="Measured figures from the last published devnet campaign. “Pending” means not yet measured at scale — never a marketing target."
    >
      {load.status === "loading" && <Loading />}
      {load.status === "error" && <ErrorState message={load.message} />}
      {load.status === "ok" &&
        (load.doc.metrics.length === 0 ? (
          <EmptyState title="No performance metrics published yet." />
        ) : (
          <>
            <div className="perf-grid">
              {load.doc.metrics.map((m) => (
                <div className="perf-tile" key={m.key}>
                  <div className="stat-label">{m.label}</div>
                  <div className="stat-value">
                    {m.value ?? "—"}
                    {m.value != null && m.unit && (
                      <span className="stat-unit">{m.unit}</span>
                    )}
                  </div>
                  <div className="perf-tile-meta">
                    <Badge tone={STATUS_TONE[m.status]}>{m.status}</Badge>
                    {m.asOf && (
                      <span className="perf-asof">as of {m.asOf}</span>
                    )}
                  </div>
                  {m.note && <p className="perf-note">{m.note}</p>}
                </div>
              ))}
            </div>
            {load.doc.sourceUrl && (
              <p className="perf-source">
                source: 4-region AWS · {load.doc.asOf} ·{" "}
                <a
                  href={load.doc.sourceUrl}
                  target="_blank"
                  rel="noreferrer"
                >
                  docs ↗
                </a>
              </p>
            )}
          </>
        ))}
    </Card>
  );
}
