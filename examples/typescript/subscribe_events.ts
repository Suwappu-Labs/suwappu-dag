/**
 * Example: tail the devnet's commit stream via WebSocket.
 *
 * Run:
 *   npm run example:subscribe-events
 *
 * Prints one line per EventView until Ctrl-C. Pre-req: devnet up at
 * `ws://127.0.0.1:9092/ws`. See ../../DEVNET.md.
 *
 * Node ≥ 22 ships a native `WebSocket`; on Node 20/21 we inject the
 * `ws` package's class so the SDK doesn't have to know which runtime
 * it's on.
 */

import { Client } from "@suwappu/client";
import { WebSocket as WsWebSocket } from "ws";

const client = new Client("http://127.0.0.1:9092");

const sub = client.subscribeEvents({
  // Inject the `ws` package's WebSocket for Node 20/21 callers.
  // Node ≥ 22 has globalThis.WebSocket; passing this explicitly
  // makes the example work on either.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  WebSocket: WsWebSocket as any,
  onEvent: (ev) => {
    console.log(
      JSON.stringify({
        event: ev.event,
        round: ev.round,
        cert_hash: ev.cert_hash,
        lane: ev.lane,
        region: ev.region,
      }),
    );
  },
  onError: (err) => {
    console.error(`ws error: ${err.message}`);
  },
  onClose: () => {
    console.log("ws closed");
  },
});

console.log("subscribing to ws://127.0.0.1:9092/ws (Ctrl-C to exit)");

// Keep the process alive until SIGINT.
process.on("SIGINT", () => {
  console.log("\nclosing...");
  sub.close();
  process.exit(0);
});
