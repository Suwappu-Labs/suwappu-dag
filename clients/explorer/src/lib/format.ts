// Formatting helpers shared across pages.

/** Truncate a hex string as `head…tail`. */
export function shortHash(hex: string, head = 10, tail = 8): string {
  if (hex.length <= head + tail + 1) return hex;
  return `${hex.slice(0, head)}…${hex.slice(-tail)}`;
}

/**
 * Group-format an integer amount for display. Accepts a `number`
 * (Authority stakes, ≤ 2^53) or a decimal string / bigint (Validator
 * stakes, u128). Uses grouping separators without losing precision on
 * large values.
 */
export function formatStake(value: number | string | bigint): string {
  let big: bigint;
  try {
    big = typeof value === "bigint" ? value : BigInt(value);
  } catch {
    // Non-integer or malformed — fall back to a plain string.
    return String(value);
  }
  return big.toLocaleString("en-US");
}

/**
 * Ratio of `value` to `max` as a float in 0..1, computed via BigInt so
 * it stays correct for u128-scale stakes. Returns 0 when `max` is 0.
 */
export function stakeFraction(
  value: number | string | bigint,
  max: number | string | bigint,
): number {
  let v: bigint;
  let m: bigint;
  try {
    v = typeof value === "bigint" ? value : BigInt(value);
    m = typeof max === "bigint" ? max : BigInt(max);
  } catch {
    return 0;
  }
  if (m <= 0n) return 0;
  // Scale to 4 significant fractional digits before converting to Number.
  const scaled = (v * 10000n) / m;
  return Number(scaled) / 10000;
}

/** Total of a list of stake values, as a bigint. */
export function totalStake(values: Array<number | string | bigint>): bigint {
  let sum = 0n;
  for (const val of values) {
    try {
      sum += typeof val === "bigint" ? val : BigInt(val);
    } catch {
      // skip malformed
    }
  }
  return sum;
}
