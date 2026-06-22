// Home page — recent blocks + live tip.
//
// Polls `suwappu_getEpoch` every 3 seconds to discover the chain head;
// renders the last ~30 blocks in reverse order. Skip rounds
// (Mysticeti-C `Skip` outcome) are elided — `suwappu_getBlock` returns
// null for them, and we just don't render them.

import { useEffect, useState } from "react";
import type { Client, BlockView, EpochView } from "@suwappu/client";

const RECENT_COUNT = 30;
const POLL_MS = 3000;

export function Home({ client }: { client: Client }) {
  const [epoch, setEpoch] = useState<EpochView | null>(null);
  const [recent, setRecent] = useState<BlockView[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function refresh() {
      try {
        const ep = await client.getEpoch();
        if (cancelled) return;
        setEpoch(ep);
        setError(null);

        // Pull the last RECENT_COUNT non-skipped blocks. Walk
        // backwards from the tip; skip nulls.
        const tip = Number(ep.latest_committed_round);
        const blocks: BlockView[] = [];
        for (
          let r = tip;
          r >= 0 && blocks.length < RECENT_COUNT;
          r--
        ) {
          const b = await client.getBlock(r);
          if (cancelled) return;
          if (b !== null) {
            blocks.push(b);
          }
        }
        if (cancelled) return;
        setRecent(blocks);
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
        }
      }
    }

    refresh();
    const id = setInterval(refresh, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [client]);

  return (
    <div className="home">
      <section className="tip">
        <h1>Chain head</h1>
        {epoch ? (
          <dl>
            <dt>Current round</dt>
            <dd>{epoch.latest_committed_round.toLocaleString()}</dd>
            <dt>Current epoch</dt>
            <dd>{epoch.current.toLocaleString()}</dd>
            <dt>Last epoch boundary</dt>
            <dd>{epoch.last_boundary_round.toLocaleString()}</dd>
            <dt>Rounds per epoch</dt>
            <dd>{epoch.rounds_per_epoch.toLocaleString()}</dd>
          </dl>
        ) : error ? (
          <div className="error">RPC error: {error}</div>
        ) : (
          <div className="loading">Loading…</div>
        )}
      </section>

      <section className="recent">
        <h2>Recent blocks</h2>
        {recent.length === 0 && !error && (
          <div className="loading">Loading…</div>
        )}
        <table>
          <thead>
            <tr>
              <th>Round</th>
              <th>Cert hash</th>
              <th>Intents</th>
            </tr>
          </thead>
          <tbody>
            {recent.map((b) => (
              <tr key={b.round}>
                <td>
                  <a href={`#/block/${b.round}`}>{b.round}</a>
                </td>
                <td className="hash">
                  <a href={`#/block/${b.round}`}>{shortHash(b.cert_hash)}</a>
                </td>
                <td>{b.intents.length}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>
    </div>
  );
}

function shortHash(hex: string): string {
  if (hex.length <= 18) return hex;
  return `${hex.slice(0, 10)}…${hex.slice(-8)}`;
}
