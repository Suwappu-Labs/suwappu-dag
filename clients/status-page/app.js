// gsx-devnet status page — vanilla JS, no framework.
//
// Polls every 5 seconds:
//   - https://rpc.devnet.gsx.globalsettlement.com → gsx_getEpoch
//     (chain head + last-commit-timestamp via the explorer's lens).
//   - https://faucet.devnet.gsx.globalsettlement.com/health → 200 = up.
//
// We don't have per-region tip-round from a single RPC call (the ALB
// load-balances across the 4 validators). For per-region visibility
// the status page falls back to the AWS CloudWatch GetMetricData
// public proxy at status-api.devnet.gsx.globalsettlement.com (G6's
// metrics endpoint; not wired in this v0.1). For now we show:
//   - One "Cluster tip" tile (cluster-wide, scraped via the ALB).
//   - One tile per faucet endpoint (alive/down).
//
// Overall state machine:
//   green   — tip advancing AND faucet up
//   yellow  — tip advancing AND faucet down (devs can read but not drip)
//             OR faucet up AND tip flat <1 min (warming up)
//   red     — tip flat >1 min OR /epoch unreachable

"use strict";

const RPC_URL    = "https://rpc.devnet.gsx.globalsettlement.com";
const FAUCET_URL = "https://faucet.devnet.gsx.globalsettlement.com";
const POLL_MS    = 5000;
const TIP_FLAT_THRESHOLD_MS = 60_000;

const state = {
  tipRound: null,
  tipChangedAtMs: null,
  tipFlatSinceMs: null,
  epoch: null,
  faucetAlive: null,
  lastRpcError: null,
  lastFaucetError: null,
};

async function rpcCall(method) {
  const resp = await fetch(RPC_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method }),
  });
  if (!resp.ok) throw new Error(`RPC ${method} HTTP ${resp.status}`);
  const json = await resp.json();
  if (json.error) throw new Error(`RPC ${method} error: ${JSON.stringify(json.error)}`);
  return json.result;
}

async function refresh() {
  const now = Date.now();

  // RPC poll. gsx_getEpoch is cheap; ALB load-balances across the
  // 4 validators so we see whichever responds first — for overall
  // health, that's what matters.
  try {
    const epoch = await rpcCall("gsx_getEpoch");
    state.epoch = epoch;
    state.lastRpcError = null;
    const tip = epoch.latest_committed_round ?? 0;
    if (state.tipRound === null || tip > state.tipRound) {
      state.tipRound = tip;
      state.tipChangedAtMs = now;
      state.tipFlatSinceMs = null;
    } else {
      // tip flat — start the flatness clock if it isn't already
      // running.
      if (state.tipFlatSinceMs === null) state.tipFlatSinceMs = now;
    }
  } catch (e) {
    state.lastRpcError = e.message;
  }

  // Faucet liveness. /health returns 200 with a JSON body when the
  // faucet's own wallet has at least one drip's worth of balance.
  try {
    const resp = await fetch(`${FAUCET_URL}/health`, { method: "GET" });
    state.faucetAlive = resp.ok;
    state.lastFaucetError = null;
  } catch (e) {
    state.faucetAlive = false;
    state.lastFaucetError = e.message;
  }

  render();
}

function render() {
  // ---- chain head dl ----
  if (state.epoch) {
    $("tip-round").textContent = (state.epoch.latest_committed_round ?? 0).toLocaleString();
    $("tip-epoch").textContent = state.epoch.current.toLocaleString();
    $("tip-bound").textContent = state.epoch.last_boundary_round.toLocaleString();
    $("tip-rpe").textContent = state.epoch.rounds_per_epoch.toLocaleString();
  } else if (state.lastRpcError) {
    $("tip-round").textContent = "RPC error";
  }

  // ---- per-tile rendering ----
  // Cluster tip tile + per-region tiles (per-region needs CloudWatch
  // proxy — see G6; not wired in this v0.1, so render a single
  // "cluster" tile for now).
  const validators = $("validators");
  validators.innerHTML = "";
  const tipFlatMs = state.tipFlatSinceMs !== null
    ? Date.now() - state.tipFlatSinceMs
    : 0;
  let tipStatus, tipLabel;
  if (state.lastRpcError) {
    tipStatus = "red";
    tipLabel = "Unreachable";
  } else if (state.tipFlatSinceMs !== null && tipFlatMs > TIP_FLAT_THRESHOLD_MS) {
    tipStatus = "red";
    tipLabel = `Halted (${Math.round(tipFlatMs / 1000)}s flat)`;
  } else if (state.tipFlatSinceMs !== null) {
    tipStatus = "yellow";
    tipLabel = `Tip flat ${Math.round(tipFlatMs / 1000)}s`;
  } else {
    tipStatus = "green";
    tipLabel = "Advancing";
  }
  validators.appendChild(tile({
    region: "cluster (ALB)",
    value: state.tipRound !== null ? state.tipRound.toLocaleString() : "—",
    status: tipLabel,
    cls: tipStatus,
  }));

  // ---- faucet tile ----
  const faucet = $("faucet");
  faucet.innerHTML = "";
  let faucetStatus, faucetLabel;
  if (state.faucetAlive === null) {
    faucetStatus = "unknown";
    faucetLabel = "Checking…";
  } else if (state.faucetAlive) {
    faucetStatus = "green";
    faucetLabel = "Up";
  } else {
    faucetStatus = "red";
    faucetLabel = "Down";
  }
  faucet.appendChild(tile({
    region: "faucet (us-east-1)",
    value: faucetStatus === "green" ? "200" : "—",
    status: faucetLabel,
    cls: faucetStatus,
  }));

  // ---- overall ----
  let overallCls, overallLabel, overallDetail;
  if (tipStatus === "red") {
    overallCls = "red";
    overallLabel = "Major incident";
    overallDetail = state.lastRpcError ?? "Chain has stopped progressing.";
  } else if (tipStatus === "yellow" || faucetStatus === "red") {
    overallCls = "yellow";
    overallLabel = "Degraded";
    overallDetail = faucetStatus === "red"
      ? "Chain advancing; faucet down — devs can read state but can't acquire fresh tokens."
      : "Chain progress is slow but recovering.";
  } else if (faucetStatus === "unknown") {
    overallCls = "yellow";
    overallLabel = "Loading…";
    overallDetail = "";
  } else {
    overallCls = "green";
    overallLabel = "All systems operational";
    overallDetail = `RPC + faucet up. Last commit at round ${state.tipRound}.`;
  }
  const overall = $("overall");
  overall.className = `overall ${overallCls}`;
  $("overall-label").textContent = overallLabel;
  $("overall-detail").textContent = overallDetail;

  $("updated").textContent = new Date().toLocaleTimeString();
}

function tile({ region, value, status, cls }) {
  const div = document.createElement("div");
  div.className = `tile ${cls}`;
  div.innerHTML = `
    <div class="region">${escapeHtml(region)}</div>
    <div class="value">${escapeHtml(value)}</div>
    <div class="status">${escapeHtml(status)}</div>
  `;
  return div;
}

function $(id) { return document.getElementById(id); }

function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

refresh();
setInterval(refresh, POLL_MS);
