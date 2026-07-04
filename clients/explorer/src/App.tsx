// suwappu-devnet explorer — single-page hash-routed app.
//
// Routes:
//   #/               → Overview (network dashboard)
//   #/validators     → Dual-ring validator sets
//   #/attestation    → Post-quantum bridge attestation
//   #/block/<round>  → Block detail
//   #/tx/<hash>      → Transaction detail
//
// State is intentionally tiny: each page owns its own polling. No
// global store, no router dependency — just `window.location.hash`.

import { useEffect, useState } from "react";
import { Client } from "@suwappu/client";
import { Home } from "./pages/Home.js";
import { Validators } from "./pages/Validators.js";
import { Attestation } from "./pages/Attestation.js";
import { Capabilities } from "./pages/Capabilities.js";
import { BlockPage } from "./pages/BlockPage.js";
import { TxPage } from "./pages/TxPage.js";

type Route =
  | { kind: "home" }
  | { kind: "validators" }
  | { kind: "attestation" }
  | { kind: "capabilities" }
  | { kind: "block"; round: number }
  | { kind: "tx"; hash: string }
  | { kind: "not-found"; raw: string };

function parseRoute(hash: string): Route {
  const path = hash.replace(/^#\/?/, "");
  if (path === "" || path === "/") return { kind: "home" };
  if (path === "validators") return { kind: "validators" };
  if (path === "attestation") return { kind: "attestation" };
  if (path === "capabilities") return { kind: "capabilities" };
  const blockMatch = path.match(/^block\/(\d+)$/);
  if (blockMatch) return { kind: "block", round: Number(blockMatch[1]) };
  const txMatch = path.match(/^tx\/(0x)?([0-9a-fA-F]{64})$/);
  if (txMatch) return { kind: "tx", hash: `0x${txMatch[2]!.toLowerCase()}` };
  return { kind: "not-found", raw: path };
}

const NAV: Array<{ href: string; label: string; match: Route["kind"] }> = [
  { href: "#/", label: "Overview", match: "home" },
  { href: "#/validators", label: "Validators", match: "validators" },
  { href: "#/attestation", label: "Attestation", match: "attestation" },
  { href: "#/capabilities", label: "Capabilities", match: "capabilities" },
];

export function App({ rpcUrl }: { rpcUrl: string }) {
  const [route, setRoute] = useState<Route>(() =>
    parseRoute(window.location.hash),
  );
  const [client] = useState(() => new Client(rpcUrl));
  const [menuOpen, setMenuOpen] = useState(false);

  useEffect(() => {
    function onHashChange() {
      setRoute(parseRoute(window.location.hash));
      setMenuOpen(false);
    }
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  return (
    <div className="app">
      <header className="topbar">
        <div className="topbar-row">
          <a href="#/" className="logo">
            <span className="logo-mark" aria-hidden="true">
              ▤
            </span>
            suwappu<span className="logo-dim">-devnet</span>
          </a>
          <button
            type="button"
            className="nav-toggle"
            aria-label="Toggle navigation"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen((v) => !v)}
          >
            <svg width="20" height="20" viewBox="0 0 24 24" aria-hidden="true">
              <path
                d="M4 7h16M4 12h16M4 17h16"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
              />
            </svg>
          </button>
          <nav className={`nav${menuOpen ? " open" : ""}`} aria-label="Primary">
            {NAV.map((item) => (
              <a
                key={item.href}
                href={item.href}
                className={`nav-link${route.kind === item.match ? " active" : ""}`}
                aria-current={route.kind === item.match ? "page" : undefined}
              >
                {item.label}
              </a>
            ))}
          </nav>
        </div>
        <div className="topbar-tools">
          <SearchBox />
          <span
            className="rpc-url"
            title="RPC endpoint this explorer is pointing at"
          >
            <span className="rpc-dot" aria-hidden="true" />
            {rpcUrl}
          </span>
        </div>
      </header>

      <main>
        {route.kind === "home" && <Home client={client} />}
        {route.kind === "validators" && <Validators client={client} />}
        {route.kind === "attestation" && <Attestation client={client} />}
        {route.kind === "capabilities" && <Capabilities />}
        {route.kind === "block" && (
          <BlockPage client={client} round={route.round} />
        )}
        {route.kind === "tx" && <TxPage client={client} hash={route.hash} />}
        {route.kind === "not-found" && (
          <div className="page">
            <div className="state error" role="alert">
              Unknown route: <code>{route.raw}</code>. Try{" "}
              <a href="#/">Overview</a>, <code>#/block/&lt;round&gt;</code>, or{" "}
              <code>#/tx/&lt;0x…&gt;</code>.
            </div>
          </div>
        )}
      </main>

      <footer>
        <a href="https://github.com/Suwappu-Labs/suwappu-dag">
          suwappu-dag on GitHub
        </a>
        {" · "}
        <a href="https://github.com/Suwappu-Labs/suwappu-dag/blob/main/DEVNET.md">
          DEVNET.md
        </a>
        {" · "}
        <a href="https://github.com/Suwappu-Labs/suwappu-dag/blob/main/OPERATIONS.md">
          OPERATIONS.md
        </a>
      </footer>
    </div>
  );
}

function SearchBox() {
  const [value, setValue] = useState("");
  function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = value.trim();
    if (/^\d+$/.test(trimmed)) {
      window.location.hash = `#/block/${trimmed}`;
    } else if (/^(0x)?[0-9a-fA-F]{64}$/.test(trimmed)) {
      const h = trimmed.startsWith("0x") ? trimmed : `0x${trimmed}`;
      window.location.hash = `#/tx/${h.toLowerCase()}`;
    }
    setValue("");
  }
  return (
    <form onSubmit={onSubmit} className="search" role="search">
      <svg
        className="search-icon"
        width="16"
        height="16"
        viewBox="0 0 24 24"
        aria-hidden="true"
      >
        <circle
          cx="11"
          cy="11"
          r="7"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
        />
        <path
          d="M21 21l-4.3-4.3"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
        />
      </svg>
      <input
        type="text"
        placeholder="Round number or 0x-tx-hash"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        aria-label="Search by block round or tx hash"
      />
      <button type="submit">Go</button>
    </form>
  );
}
