// Block detail — round, cert hash, intent list with tx-hash links.

import { useEffect, useState } from "react";
import type { Client, BlockView } from "@suwappu/client";

export function BlockPage({
  client,
  round,
}: {
  client: Client;
  round: number;
}) {
  const [block, setBlock] = useState<BlockView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notFound, setNotFound] = useState(false);

  useEffect(() => {
    let cancelled = false;
    client
      .getBlock(round)
      .then((b) => {
        if (cancelled) return;
        if (b === null) {
          setNotFound(true);
        } else {
          setBlock(b);
          setNotFound(false);
        }
      })
      .catch((e) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [client, round]);

  if (error) {
    return (
      <div className="block-page">
        <h1>Block {round}</h1>
        <div className="error">RPC error: {error}</div>
      </div>
    );
  }
  if (notFound) {
    return (
      <div className="block-page">
        <h1>Block {round}</h1>
        <div className="not-found">
          No block committed at round {round}. Either the round is in the
          future, or the round was skipped (DagBft-C <em>Skip</em>{" "}
          outcome — a legitimate gap; the next round will have a block).
        </div>
        <div>
          <a href={`#/block/${round + 1}`}>Try the next round →</a>
        </div>
      </div>
    );
  }
  if (!block) {
    return (
      <div className="block-page">
        <h1>Block {round}</h1>
        <div className="loading">Loading…</div>
      </div>
    );
  }

  return (
    <div className="block-page">
      <h1>Block {block.round}</h1>
      <dl>
        <dt>Round</dt>
        <dd>{block.round}</dd>
        <dt>Cert hash</dt>
        <dd className="hash">{block.cert_hash}</dd>
        <dt>Intent count</dt>
        <dd>{block.intents.length}</dd>
      </dl>

      <h2>Intents</h2>
      {block.intents.length === 0 ? (
        <div className="empty">
          Empty block — committed but no intents (governance-only or a
          checkpoint commit).
        </div>
      ) : (
        <table>
          <thead>
            <tr>
              <th>Index</th>
              <th>Kind</th>
              <th>Tx hash</th>
              <th>Detail</th>
            </tr>
          </thead>
          <tbody>
            {block.intents.map((intent, idx) => {
              const txHash = block.tx_hashes?.[idx] ?? null;
              return (
                <tr key={idx}>
                  <td>{idx}</td>
                  <td>
                    <code>{intent.kind}</code>
                  </td>
                  <td className="hash">
                    {txHash ? (
                      <a href={`#/tx/${txHash}`}>{shortHash(txHash)}</a>
                    ) : (
                      <span className="muted">—</span>
                    )}
                  </td>
                  <td>
                    <IntentSummary intent={intent} />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <nav className="pager">
        {round > 0 && <a href={`#/block/${round - 1}`}>← previous round</a>}
        <a href={`#/block/${round + 1}`}>next round →</a>
      </nav>
    </div>
  );
}

function IntentSummary({ intent }: { intent: BlockView["intents"][number] }) {
  switch (intent.kind) {
    case "transfer":
      return (
        <span>
          {shortHash(intent.from)} → {shortHash(intent.to)}: {intent.amount}{" "}
          SUWAPPU
        </span>
      );
    case "admit_authority":
      return <span>admit authority_id={intent.authority_id}</span>;
    case "exit_authority":
      return <span>exit authority_id={intent.authority_id}</span>;
    case "eject_authority":
      return <span>eject authority_id={intent.authority_id}</span>;
    case "unknown":
      return (
        <span className="muted" title={intent.kind_hint}>
          unknown ({intent.kind_hint})
        </span>
      );
    default:
      // Forward-compat: a future Intent variant may surface a kind
      // string this build doesn't recognize. Don't crash; just show
      // raw JSON.
      return <code>{JSON.stringify(intent)}</code>;
  }
}

function shortHash(hex: string): string {
  if (hex.length <= 18) return hex;
  return `${hex.slice(0, 10)}…${hex.slice(-8)}`;
}
