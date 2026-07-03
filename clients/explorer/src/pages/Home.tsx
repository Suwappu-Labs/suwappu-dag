// Overview — the institutional network-overview dashboard (route #/).
//
// Live KPIs + safety/crypto posture + round-cadence chart + recent
// blocks. Epoch and recent blocks poll every 3s (batched parallel
// fetch); the ring registries are fetched once (they change rarely).

import { useEffect, useState } from "react";
import type {
  Client,
  BlockView,
  EpochView,
  AuthorityMemberView,
  ValidatorMemberView,
} from "@suwappu/client";
import { Card } from "../components/Card.js";
import { StatTile } from "../components/StatTile.js";
import { Badge } from "../components/Badge.js";
import { LineChart } from "../components/LineChart.js";
import type { LinePoint } from "../components/LineChart.js";
import { Loading, ErrorState } from "../components/states.js";
import { Hash } from "../components/Hash.js";
import { fetchRecentBlocks } from "../lib/blocks.js";
import { shortHash } from "../lib/format.js";

const POLL_MS = 3000;
const AUTHORITY_CAP = 40;
const VALIDATOR_CAP = 200;

export function Home({ client }: { client: Client }) {
  const [epoch, setEpoch] = useState<EpochView | null>(null);
  const [recent, setRecent] = useState<BlockView[]>([]);
  const [authority, setAuthority] = useState<AuthorityMemberView[] | null>(null);
  const [validator, setValidator] = useState<ValidatorMemberView[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Ring registries: fetched once (capacity/size rarely changes).
  useEffect(() => {
    let cancelled = false;
    Promise.all([
      client.getAuthorityRegistry(),
      client.getValidatorRegistry(),
    ])
      .then(([a, v]) => {
        if (cancelled) return;
        setAuthority(a);
        setValidator(v);
      })
      .catch(() => {
        // Non-fatal for the overview — the KPI tiles degrade to "—".
      });
    return () => {
      cancelled = true;
    };
  }, [client]);

  // Live tip + recent blocks: polled, batched parallel fetch.
  useEffect(() => {
    let cancelled = false;
    async function refresh() {
      try {
        const ep = await client.getEpoch();
        if (cancelled) return;
        setEpoch(ep);
        const blocks = await fetchRecentBlocks(
          client,
          Number(ep.latest_committed_round),
        );
        if (cancelled) return;
        setRecent(blocks);
        setError(null);
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      }
    }
    refresh();
    const id = window.setInterval(refresh, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [client]);

  // Chart wants oldest→newest; `recent` is newest-first.
  const chartPoints: LinePoint[] = [...recent]
    .reverse()
    .map((b) => ({
      x: b.round,
      y: b.intents.length,
      meta: shortHash(b.cert_hash),
    }));

  const authAtCap = authority != null && authority.length >= AUTHORITY_CAP;
  const valAtCap = validator != null && validator.length >= VALIDATOR_CAP;

  return (
    <div className="page overview">
      <div className="page-intro">
        <h1>Network overview</h1>
        <p className="lede">
          A post-quantum, dual-ring settlement chain on a Mysticeti-C
          certificate DAG. Live state below is served read-only from the
          devnet RPC.
        </p>
      </div>

      {error && !epoch && <ErrorState message={error} />}

      <div className="stat-row">
        <StatTile
          label="Committed round"
          value={epoch ? epoch.latest_committed_round.toLocaleString() : "—"}
        />
        <StatTile
          label="Epoch"
          value={epoch ? epoch.current.toLocaleString() : "—"}
          hint={
            epoch
              ? `began at round ${epoch.last_boundary_round.toLocaleString()}`
              : undefined
          }
        />
        <StatTile
          label="Rounds / epoch"
          value={epoch ? epoch.rounds_per_epoch.toLocaleString() : "—"}
        />
        <StatTile
          label="Authority Ring"
          value={authority ? authority.length : "—"}
          capacity={AUTHORITY_CAP}
          hint={authAtCap ? "at capacity" : "seated"}
          atCapacity={authAtCap}
        />
        <StatTile
          label="Validator Ring"
          value={validator ? validator.length : "—"}
          capacity={VALIDATOR_CAP}
          hint={valAtCap ? "at capacity" : "seated"}
          atCapacity={valAtCap}
        />
      </div>

      <Card
        title="Safety & crypto posture"
        subtitle="Load-bearing invariants of the SUWAPPU DAG Layer 1."
        className="posture"
      >
        <ul className="posture-list">
          <li>
            <Badge tone="accent">Joint-quorum AND-gate</Badge>
            <p>
              A safety violation requires Byzantine corruption of{" "}
              <em>both</em> the 40-slot Authority Ring and the 200-slot
              Validator Ring simultaneously. Neither ring is a single point
              of failure.
            </p>
          </li>
          <li>
            <Badge tone="pq">Post-quantum</Badge>
            <p>
              Long-lived integrity surfaces use NIST-standardized primitives:
              ML-DSA-65 (FIPS 204) for intent and header signing, ML-KEM-768
              (FIPS 203) for confidential transfers.
            </p>
          </li>
          <li>
            <Badge tone="accent">Fast-path lane</Badge>
            <p>
              Single-owner transactions confirm on a low-latency fast path.
              An Authority Node that equivocates on a fast-path certificate
              forfeits 100% of bonded stake plus expulsion.
            </p>
          </li>
        </ul>
      </Card>

      <Card
        title="Round cadence"
        subtitle="Intents per committed block, most recent rounds."
      >
        {recent.length === 0 ? (
          error ? (
            <ErrorState message={error} />
          ) : (
            <Loading />
          )
        ) : (
          <LineChart
            points={chartPoints}
            yLabel="intents"
            ariaLabel={`Intents per committed block across rounds ${
              chartPoints[0]?.x ?? ""
            } to ${chartPoints[chartPoints.length - 1]?.x ?? ""}. Table of the same blocks follows below.`}
          />
        )}
      </Card>

      <Card title="Recent blocks">
        {recent.length === 0 && !error ? (
          <Loading />
        ) : (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Round</th>
                  <th>Cert hash</th>
                  <th className="num">Intents</th>
                </tr>
              </thead>
              <tbody>
                {recent.map((b) => (
                  <tr key={b.round}>
                    <td>
                      <a href={`#/block/${b.round}`}>{b.round}</a>
                    </td>
                    <td>
                      <Hash
                        value={b.cert_hash}
                        truncate
                        label="cert hash"
                      />
                    </td>
                    <td className="num">{b.intents.length}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}
