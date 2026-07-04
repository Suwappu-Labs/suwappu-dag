// Address page — substrate balance for a 20-byte address.
//
// The substrate doesn't distinguish "absent" from "explicit zero", so a
// zero balance is rendered honestly as such rather than as "not found".

import { useEffect, useState } from "react";
import type { BalanceView, Client } from "@suwappu/client";

const POLL_MS = 5000;

export function AddressPage({
  client,
  address,
}: {
  client: Client;
  address: string;
}) {
  const [view, setView] = useState<BalanceView | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function refresh() {
      try {
        const v = await client.getBalance(address);
        if (cancelled) return;
        setView(v);
        setError(null);
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
        }
      }
    }

    setView(null);
    refresh();
    const id = setInterval(refresh, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [client, address]);

  if (error && !view) {
    return <div className="error">RPC error: {error}</div>;
  }
  if (!view) {
    return <div className="loading">Loading…</div>;
  }

  return (
    <div className="address">
      <h1>Address</h1>
      <dl>
        <dt>Address</dt>
        <dd className="hash">{view.address}</dd>
        <dt>Balance (smallest unit)</dt>
        <dd>{formatUnits(view.balance)}</dd>
      </dl>
      {view.balance === "0" && (
        <p className="muted">
          Zero balance — the substrate does not distinguish an unused address
          from one holding exactly zero.
        </p>
      )}
      {error && <div className="error">Last poll failed: {error}</div>}
    </div>
  );
}

function formatUnits(decimal: string): string {
  return decimal.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}
