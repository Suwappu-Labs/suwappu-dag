// Transaction detail — looks up via gsx_getTransaction.

import { useEffect, useState } from "react";
import type { Client, TransactionView } from "@gsx/client";

export function TxPage({ client, hash }: { client: Client; hash: string }) {
  const [tx, setTx] = useState<TransactionView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notFound, setNotFound] = useState(false);

  useEffect(() => {
    let cancelled = false;
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
        if (t === null) {
          setNotFound(true);
        } else {
          setTx(t);
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
  }, [client, hash]);

  if (error) {
    return (
      <div className="tx-page">
        <h1>Transaction</h1>
        <div className="hash">{hash}</div>
        <div className="error">RPC error: {error}</div>
      </div>
    );
  }
  if (notFound) {
    return (
      <div className="tx-page">
        <h1>Transaction</h1>
        <div className="hash">{hash}</div>
        <div className="not-found">
          No transaction found at this hash. Possible reasons:
          <ul>
            <li>The transaction hasn't committed yet — wait a few seconds and reload.</li>
            <li>The hash is wrong (typo, missing/extra digits).</li>
            <li>
              The transaction was submitted to a different network
              (chain_id mismatch — devnet is <code>2025</code>).
            </li>
          </ul>
        </div>
      </div>
    );
  }
  if (!tx) {
    return (
      <div className="tx-page">
        <h1>Transaction</h1>
        <div className="hash">{hash}</div>
        <div className="loading">Loading…</div>
      </div>
    );
  }

  return (
    <div className="tx-page">
      <h1>Transaction</h1>
      <dl>
        <dt>Tx hash</dt>
        <dd className="hash">{tx.tx_hash}</dd>
        <dt>Committed at round</dt>
        <dd>
          <a href={`#/block/${tx.round}`}>{tx.round}</a>
        </dd>
        <dt>Block cert hash</dt>
        <dd className="hash">
          <a href={`#/block/${tx.round}`}>{tx.cert_hash}</a>
        </dd>
        <dt>Position in block</dt>
        <dd>{tx.index}</dd>
        <dt>Kind</dt>
        <dd>
          <code>{tx.intent.kind}</code>
        </dd>
      </dl>

      <h2>Intent payload</h2>
      <pre>{JSON.stringify(tx.intent, null, 2)}</pre>
    </div>
  );
}
