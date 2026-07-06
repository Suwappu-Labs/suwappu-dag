// Capabilities — a sober, factual capability inventory (route
// #/capabilities).
//
// Fetches the committed `/capabilities.json` and renders each group as a
// titled Card of item cards. Deliberately un-hyped: status is conveyed by
// a Badge tone + a plain label, invariants by a small chip, and every
// doc link points straight at the repository.

import { useEffect, useState } from "react";
import { Card } from "../components/Card.js";
import { Badge } from "../components/Badge.js";
import type { BadgeTone } from "../components/Badge.js";
import { Loading, EmptyState, ErrorState } from "../components/states.js";
import { fetchCapabilities } from "../lib/capabilities.js";
import type {
  CapabilitiesDoc,
  CapabilityStatus,
} from "../lib/capabilities.js";

const STATUS_TONE: Record<CapabilityStatus, BadgeTone> = {
  shipped: "good", // implemented, reviewed, CI-green
  "in-progress": "accent", // landed / ratified, remaining work named
  planned: "muted", // designed, gated on a dependency
};

const STATUS_LABEL: Record<CapabilityStatus, string> = {
  shipped: "shipped",
  "in-progress": "in progress",
  planned: "planned",
};

type Load =
  | { status: "loading" }
  | { status: "ok"; doc: CapabilitiesDoc }
  | { status: "error"; message: string };

export function Capabilities() {
  const [load, setLoad] = useState<Load>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;
    fetchCapabilities()
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
    <div className="page capabilities">
      <div className="page-intro">
        <h1>Capabilities</h1>
        <p className="lede">
          Honest capability status for the SUWAPPU DAG Layer 1 — what is
          shipped, what is in progress, and what is planned. Links point at
          the repository.
        </p>
      </div>

      {load.status === "ok" && load.doc.positioning && (
        <p className="cap-positioning">{load.doc.positioning}</p>
      )}

      {load.status === "loading" && <Loading />}
      {load.status === "error" && <ErrorState message={load.message} />}
      {load.status === "ok" &&
        (load.doc.groups.length === 0 ? (
          <EmptyState title="No capabilities published yet." />
        ) : (
          <>
            {load.doc.groups.map((g) => (
              <Card title={g.title} key={g.title}>
                <ul className="cap-list">
                  {g.items.map((it) => (
                    <li className="cap-item" key={it.name}>
                      <div className="cap-item-head">
                        <span className="cap-name">{it.name}</span>
                        <span className="cap-badges">
                          <Badge tone={STATUS_TONE[it.status]}>
                            {STATUS_LABEL[it.status]}
                          </Badge>
                          {it.invariant && (
                            <Badge
                              tone="accent"
                              title="A load-bearing invariant of the protocol — code that weakens it does not ship."
                            >
                              load-bearing invariant
                            </Badge>
                          )}
                        </span>
                      </div>
                      <p className="cap-note">{it.note}</p>
                      {it.doc && (
                        <a
                          className="cap-docs"
                          href={load.doc.repoBase + it.doc}
                          target="_blank"
                          rel="noreferrer"
                        >
                          docs ↗
                        </a>
                      )}
                    </li>
                  ))}
                </ul>
              </Card>
            ))}
            {load.doc.note && (
              <p className="cap-caption">
                As of {load.doc.asOf}. {load.doc.note}
              </p>
            )}
          </>
        ))}
    </div>
  );
}
