// Attestation — the post-quantum bridge-header attestation (route
// #/attestation).
//
// Polls `suwappu_getHeaderAttestation` (no typed SDK method — driven
// through the generic `client.call` escape hatch in lib/rpc.ts). The
// devnet endpoint may not have a bridge signer configured, so an
// absent/null result renders a clean empty state rather than crashing.

import { useEffect, useState } from "react";
import type { Client } from "@suwappu/client";
import { Card } from "../components/Card.js";
import { Badge } from "../components/Badge.js";
import { Hash } from "../components/Hash.js";
import { Loading, EmptyState, ErrorState } from "../components/states.js";
import { fetchHeaderAttestation } from "../lib/rpc.js";
import type { AttestationResult } from "../lib/rpc.js";

const POLL_MS = 3000;

export function Attestation({ client }: { client: Client }) {
  const [result, setResult] = useState<AttestationResult | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function refresh() {
      const r = await fetchHeaderAttestation(client);
      if (!cancelled) setResult(r);
    }
    refresh();
    const id = window.setInterval(refresh, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [client]);

  return (
    <div className="page attestation">
      <div className="page-intro">
        <h1>Bridge attestation</h1>
        <p className="lede">
          The finalized-round anchor a validator serves to cross-chain
          relayers. It is signed with ML-DSA-65 (FIPS 204) — the only bridge
          path that is both trust-minimized and post-quantum.
        </p>
      </div>

      {result == null && (
        <Card title="Latest header attestation">
          <Loading />
        </Card>
      )}

      {result?.status === "error" && (
        <Card title="Latest header attestation">
          <ErrorState message={result.message} />
        </Card>
      )}

      {result?.status === "unavailable" && (
        <Card title="Latest header attestation">
          <EmptyState title="Header attestation not enabled on this endpoint yet.">
            {result.reason === "not-enabled" ? (
              <>
                This RPC endpoint does not expose{" "}
                <code>suwappu_getHeaderAttestation</code>. Bridge signing is an
                opt-in validator feature.
              </>
            ) : (
              <>
                The validator has no bridge signer configured, or no block has
                finalized yet. Attestations appear here once a signer is set
                and a round has committed.
              </>
            )}
          </EmptyState>
        </Card>
      )}

      {result?.status === "ok" && (
        <>
          <Card
            title="Latest header attestation"
            subtitle="A validator's signed claim over its latest finalized block."
            actions={<Badge tone="pq">ML-DSA-65 · post-quantum</Badge>}
          >
            <dl className="detail-grid">
              <dt>Block number (round)</dt>
              <dd>
                <a href={`#/block/${result.attestation.block_number}`}>
                  {result.attestation.block_number.toLocaleString()}
                </a>
              </dd>

              <dt>State root</dt>
              <dd>
                <Hash
                  value={result.attestation.state_root}
                  truncate
                  head={12}
                  tail={10}
                  label="state root"
                />{" "}
                <span className="muted">BLAKE3 L1 root</span>
              </dd>

              <dt>Attesting authority</dt>
              <dd>slot {result.attestation.authority_id}</dd>

              <dt>Signer public key</dt>
              <dd>
                <Hash
                  value={result.attestation.pubkey}
                  truncate
                  head={12}
                  tail={10}
                  label="signer public key"
                />{" "}
                <span className="pq-tag">ML-DSA-65</span>
              </dd>

              <dt>Signature</dt>
              <dd>
                <Hash
                  value={result.attestation.signature}
                  truncate
                  head={12}
                  tail={10}
                  label="ML-DSA-65 signature"
                />
              </dd>

              <dt>Network id</dt>
              <dd>
                <Hash
                  value={result.attestation.network_id}
                  truncate
                  head={12}
                  tail={10}
                  label="network id"
                />
              </dd>

              <dt>Oracle</dt>
              <dd>
                <Hash
                  value={result.attestation.oracle}
                  truncate
                  head={12}
                  tail={8}
                  label="oracle address"
                />
              </dd>
            </dl>
          </Card>

          <Card title="What this is">
            <p className="explainer">
              When a validator commits a round, it captures the committed{" "}
              <code>(round, state root)</code> and produces a detached
              ML-DSA-65 signature over a BLAKE3 digest bound to this network id
              and oracle address. A relayer polls every validator, collects a
              set whose stake clears the on-chain &gt;2/3 threshold, and submits
              the aggregate to the destination header oracle.
            </p>
            <p className="explainer muted">
              Honest framing: this is a validator <em>attestation</em>, not a
              storage proof. Safety rests on an honest &gt;2/3-stake quorum —
              sync-committee-class, not a light client. It is, however, the only
              bridge configuration that is post-quantum end to end.
            </p>
          </Card>
        </>
      )}
    </div>
  );
}
