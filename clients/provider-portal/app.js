// Suwappu Compute provider portal — calculator + live tiles.
// No framework, no build step (same conventions as clients/status-page).
//
// Sources of truth the numbers mirror:
//   - Testnet points: docs/testnet/POINTS.md (public contract).
//   - Mainnet stablecoin: suwappu-lattice-protocol ltp/incentives.py
//     defaults (storage/serving rates) and suwappu-precompiles::rewards
//     RewardParams (governance-set; placeholder values here, labeled
//     illustrative in the UI).

"use strict";

// --- Constants -----------------------------------------------------------

// Testnet cadence: 4096 rounds x 250 ms ~= 17 min per epoch.
const EPOCH_MINUTES = (4096 * 250) / 1000 / 60;
const EPOCHS_PER_WEEK = (7 * 24 * 60) / EPOCH_MINUTES;
const EPOCHS_PER_MONTH = (30 * 24 * 60) / EPOCH_MINUTES;

// Testnet points formula (docs/testnet/POINTS.md).
const CERT_POINTS_CAP = 50; // per epoch
const COMMIT_POINTS_CAP = 30; // per epoch

// Mainnet illustrative rates, stablecoin micro-units (6 decimals).
// Storage/serving mirror ltp.incentives.IncentiveConfig defaults;
// per-certificate / per-attestation are placeholders for the
// governance-set RewardParams and are labeled illustrative in the UI.
const MICRO = 1e6;
const PRICE_PER_GIB_MONTH_MICRO = 20_000; // $0.02 / GiB-month stored
const PRICE_PER_GIB_SERVED_MICRO = 10_000; // $0.01 / GiB served
const PER_CERTIFICATE_MICRO = 100; // $0.0001 (illustrative)
const PER_ATTESTATION_MICRO = 1_000; // $0.001 (illustrative)

// --- Mode toggle ---------------------------------------------------------

let mode = "testnet";

function setMode(next) {
  mode = next;
  document.getElementById("mode-testnet").classList.toggle("active", next === "testnet");
  document.getElementById("mode-mainnet").classList.toggle("active", next === "mainnet");
  document.getElementById("panel-testnet").style.display = next === "testnet" ? "" : "none";
  document.getElementById("panel-mainnet").style.display = next === "mainnet" ? "" : "none";
  recalc();
}

// --- Calculator ----------------------------------------------------------

function num(id) {
  const v = parseFloat(document.getElementById(id).value);
  return Number.isFinite(v) && v >= 0 ? v : 0;
}

function fmt(n, digits) {
  return n.toLocaleString("en-US", { maximumFractionDigits: digits });
}

function recalcTestnet() {
  const uptimePoints = num("t-uptime"); // 100 / 50 / 0 per the tier
  const certPoints = Math.min(num("t-certs") / 1000, CERT_POINTS_CAP);
  const commitPoints = Math.min(num("t-commits"), COMMIT_POINTS_CAP);
  // Below 95% uptime the epoch earns nothing at all.
  const perEpoch = uptimePoints === 0 ? 0 : uptimePoints + certPoints + commitPoints;
  const weekly = perEpoch * EPOCHS_PER_WEEK;
  document.getElementById("t-result").textContent = fmt(weekly, 0);
  document.getElementById("t-breakdown").textContent =
    uptimePoints === 0
      ? "Below 95% uptime an epoch earns nothing — liveness is the gate."
      : `${fmt(perEpoch, 1)} points/epoch (uptime ${uptimePoints} + certs ` +
        `${fmt(certPoints, 1)} + commits ${fmt(commitPoints, 0)}) x ` +
        `${fmt(EPOCHS_PER_WEEK, 0)} epochs/week`;
}

function recalcMainnet() {
  const role = document.getElementById("m-role").value;
  const uptime = parseFloat(document.getElementById("m-uptime").value);
  document.getElementById("m-ltp-fields").style.display = role === "ltp" ? "" : "none";
  document.getElementById("m-dag-fields").style.display = role === "dag" ? "" : "none";

  let monthlyMicro = 0;
  let breakdown = "";
  if (role === "ltp") {
    const storage = num("m-storage") * PRICE_PER_GIB_MONTH_MICRO;
    const served = num("m-served") * PRICE_PER_GIB_SERVED_MICRO;
    monthlyMicro = (storage + served) * uptime;
    breakdown =
      `storage $${fmt(storage / MICRO, 2)} + serving $${fmt(served / MICRO, 2)}, ` +
      `x${uptime} uptime gate — assumes full audit pass (pay scales with the PDP pass ratio)`;
  } else {
    const perEpoch =
      num("m-certs") * PER_CERTIFICATE_MICRO + num("m-attests") * PER_ATTESTATION_MICRO;
    monthlyMicro = perEpoch * EPOCHS_PER_MONTH * uptime;
    breakdown =
      `$${fmt(perEpoch / MICRO, 4)}/epoch x ${fmt(EPOCHS_PER_MONTH, 0)} epochs/month, ` +
      `x${uptime} uptime gate — before the per-epoch budget clamp`;
  }
  if (uptime === 0) breakdown = "Below 95% uptime an epoch is unpaid — liveness is the gate.";
  document.getElementById("m-result").textContent = "$" + fmt(monthlyMicro / MICRO, 2);
  document.getElementById("m-breakdown").textContent = breakdown;
}

function recalc() {
  if (mode === "testnet") recalcTestnet();
  else recalcMainnet();
}

// --- Live tiles ----------------------------------------------------------

const RPC_URL = "https://rpc.devnet.suwappu.bot/";

async function pollEpoch() {
  const tip = document.getElementById("tile-tip");
  const epoch = document.getElementById("tile-epoch");
  const rpe = document.getElementById("tile-rpe");
  const rpc = document.getElementById("tile-rpc");
  try {
    const res = await fetch(RPC_URL, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0", id: 1, method: "suwappu_getEpoch", params: [],
      }),
      signal: AbortSignal.timeout(5000),
    });
    if (!res.ok) throw new Error("http " + res.status);
    const body = await res.json();
    const r = body.result || {};
    tip.textContent = r.latest_committed_round != null ? fmt(r.latest_committed_round, 0) : "—";
    epoch.textContent = r.current_epoch != null ? fmt(r.current_epoch, 0) : "—";
    rpe.textContent = r.rounds_per_epoch != null ? fmt(r.rounds_per_epoch, 0) + " rounds" : "—";
    rpc.textContent = "reachable";
    rpc.style.color = "var(--green)";
  } catch (_err) {
    tip.textContent = "—";
    epoch.textContent = "—";
    rpe.textContent = "—";
    rpc.textContent = "unreachable from here";
    rpc.style.color = "var(--fg-muted)";
  }
}

// --- Live points lookup --------------------------------------------------

const LEADERBOARD_URL = "https://leaderboard.devnet.suwappu.bot/leaderboard";
// Published TGE conversion band: testnet allocation is 5-8% of mainnet
// supply, set by the foundation board >= 90 days pre-TGE (POINTS.md).
const TGE_ALLOCATION_LOW = 0.05;
const TGE_ALLOCATION_HIGH = 0.08;

async function lookupPoints() {
  const id = num("lb-id");
  const result = document.getElementById("lb-result");
  const note = document.getElementById("lb-note");
  const total = document.getElementById("lb-total");
  const breakdown = document.getElementById("lb-breakdown");
  const share = document.getElementById("lb-share");
  result.style.display = "";
  total.textContent = "…";
  breakdown.textContent = "";
  share.textContent = "";
  try {
    const res = await fetch(LEADERBOARD_URL, { signal: AbortSignal.timeout(8000) });
    if (!res.ok) throw new Error("http " + res.status);
    const snap = await res.json();
    const entries = snap.entries || [];
    const mine = entries.find((e) => e.authority_id === id);
    if (!mine) {
      total.textContent = "not found";
      breakdown.textContent =
        `Authority id ${id} is not on the leaderboard yet — points appear ` +
        "after your first scored epoch (registration + one probe cycle).";
      return;
    }
    total.textContent = fmt(mine.total_points, 0);
    const parts = [
      ["uptime", mine.uptime_points],
      ["certs", mine.cert_points],
      ["bug bounty", mine.bug_bounty_points],
      ["hackathon", mine.hackathon_points],
    ].filter(([, v]) => typeof v === "number" && v > 0);
    breakdown.textContent =
      (mine.label ? mine.label + " — " : "") +
      (parts.length
        ? parts.map(([k, v]) => `${k} ${fmt(v, 0)}`).join(" + ")
        : "no per-category points recorded yet") +
      (mine.is_seed ? " (foundation seed — not TGE-eligible)" : "");
    // Share of the eligible pool -> TGE allocation band.
    const eligibleTotal = entries
      .filter((e) => !e.is_seed)
      .reduce((sum, e) => sum + (e.total_points || 0), 0);
    if (!mine.is_seed && eligibleTotal > 0 && mine.total_points > 0) {
      const frac = mine.total_points / eligibleTotal;
      // POINTS.md hard ceiling: no operator takes more than 2% of the
      // testnet allocation, whatever their share of points.
      const capped = Math.min(frac, 0.02);
      share.textContent =
        `Current share of eligible points: ${fmt(frac * 100, 2)}%` +
        (capped < frac ? " (capped at 2% of the allocation)" : "") +
        ` — at TGE that converts to ${fmt(capped * TGE_ALLOCATION_LOW * 100, 3)}%–` +
        `${fmt(capped * TGE_ALLOCATION_HIGH * 100, 3)}% of mainnet supply ` +
        "(if the distribution held, which it won't — more operators join).";
    }
  } catch (_err) {
    total.textContent = "—";
    breakdown.textContent = "";
    note.innerHTML =
      "The leaderboard API is not reachable from your browser right now. " +
      'The public <a href="https://testnet.suwappu.bot/leaderboard">leaderboard page</a> ' +
      "has the same numbers.";
  }
}

// --- Boot ----------------------------------------------------------------

recalc();
pollEpoch();
setInterval(pollEpoch, 15_000);
