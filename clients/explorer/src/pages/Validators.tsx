// Validators — the dual-ring differentiator (route #/validators).
//
// Two rings: the 40-slot Authority Ring (ML-DSA-65 keyed, produces
// certificates) and the 200-slot Validator Ring. Each renders a
// count/capacity + total-stake header, an optional stake-distribution
// chart, and per-member rows with a proportional stake bar. Authority
// rows also show the member's post-quantum public key.

import { useEffect, useState } from "react";
import type {
  Client,
  AuthorityMemberView,
  ValidatorMemberView,
} from "@suwappu/client";
import { Card } from "../components/Card.js";
import { Badge } from "../components/Badge.js";
import { StakeBar } from "../components/StakeBar.js";
import { BarDistribution } from "../components/BarDistribution.js";
import type { Bar } from "../components/BarDistribution.js";
import { Hash } from "../components/Hash.js";
import { Loading, EmptyState, ErrorState } from "../components/states.js";
import { formatStake, stakeFraction, totalStake } from "../lib/format.js";

const AUTHORITY_CAP = 40;
const VALIDATOR_CAP = 200;

type Load<T> =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ok"; value: T };

export function Validators({ client }: { client: Client }) {
  const [authority, setAuthority] = useState<Load<AuthorityMemberView[]>>({
    status: "loading",
  });
  const [validator, setValidator] = useState<Load<ValidatorMemberView[]>>({
    status: "loading",
  });

  useEffect(() => {
    let cancelled = false;
    client
      .getAuthorityRegistry()
      .then((a) => {
        if (!cancelled) setAuthority({ status: "ok", value: a });
      })
      .catch((e) => {
        if (!cancelled)
          setAuthority({
            status: "error",
            message: e instanceof Error ? e.message : String(e),
          });
      });
    client
      .getValidatorRegistry()
      .then((v) => {
        if (!cancelled) setValidator({ status: "ok", value: v });
      })
      .catch((e) => {
        if (!cancelled)
          setValidator({
            status: "error",
            message: e instanceof Error ? e.message : String(e),
          });
      });
    return () => {
      cancelled = true;
    };
  }, [client]);

  return (
    <div className="page validators">
      <div className="page-intro">
        <h1>Validators</h1>
        <p className="lede">
          Two independently-bonded rings jointly co-sign every commit. A
          fork requires Byzantine corruption of both rings at once (paper
          Theorem 2).
        </p>
      </div>

      <AuthorityRing state={authority} />
      <ValidatorRing state={validator} />
    </div>
  );
}

function RingHeader({
  count,
  capacity,
  total,
}: {
  count: number;
  capacity: number;
  total: bigint;
}) {
  const atCap = count >= capacity;
  return (
    <div className="ring-meta">
      <span className="ring-count">
        {count} <span className="muted">/ {capacity} seated</span>
      </span>
      {atCap && (
        <Badge tone="good" title="Ring is at seated capacity">
          at capacity
        </Badge>
      )}
      <span className="ring-total">
        total stake <strong>{formatStake(total)}</strong> SUWAPPU
      </span>
    </div>
  );
}

function AuthorityRing({ state }: { state: Load<AuthorityMemberView[]> }) {
  if (state.status === "loading")
    return (
      <Card title="Authority Ring" subtitle="ML-DSA-65 certificate producers">
        <Loading />
      </Card>
    );
  if (state.status === "error")
    return (
      <Card title="Authority Ring" subtitle="ML-DSA-65 certificate producers">
        <ErrorState message={state.message} />
      </Card>
    );

  const members = state.value;
  const total = totalStake(members.map((m) => m.stake_suwappu));
  const max = members.reduce((mx, m) => Math.max(mx, m.stake_suwappu), 0);
  const bars: Bar[] = members.map((m) => ({
    id: m.id,
    fraction: max > 0 ? m.stake_suwappu / max : 0,
    label: `${formatStake(m.stake_suwappu)} SUWAPPU`,
  }));

  return (
    <Card
      title="Authority Ring"
      subtitle="Produces certificates; each member holds a FIPS-204 ML-DSA-65 key."
      actions={<Badge tone="pq">post-quantum keyed</Badge>}
    >
      <RingHeader count={members.length} capacity={AUTHORITY_CAP} total={total} />
      {members.length === 0 ? (
        <EmptyState title="No Authority members seated.">
          The registry returned an empty set for this epoch.
        </EmptyState>
      ) : (
        <>
          <div className="dist-block">
            <div className="dist-caption">Stake distribution by slot</div>
            <BarDistribution
              bars={bars}
              ariaLabel="Authority Ring stake distribution by slot. Per-member figures in the table below."
            />
          </div>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Slot</th>
                  <th className="num">Stake (SUWAPPU)</th>
                  <th className="bar-col">Share</th>
                  <th>ML-DSA-65 public key</th>
                </tr>
              </thead>
              <tbody>
                {members.map((m) => (
                  <tr key={m.id}>
                    <td>{m.id}</td>
                    <td className="num mono">{formatStake(m.stake_suwappu)}</td>
                    <td className="bar-col">
                      <StakeBar
                        fraction={max > 0 ? m.stake_suwappu / max : 0}
                        ariaLabel={`Slot ${m.id} stake, ${
                          max > 0 ? Math.round((m.stake_suwappu / max) * 100) : 0
                        }% of the ring maximum`}
                      />
                    </td>
                    <td>
                      <span className="pq-key">
                        <Hash
                          value={m.public_key_hex}
                          truncate
                          head={10}
                          tail={8}
                          label={`ML-DSA-65 key for slot ${m.id}`}
                        />
                        <span
                          className="pq-tag"
                          title="FIPS-204 (ML-DSA-65) post-quantum public key, 1952 B"
                          aria-label="FIPS-204 ML-DSA-65 post-quantum public key"
                        >
                          ML-DSA-65
                        </span>
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </Card>
  );
}

function ValidatorRing({ state }: { state: Load<ValidatorMemberView[]> }) {
  if (state.status === "loading")
    return (
      <Card title="Validator Ring" subtitle="Second independently-bonded ring">
        <Loading />
      </Card>
    );
  if (state.status === "error")
    return (
      <Card title="Validator Ring" subtitle="Second independently-bonded ring">
        <ErrorState message={state.message} />
      </Card>
    );

  const members = state.value;
  const total = totalStake(members.map((m) => m.stake_suwappu));
  const max = members.reduce((mx, m) => {
    let v: bigint;
    try {
      v = BigInt(m.stake_suwappu);
    } catch {
      v = 0n;
    }
    return v > mx ? v : mx;
  }, 0n);
  const bars: Bar[] = members.map((m) => ({
    id: m.id,
    fraction: stakeFraction(m.stake_suwappu, max),
    label: `${formatStake(m.stake_suwappu)} SUWAPPU`,
  }));

  return (
    <Card
      title="Validator Ring"
      subtitle="Co-signs every commit alongside the Authority Ring."
    >
      <RingHeader count={members.length} capacity={VALIDATOR_CAP} total={total} />
      {members.length === 0 ? (
        <EmptyState title="No Validator members seated.">
          The registry returned an empty set for this epoch.
        </EmptyState>
      ) : (
        <>
          <div className="dist-block">
            <div className="dist-caption">Stake distribution by slot</div>
            <BarDistribution
              bars={bars}
              ariaLabel="Validator Ring stake distribution by slot. Per-member figures in the table below."
            />
          </div>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Slot</th>
                  <th className="num">Stake (SUWAPPU)</th>
                  <th className="bar-col">Share</th>
                </tr>
              </thead>
              <tbody>
                {members.map((m) => {
                  const frac = stakeFraction(m.stake_suwappu, max);
                  return (
                    <tr key={m.id}>
                      <td>{m.id}</td>
                      <td className="num mono">{formatStake(m.stake_suwappu)}</td>
                      <td className="bar-col">
                        <StakeBar
                          fraction={frac}
                          ariaLabel={`Slot ${m.id} stake, ${Math.round(
                            frac * 100,
                          )}% of the ring maximum`}
                        />
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </>
      )}
    </Card>
  );
}
