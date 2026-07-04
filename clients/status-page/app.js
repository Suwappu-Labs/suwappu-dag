// suwappu-devnet status page — live health + performance dashboard.
// Vanilla JS, no framework, no build step. Served as static files.
//
// Honesty contract (read before editing):
//   - We only show "healthy/good" for a surface whose response we could
//     actually READ and validate (the RPC JSON-RPC call). Everything we
//     can only reach across an opaque/no-cors boundary is labelled
//     "reachable", never green-as-healthy.
//   - Performance numbers are either LIVE-DERIVED from successive
//     getEpoch polls (round-advance interval, observed on this endpoint)
//     or shown as "pending — see perf run" placeholders. We never invent
//     a TPS number.

"use strict";

// ---------------------------------------------------------------------------
// Config — operators can edit this block.
// ---------------------------------------------------------------------------
const ENDPOINTS = {
  rpc:      "https://rpc.devnet.suwappu.bot",
  ws:       "wss://ws.devnet.suwappu.bot/ws",
  faucet:   "https://faucet.devnet.suwappu.bot",
  explorer: "https://explorer.devnet.suwappu.bot",
};
const DOCS = {
  perfRun:  "https://github.com/Suwappu-Labs/suwappu-dag/blob/main/DEVNET.md",
};

const POLL_MS               = 5000;   // chain-head + component poll cadence
const WS_TIMEOUT_MS         = 4000;   // WebSocket open attempt timeout
const HTTP_TIMEOUT_MS       = 6000;   // fetch timeout for reachability probes
const STALL_THRESHOLD_MS    = 30_000; // committed round flat this long => warn
const HALT_THRESHOLD_MS     = 90_000; // flat this long => critical
const MAX_SAMPLES           = 40;     // sparkline ring buffer size

// ---------------------------------------------------------------------------
// Status levels. Ordered by severity so we can take a max.
// ---------------------------------------------------------------------------
const LEVEL = { unknown: 0, good: 1, warning: 2, serious: 3, critical: 4 };
function worst(a, b) { return LEVEL[a] >= LEVEL[b] ? a : b; }

// ---------------------------------------------------------------------------
// Mutable state.
// ---------------------------------------------------------------------------
const state = {
  components: {
    rpc:      { name: "JSON-RPC",   host: ENDPOINTS.rpc,      level: "unknown", label: "Checking…", latencyMs: null },
    ws:       { name: "WebSocket",  host: ENDPOINTS.ws,       level: "unknown", label: "Checking…", latencyMs: null },
    faucet:   { name: "Faucet",     host: ENDPOINTS.faucet,   level: "unknown", label: "Checking…", latencyMs: null },
    explorer: { name: "Explorer",   host: ENDPOINTS.explorer, level: "unknown", label: "Checking…", latencyMs: null },
  },
  epoch: null,            // last EpochView
  rpcError: null,
  committedRound: null,   // last observed latest_committed_round
  roundChangedAt: null,   // ms when committedRound last increased
  lastAdvanceIntervalMs: null, // observed gap between two advances
  samples: [],            // [{ t, round }] ring buffer for sparkline
  // Committed perf.json (published figures). Loaded once at boot; on any
  // failure we fall back to the static "pending — see perf run" tiles so
  // the page never breaks.
  perf: { doc: null, loaded: false, error: false },
};

// ---------------------------------------------------------------------------
// Published perf figures. Fetched once from the committed perf.json served
// alongside this page. We render exactly what it says — a null value shows
// as the muted "pending" treatment with the metric's note; we never invent
// a number.
// ---------------------------------------------------------------------------
async function loadPerf() {
  try {
    const resp = await fetch("./perf.json", { cache: "no-store" });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const doc = await resp.json();
    if (doc && Array.isArray(doc.metrics)) {
      state.perf.doc = doc;
      state.perf.loaded = true;
    } else {
      state.perf.error = true;
    }
  } catch (_e) {
    state.perf.error = true;
  }
  render();
}

function perfBadgeCls(status) {
  if (status === "observed") return "observed";
  if (status === "published") return "published";
  return ""; // pending / unknown → base muted badge
}

// ---------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------
function $(id) { return document.getElementById(id); }
function el(tag, cls) { const e = document.createElement(tag); if (cls) e.className = cls; return e; }
function fmt(n) { return typeof n === "number" ? n.toLocaleString() : "—"; }

function withTimeout(promise, ms, onTimeout) {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => onTimeout ? resolve(onTimeout()) : reject(new Error("timeout")), ms);
    promise.then(v => { clearTimeout(t); resolve(v); },
                 e => { clearTimeout(t); reject(e); });
  });
}

// ---------------------------------------------------------------------------
// Health checks. Each returns { level, label, latencyMs } and is honest
// about what it could actually verify.
// ---------------------------------------------------------------------------

// RPC: a real JSON-RPC call whose result we parse and validate. This is a
// genuine health check — a good/green here means we read a well-formed
// EpochView back.
async function checkRpc() {
  const t0 = performance.now();
  try {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), HTTP_TIMEOUT_MS);
    const resp = await fetch(ENDPOINTS.rpc, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "suwappu_getEpoch", params: null }),
      signal: ctrl.signal,
    });
    clearTimeout(timer);
    const latencyMs = Math.round(performance.now() - t0);
    if (!resp.ok) {
      return { level: "critical", label: `HTTP ${resp.status}`, latencyMs, epoch: null };
    }
    const json = await resp.json();
    if (json.error) {
      return { level: "serious", label: "RPC error", latencyMs, epoch: null };
    }
    const epoch = json.result;
    if (!epoch || typeof epoch.current !== "number") {
      return { level: "serious", label: "Bad payload", latencyMs, epoch: null };
    }
    return { level: "good", label: "Healthy", latencyMs, epoch };
  } catch (e) {
    const latencyMs = Math.round(performance.now() - t0);
    // Could be down, DNS, TLS, or a CORS block on a live host. We cannot
    // read the response, so we cannot claim healthy — report unreachable.
    return { level: "critical", label: "Unreachable", latencyMs, epoch: null };
  }
}

// WebSocket: open a short-lived socket. onopen => the endpoint accepted the
// upgrade (reachable/connected). We do NOT subscribe, so we verify the
// handshake, not stream health.
function checkWs() {
  return new Promise((resolve) => {
    const t0 = performance.now();
    let settled = false;
    let sock;
    const done = (r) => {
      if (settled) return;
      settled = true;
      try { if (sock) sock.close(); } catch (_) {}
      resolve(r);
    };
    const timer = setTimeout(
      () => done({ level: "serious", label: "No response", latencyMs: WS_TIMEOUT_MS }),
      WS_TIMEOUT_MS);
    try {
      sock = new WebSocket(ENDPOINTS.ws);
      sock.onopen = () => {
        clearTimeout(timer);
        done({ level: "good", label: "Connected", latencyMs: Math.round(performance.now() - t0) });
      };
      sock.onerror = () => {
        clearTimeout(timer);
        done({ level: "critical", label: "Unreachable", latencyMs: Math.round(performance.now() - t0) });
      };
    } catch (e) {
      clearTimeout(timer);
      done({ level: "critical", label: "Unreachable", latencyMs: Math.round(performance.now() - t0) });
    }
  });
}

// Opaque reachability probe (faucet, explorer). A no-cors GET yields an
// opaque response we cannot inspect — so a resolved fetch means the host
// answered (DNS+TLS+HTTP), i.e. REACHABLE, not verified-healthy. A thrown
// fetch means we could not reach it. We are explicit about that limit.
async function checkReachable(url) {
  const t0 = performance.now();
  try {
    await withTimeout(
      fetch(url, { method: "GET", mode: "no-cors", cache: "no-store" }),
      HTTP_TIMEOUT_MS);
    return { level: "warning", label: "Reachable", latencyMs: Math.round(performance.now() - t0) };
  } catch (e) {
    return { level: "critical", label: "Unreachable", latencyMs: Math.round(performance.now() - t0) };
  }
}

// ---------------------------------------------------------------------------
// Poll cycle.
// ---------------------------------------------------------------------------
async function refresh() {
  const now = Date.now();

  const [rpc, ws, faucet, explorer] = await Promise.all([
    checkRpc(),
    checkWs(),
    checkReachable(ENDPOINTS.faucet),
    checkReachable(ENDPOINTS.explorer),
  ]);

  state.components.rpc      = { ...state.components.rpc,      ...rpc };
  state.components.ws       = { ...state.components.ws,       ...ws };
  state.components.faucet   = { ...state.components.faucet,   ...faucet };
  state.components.explorer = { ...state.components.explorer, ...explorer };

  if (rpc.epoch) {
    state.epoch = rpc.epoch;
    state.rpcError = null;
    const round = rpc.epoch.latest_committed_round ?? 0;

    if (state.committedRound === null) {
      state.committedRound = round;
      state.roundChangedAt = now;
    } else if (round > state.committedRound) {
      if (state.roundChangedAt !== null) {
        state.lastAdvanceIntervalMs = now - state.roundChangedAt;
      }
      state.committedRound = round;
      state.roundChangedAt = now;
    }
    // Ring-buffer sample for the sparkline (record every poll so a flat
    // line is itself the "not advancing" signal).
    state.samples.push({ t: now, round });
    if (state.samples.length > MAX_SAMPLES) state.samples.shift();
  } else {
    state.rpcError = rpc.label;
  }

  render();
}

// ---------------------------------------------------------------------------
// Freshness of the chain head, derived from roundChangedAt.
// ---------------------------------------------------------------------------
function freshness() {
  if (state.rpcError) return { level: "critical", text: "chain head unreadable" };
  if (state.committedRound === null || state.roundChangedAt === null) {
    return { level: "unknown", text: "no sample yet" };
  }
  const flatMs = Date.now() - state.roundChangedAt;
  if (flatMs > HALT_THRESHOLD_MS) return { level: "critical", text: `flat ${Math.round(flatMs / 1000)}s — halted?` };
  if (flatMs > STALL_THRESHOLD_MS) return { level: "warning",  text: `flat ${Math.round(flatMs / 1000)}s — stalling` };
  return { level: "good", text: `advancing (${Math.round(flatMs / 1000)}s since last)` };
}

// ---------------------------------------------------------------------------
// Icons — status conveyed by icon + text + colour, never colour alone.
// ---------------------------------------------------------------------------
function bannerIcon(level) {
  const color = { good: "var(--good)", warning: "var(--warning)",
    critical: "var(--critical)", unknown: "var(--text-muted)" }[level] || "var(--text-muted)";
  const paths = {
    good:     `<circle cx="16" cy="16" r="14" fill="none" stroke="${color}" stroke-width="2.5"/><path d="M9 16.5l4.5 4.5L23 11.5" fill="none" stroke="${color}" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>`,
    warning:  `<path d="M16 3l14 25H2z" fill="none" stroke="${color}" stroke-width="2.5" stroke-linejoin="round"/><path d="M16 12v7" stroke="${color}" stroke-width="2.5" stroke-linecap="round"/><circle cx="16" cy="23.5" r="1.6" fill="${color}"/>`,
    critical: `<circle cx="16" cy="16" r="14" fill="none" stroke="${color}" stroke-width="2.5"/><path d="M11 11l10 10M21 11L11 21" stroke="${color}" stroke-width="2.5" stroke-linecap="round"/>`,
    unknown:  `<circle cx="16" cy="16" r="14" fill="none" stroke="${color}" stroke-width="2.5"/><circle cx="9" cy="16" r="1.8" fill="${color}"/><circle cx="16" cy="16" r="1.8" fill="${color}"/><circle cx="23" cy="16" r="1.8" fill="${color}"/>`,
  };
  return `<svg viewBox="0 0 32 32" width="32" height="32">${paths[level] || paths.unknown}</svg>`;
}

// ---------------------------------------------------------------------------
// Render.
// ---------------------------------------------------------------------------
function render() {
  renderComponents();
  renderChain();
  renderSparkline();
  renderPerf();
  renderBanner();

  $("last-checked").textContent = new Date().toLocaleTimeString();
  $("interval").textContent = `${POLL_MS / 1000}s`;
}

function pill(level, label) {
  const p = el("span", `pill ${level}`);
  const dot = el("span", "dot");
  p.appendChild(dot);
  p.appendChild(document.createTextNode(label));
  return p;
}

function renderComponents() {
  const root = $("components");
  root.textContent = "";
  for (const key of ["rpc", "ws", "faucet", "explorer"]) {
    const c = state.components[key];
    const card = el("div", "card");

    const head = el("div", "card-head");
    const nameWrap = el("div");
    const name = el("div", "name"); name.textContent = c.name;
    const host = el("div", "host"); host.textContent = c.host;
    nameWrap.appendChild(name); nameWrap.appendChild(host);
    head.appendChild(nameWrap);
    head.appendChild(pill(c.level, c.label));
    card.appendChild(head);

    const metrics = el("div", "metrics");
    const lat = el("div", "latency");
    if (c.latencyMs !== null) {
      lat.innerHTML = `${c.latencyMs}<span class="unit"> ms</span>`;
    } else {
      lat.innerHTML = `—<span class="unit"> ms</span>`;
    }
    metrics.appendChild(lat);
    card.appendChild(metrics);

    root.appendChild(card);
  }
}

function statCard(k, v, sub, subLevel) {
  const card = el("div", "stat");
  const kk = el("div", "k"); kk.textContent = k;
  const vv = el("div", "v"); vv.textContent = v;
  card.appendChild(kk); card.appendChild(vv);
  if (sub) {
    if (subLevel) { card.appendChild(pill(subLevel, sub)); }
    else { const s = el("div", "sub"); s.textContent = sub; card.appendChild(s); }
  }
  return card;
}

function renderChain() {
  const root = $("chain");
  root.textContent = "";
  const e = state.epoch;
  const fr = freshness();
  root.appendChild(statCard("Committed round",
    e ? fmt(e.latest_committed_round ?? 0) : "—", fr.text, fr.level));
  root.appendChild(statCard("Current epoch", e ? fmt(e.current) : "—"));
  root.appendChild(statCard("Last boundary round", e ? fmt(e.last_boundary_round) : "—"));
  root.appendChild(statCard("Rounds / epoch", e ? fmt(e.rounds_per_epoch) : "—"));
}

function renderSparkline() {
  const wrap = $("spark-wrap");
  const sub = $("spark-sub");
  const samples = state.samples;

  if (samples.length < 2) {
    wrap.innerHTML = `<div class="spark-empty">Collecting samples… (${samples.length}/2)</div>`;
    return;
  }

  const rounds = samples.map(s => s.round);
  const min = Math.min(...rounds);
  const max = Math.max(...rounds);
  const span = max - min || 1;

  const W = 600, H = 64, pad = 4;
  const n = samples.length;
  const pts = samples.map((s, i) => {
    const x = pad + (i / (n - 1)) * (W - 2 * pad);
    const y = H - pad - ((s.round - min) / span) * (H - 2 * pad);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });

  // Recessive baseline axis + single 2px series line. No legend (one series;
  // the card title names it).
  const baselineY = (H - pad).toFixed(1);
  wrap.innerHTML =
    `<svg class="spark" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" role="img" ` +
    `aria-label="Committed round over the last ${n} polls, ranging ${min} to ${max}.">` +
      `<line x1="${pad}" y1="${baselineY}" x2="${W - pad}" y2="${baselineY}" ` +
        `stroke="var(--border)" stroke-width="1"/>` +
      `<polyline points="${pts.join(" ")}" fill="none" stroke="var(--series-1)" ` +
        `stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>` +
    `</svg>` +
    `<div class="spark-axis"><span>${fmt(min)}</span><span>${fmt(max)}</span></div>`;

  const advancing = max > min;
  sub.textContent = advancing
    ? `Advancing — ${fmt(min)} → ${fmt(max)} over last ${n} polls.`
    : `Flat at ${fmt(max)} across last ${n} polls.`;
}

function renderPerf() {
  const root = $("perf");
  root.textContent = "";

  // Round cadence — live if we've seen an advance, else collecting.
  // The pending state uses the `.v.pending` treatment (smaller, muted)
  // like the other perf tiles, so the hero-number font never wraps a
  // long status string mid-word.
  let cadenceVal, cadenceBadge, cadenceCls, cadenceVClass = "v";
  if (state.lastAdvanceIntervalMs !== null) {
    cadenceVal = `${(state.lastAdvanceIntervalMs / 1000).toFixed(2)}<span class="unit"> s/advance</span>`;
    cadenceBadge = "observed, this endpoint"; cadenceCls = "observed";
  } else {
    cadenceVal = "awaiting…";
    cadenceBadge = "collecting"; cadenceCls = ""; cadenceVClass = "v pending";
  }
  const cadence = el("div", "stat");
  cadence.innerHTML =
    `<div class="k">Round cadence</div><div class="${cadenceVClass}">${cadenceVal}</div>`;
  const cb = el("div", `badge ${cadenceCls}`); cb.textContent = cadenceBadge;
  cadence.appendChild(cb);
  root.appendChild(cadence);

  // Committed TPS + Fast-path finality — NOT derivable from getEpoch; these
  // come from the committed perf.json (or fall back to a static placeholder
  // if that failed to load). We never fabricate a number here.
  root.appendChild(perfMetricTile("Committed TPS", "committed_tps"));
  root.appendChild(perfMetricTile("Fast-path finality", "fastpath_finality"));
}

// A perf tile driven by perf.json. Shows value+unit with the metric's status
// badge when the value is non-null; otherwise the muted "pending" treatment
// with the metric's note. If perf.json is unavailable, falls back to the
// original static "pending — see perf run" placeholder so the page still works.
function perfMetricTile(title, key) {
  const doc = state.perf.doc;
  const m = state.perf.loaded && doc && Array.isArray(doc.metrics)
    ? doc.metrics.find((x) => x.key === key)
    : null;

  const stat = el("div", "stat");
  const k = el("div", "k"); k.textContent = title;
  stat.appendChild(k);

  // Fallback: perf.json missing/failed → keep the honest static placeholder.
  if (!m) {
    const v = el("div", "v pending"); v.textContent = "pending — see perf run";
    stat.appendChild(v);
    const link = el("a", "badge"); link.href = DOCS.perfRun; link.textContent = "docs →";
    link.style.textDecoration = "none";
    stat.appendChild(link);
    return stat;
  }

  const hasValue = m.value !== null && m.value !== undefined;
  const v = el("div", hasValue ? "v" : "v pending");
  if (hasValue) {
    v.textContent = String(m.value);
    const unit = el("span", "unit"); unit.textContent = ` ${m.unit || ""}`;
    v.appendChild(unit);
  } else {
    v.textContent = "pending";
  }
  stat.appendChild(v);

  const badge = el("div", `badge ${perfBadgeCls(m.status)}`);
  badge.textContent = m.status;
  stat.appendChild(badge);

  if (m.note) {
    const sub = el("div", "sub"); sub.textContent = m.note;
    stat.appendChild(sub);
  }
  return stat;
}

function renderBanner() {
  // Overall = worst of chain freshness + component levels, mapped to a
  // three-state operator summary.
  const fr = freshness();
  let level = fr.level;
  for (const key of Object.keys(state.components)) {
    level = worst(level, state.components[key].level);
  }

  const banner = $("banner");
  let cls, label, detail;

  // RPC unreadable OR chain halted => outage. Any reachable-but-degraded or
  // stalling => degraded. Otherwise operational.
  const rpcLevel = state.components.rpc.level;
  if (rpcLevel === "critical" || fr.level === "critical") {
    cls = "critical"; label = "Major outage";
    detail = state.rpcError
      ? `RPC ${state.rpcError.toLowerCase()} — chain head cannot be read.`
      : `Chain head ${fr.text}.`;
  } else if (LEVEL[level] >= LEVEL.warning) {
    cls = "warning"; label = "Degraded";
    const down = Object.values(state.components)
      .filter(c => c.level === "critical" || c.level === "serious")
      .map(c => c.name);
    detail = down.length
      ? `Reachability issue: ${down.join(", ")}. Chain head ${fr.text}.`
      : `Chain head ${fr.text}; some surfaces only confirmed reachable, not health-verified.`;
  } else if (LEVEL[level] === LEVEL.unknown) {
    cls = "unknown"; label = "Checking devnet…";
    detail = "Running component health checks.";
  } else {
    cls = "good"; label = "All systems operational";
    detail = `RPC healthy, chain ${fr.text}. Faucet & explorer reachable.`;
  }

  banner.className = `banner ${cls}`;
  $("banner-icon").innerHTML = bannerIcon(cls);
  $("banner-label").textContent = label;
  $("banner-detail").textContent = detail;
}

// ---------------------------------------------------------------------------
// Boot.
// ---------------------------------------------------------------------------
render();      // paint the "checking" scaffold immediately
loadPerf();    // fetch published perf figures once (non-blocking)
refresh();
setInterval(refresh, POLL_MS);
