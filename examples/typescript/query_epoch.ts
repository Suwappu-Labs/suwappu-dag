/**
 * Example: fetch the current epoch from a running devnet's JSON-RPC.
 *
 * Run:
 *   npm run example:query-epoch
 *
 * Pre-req: a devnet up at `http://127.0.0.1:9092`. See ../../DEVNET.md.
 */

import { Client } from "@gsx/client";

const client = new Client("http://127.0.0.1:9092");

const epoch = await client.getEpoch();
console.log(`current epoch         : ${epoch.current}`);
console.log(`last boundary round   : ${epoch.last_boundary_round}`);
console.log(`rounds per epoch      : ${epoch.rounds_per_epoch}`);
