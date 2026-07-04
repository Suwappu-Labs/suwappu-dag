// Validators page — the authority ring and validator ring, live.
//
// Polls `suwappu_getEpoch` + both registry endpoints every 5 seconds.
// Authority stakes are u64 (safe as JS numbers); validator stakes are
// u128 decimal strings — summed with BigInt, never Number.

import { useEffect, useState } from "react";
import type {
  AuthorityMemberView,
  Client,
  EpochView,
  ValidatorMemberView,
} from "@suwappu/client";

const POLL_MS = 5000;

export function ValidatorsPage({ client }: { client: Client }) {
  const [epoch, setEpoch] = useState<EpochView | null>(null);
  const [authorities, setAuthorities] = useState<AuthorityMemberView[] | null>(
    null,
  );
  const [validators, setValidators] = useState<ValidatorMemberView[] | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function refresh() {
      try {
        const [ep, auths, vals] = await Promise.all([
          client.getEpoch(),
          client.getAuthorityRegistry(),
          client.getValidatorRegistry(),
        ]);
        if (cancelled) return;
        setEpoch(ep);
        setAuthorities(auths);
        setValidators(vals);
        setError(null);
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

  if (error && !authorities) {
    return <div className="error">RPC error: {error}</div>;
  }
  if (!authorities || !validators) {
    return <div className="loading">Loading…</div>;
  }

  const authorityStakeTotal = authorities.reduce(
    (acc, a) => acc + BigInt(a.stake_suwappu),
    0n,
  );
  const validatorStakeTotal = validators.reduce(
    (acc, v) => acc + BigInt(v.stake_suwappu),
    0n,
  );

  return (
    <div className="validators">
      <section>
        <h1>Rings</h1>
        <dl>
          <dt>Current epoch</dt>
          <dd>{epoch ? epoch.current.toLocaleString() : "…"}</dd>
          <dt>Authority seats</dt>
          <dd>{authorities.length.toLocaleString()}</dd>
          <dt>Authority stake (total)</dt>
          <dd>{formatSuwappu(authorityStakeTotal)}</dd>
          <dt>Validator seats</dt>
          <dd>{validators.length.toLocaleString()}</dd>
          <dt>Validator stake (total)</dt>
          <dd>{formatSuwappu(validatorStakeTotal)}</dd>
        </dl>
        {error && <div className="error">Last poll failed: {error}</div>}
      </section>

      <section>
        <h2>Authority ring</h2>
        <p className="muted">
          Authorities propose and certify blocks. Membership changes apply at
          epoch boundaries via governance intents.
        </p>
        {authorities.length === 0 ? (
          <div className="empty">No seated authorities.</div>
        ) : (
          <table>
            <thead>
              <tr>
                <th>Seat</th>
                <th>Stake (SUWAPPU)</th>
                <th>Stake share</th>
                <th>ML-DSA-65 public key</th>
              </tr>
            </thead>
            <tbody>
              {authorities.map((a) => (
                <tr key={a.id}>
                  <td>{a.id}</td>
                  <td>{formatSuwappu(BigInt(a.stake_suwappu))}</td>
                  <td>
                    {stakeShare(BigInt(a.stake_suwappu), authorityStakeTotal)}
                  </td>
                  <td className="hash" title={a.public_key_hex}>
                    {shortHex(a.public_key_hex)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section>
        <h2>Validator ring</h2>
        <p className="muted">
          Validators attest to certified blocks; stake is u128 and rendered
          exactly.
        </p>
        {validators.length === 0 ? (
          <div className="empty">No seated validators.</div>
        ) : (
          <table>
            <thead>
              <tr>
                <th>Seat</th>
                <th>Stake (SUWAPPU)</th>
                <th>Stake share</th>
              </tr>
            </thead>
            <tbody>
              {validators.map((v) => (
                <tr key={v.id}>
                  <td>{v.id}</td>
                  <td>{formatSuwappu(BigInt(v.stake_suwappu))}</td>
                  <td>
                    {stakeShare(BigInt(v.stake_suwappu), validatorStakeTotal)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </div>
  );
}

/** Group digits for readability without float precision loss. */
function formatSuwappu(v: bigint): string {
  const s = v.toString();
  return s.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** Percentage of `total` with one decimal, exact via basis points. */
function stakeShare(v: bigint, total: bigint): string {
  if (total === 0n) return "—";
  const bps = (v * 10000n) / total;
  return `${(Number(bps) / 100).toFixed(1)}%`;
}

function shortHex(hex: string): string {
  if (hex.length <= 20) return hex;
  return `${hex.slice(0, 12)}…${hex.slice(-8)}`;
}
