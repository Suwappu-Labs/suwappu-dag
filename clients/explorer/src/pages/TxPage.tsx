// Transaction detail — looks up via suwappu_getTransaction.

import { useEffect, useState } from "react";
import type { Client, TransactionView } from "@suwappu/client";
import { Card } from "../components/Card.js";
import { Hash } from "../components/Hash.js";
import { Loading, EmptyState, ErrorState } from "../components/states.js";

export function TxPage({ client, hash }: { client: Client; hash: string }) {
  const [tx, setTx] = useState<TransactionView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notFound, setNotFound] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setTx(null);
    setError(null);
    setNotFound(false);
    // Convert the 0x-hex hash to a Uint8Array for the SDK call.
    const trimmed = hash.startsWith("0x") ? hash.slice(2) : hash;
    const bytes = new Uint8Array(
      trimmed.match(/.{2}/g)!.map((b) => parseInt(b, 16)),
    );
    if (bytes.length !== 32) {
      setError(`Expected 32-byte tx hash, got ${bytes.length} bytes`);
      return;
    }
    client
      .getTransaction(bytes)
      .then((t) => {
        if (cancelled) return;
        if (t === null) setNotFound(true);
        else setTx(t);
      })
      .catch((e) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [client, hash]);

  return (
    <div className="page tx-page">
      <div className="page-intro">
        <h1>Transaction</h1>
        <div className="subhash">
          <Hash value={hash} truncate head={14} tail={12} label="tx hash" />
        </div>
      </div>

      {error ? (
        <Card>
          <ErrorState message={error} />
        </Card>
      ) : notFound ? (
        <Card>
          <EmptyState title="No transaction found at this hash.">
            <ul>
              <li>It hasn't committed yet — wait a few seconds and reload.</li>
              <li>The hash is wrong (typo, missing/extra digits).</li>
              <li>
                It was submitted to a different network (chain_id mismatch —
                devnet is <code>2025</code>).
              </li>
            </ul>
          </EmptyState>
        </Card>
      ) : !tx ? (
        <Card>
          <Loading />
        </Card>
      ) : (
        <>
          <Card title="Summary">
            <dl className="detail-grid">
              <dt>Tx hash</dt>
              <dd>
                <Hash value={tx.tx_hash} label="tx hash" />
              </dd>
              <dt>Committed at round</dt>
              <dd>
                <a href={`#/block/${tx.round}`}>{tx.round.toLocaleString()}</a>
              </dd>
              <dt>Block cert hash</dt>
              <dd>
                <a href={`#/block/${tx.round}`} className="mono">
                  {tx.cert_hash}
                </a>
              </dd>
              <dt>Position in block</dt>
              <dd>{tx.index}</dd>
              <dt>Kind</dt>
              <dd>
                <code>{tx.intent.kind}</code>
              </dd>
            </dl>
          </Card>

          <Card title="Intent payload">
            <pre>{JSON.stringify(tx.intent, null, 2)}</pre>
          </Card>
        </>
      )}
    </div>
  );
}
