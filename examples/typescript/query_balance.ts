/**
 * Example: fetch a substrate balance for a 20-byte address.
 *
 * Run:
 *   npm run example:query-balance -- 0x0101010101010101010101010101010101010101
 *
 * With no address argument, queries the zero address (always "0" on
 * a fresh devnet — useful as a smoke test).
 */

import { Client } from "@suwappu/client";

const addrArg =
  process.argv[2] ?? "0x0000000000000000000000000000000000000000";

const client = new Client("http://127.0.0.1:9092");
const view = await client.getBalance(addrArg);
console.log(`address : ${addrArg}`);
console.log(`balance : ${view.balance} (decimal string; lift to BigInt for math)`);
