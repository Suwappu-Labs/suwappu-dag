// Batched recent-blocks fetch.
//
// Replaces the previous one-at-a-time backwards `await getBlock` loop
// (N+1, serialized) with a bounded parallel fetch: request a window of
// the last `WINDOW` rounds in parallel via Promise.all, drop nulls
// (skipped rounds — Mysticeti-C `Skip`), and keep the newest `KEEP`.
//
// Concurrency is bounded by the window size, so a refresh issues at
// most WINDOW in-flight requests — it does not walk unbounded toward
// round 0.

import type { Client, BlockView } from "@suwappu/client";

export const WINDOW = 40;
export const KEEP = 30;

export async function fetchRecentBlocks(
  client: Client,
  tip: number,
): Promise<BlockView[]> {
  const start = Math.max(0, tip - WINDOW + 1);
  const rounds: number[] = [];
  for (let r = tip; r >= start; r--) rounds.push(r);

  const results = await Promise.all(rounds.map((r) => client.getBlock(r)));

  const blocks: BlockView[] = [];
  for (const b of results) {
    if (b !== null) blocks.push(b);
    if (blocks.length >= KEEP) break;
  }
  return blocks;
}
