// suwappu-devnet explorer — single-page hash-routed app.
//
// Routes:
//   #/                → Home (recent blocks + live tip)
//   #/block/<round>   → Block detail
//   #/tx/<hash>       → Transaction detail
//
// State is intentionally tiny: a polling-based hook re-fetches the
// active page's data every 3 seconds. No global store, no router
// dependency — just `window.location.hash` parsing.

import { useEffect, useState } from "react";
import { Client } from "@suwappu/client";
import { Home } from "./pages/Home.js";
import { BlockPage } from "./pages/BlockPage.js";
import { TxPage } from "./pages/TxPage.js";

type Route =
  | { kind: "home" }
  | { kind: "block"; round: number }
  | { kind: "tx"; hash: string }
  | { kind: "not-found"; raw: string };

function parseRoute(hash: string): Route {
  // Strip leading "#" and any leading "/"
  const path = hash.replace(/^#\/?/, "");
  if (path === "" || path === "/") return { kind: "home" };
  const blockMatch = path.match(/^block\/(\d+)$/);
  if (blockMatch) return { kind: "block", round: Number(blockMatch[1]) };
  const txMatch = path.match(/^tx\/(0x)?([0-9a-fA-F]{64})$/);
  if (txMatch)
    return { kind: "tx", hash: `0x${txMatch[2]!.toLowerCase()}` };
  return { kind: "not-found", raw: path };
}

export function App({ rpcUrl }: { rpcUrl: string }) {
  const [route, setRoute] = useState<Route>(() => parseRoute(window.location.hash));
  const [client] = useState(() => new Client(rpcUrl));

  useEffect(() => {
    function onHashChange() {
      setRoute(parseRoute(window.location.hash));
    }
    window.addEventListener("hashchange", onHashChange);
    return () => window.removeEventListener("hashchange", onHashChange);
  }, []);

  return (
    <div className="app">
      <header>
        <a href="#/" className="logo">
          suwappu-devnet explorer
        </a>
        <SearchBox />
        <span className="rpc-url" title="RPC URL the explorer is pointing at">
          {rpcUrl}
        </span>
      </header>
      <main>
        {route.kind === "home" && <Home client={client} />}
        {route.kind === "block" && (
          <BlockPage client={client} round={route.round} />
        )}
        {route.kind === "tx" && <TxPage client={client} hash={route.hash} />}
        {route.kind === "not-found" && (
          <div className="error">
            Unknown route: <code>{route.raw}</code>. Try{" "}
            <a href="#/">home</a>, <code>#/block/&lt;round&gt;</code>, or{" "}
            <code>#/tx/&lt;0x...&gt;</code>.
          </div>
        )}
      </main>
      <footer>
        <a href="https://github.com/suwappu/suwappu-dag">
          suwappu-dag on GitHub
        </a>
        {" · "}
        <a href="https://github.com/suwappu/suwappu-dag/blob/main/DEVNET.md">
          DEVNET.md
        </a>
        {" · "}
        <a href="https://github.com/suwappu/suwappu-dag/blob/main/OPERATIONS.md">
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
    <form onSubmit={onSubmit} className="search">
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
