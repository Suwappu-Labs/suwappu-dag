// Block detail — round, cert hash, intent list with tx-hash links.

import { useEffect, useState } from "react";
import type { Client, BlockView } from "@suwappu/client";
import { Card } from "../components/Card.js";
import { Hash } from "../components/Hash.js";
import { Loading, EmptyState, ErrorState } from "../components/states.js";
import { shortHash } from "../lib/format.js";

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
    setBlock(null);
    setError(null);
    setNotFound(false);
    client
      .getBlock(round)
      .then((b) => {
        if (cancelled) return;
        if (b === null) setNotFound(true);
        else setBlock(b);
      })
      .catch((e) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [client, round]);

  return (
    <div className="page block-page">
      <div className="page-intro">
        <h1>Block {round.toLocaleString()}</h1>
      </div>

      {error ? (
        <Card>
          <ErrorState message={error} />
        </Card>
      ) : notFound ? (
        <Card>
          <EmptyState title={`No block committed at round ${round}.`}>
            Either the round is in the future, or it was skipped (Mysticeti-C{" "}
            <em>Skip</em> outcome — a legitimate gap; the next round will have a
            block).
            <div className="pager">
              <span />
              <a href={`#/block/${round + 1}`}>Try the next round →</a>
            </div>
          </EmptyState>
        </Card>
      ) : !block ? (
        <Card>
          <Loading />
        </Card>
      ) : (
        <>
          <Card title="Header">
            <dl className="detail-grid">
              <dt>Round</dt>
              <dd>{block.round.toLocaleString()}</dd>
              <dt>Cert hash</dt>
              <dd>
                <Hash value={block.cert_hash} label="cert hash" />
              </dd>
              <dt>Intent count</dt>
              <dd>{block.intents.length}</dd>
            </dl>
          </Card>

          <Card title="Intents">
            {block.intents.length === 0 ? (
              <EmptyState title="Empty block.">
                Committed but carries no intents (governance-only or a
                checkpoint commit).
              </EmptyState>
            ) : (
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th className="num">#</th>
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
                          <td className="num">{idx}</td>
                          <td>
                            <code>{intent.kind}</code>
                          </td>
                          <td>
                            {txHash ? (
                              <a href={`#/tx/${txHash}`} className="mono">
                                {shortHash(txHash)}
                              </a>
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
              </div>
            )}
          </Card>

          <nav className="pager">
            {round > 0 ? (
              <a href={`#/block/${round - 1}`}>← previous round</a>
            ) : (
              <span />
            )}
            <a href={`#/block/${round + 1}`}>next round →</a>
          </nav>
        </>
      )}
    </div>
  );
}

function IntentSummary({ intent }: { intent: BlockView["intents"][number] }) {
  switch (intent.kind) {
    case "transfer":
      return (
        <span className="mono">
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
      // string this build doesn't recognize. Don't crash; show raw JSON.
      return <code>{JSON.stringify(intent)}</code>;
  }
}
