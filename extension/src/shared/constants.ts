// Product name and units. The name is a placeholder: change it here only.
export const APP_NAME = "Pearl Wallet";
/** Token ticker is unsettled (SPEC 8); placeholder shown next to amounts. */
export const TOKEN_SYMBOL = "PEARL";
export const CHAIN_NAME = "Agent chain";

export const SOL_DECIMALS = 9;
export const SPL_DECIMALS = 6;
export const NATIVE_DECIMALS = 9;
/** 1 SPL base unit = 1000 chain base units (SPEC 1). */
export const SPL_TO_NATIVE = 1000n;

export const DEFAULT_AUTOLOCK_MIN = 15;
export const MESSAGE_NAMESPACE = "pearl-sol:message:v1";

export function formatUnits(v: bigint | string, decimals: number, maxFrac = 4): string {
  const x = typeof v === "string" ? BigInt(v) : v;
  const neg = x < 0n;
  const a = neg ? -x : x;
  const base = 10n ** BigInt(decimals);
  const whole = a / base;
  let frac = (a % base).toString().padStart(decimals, "0").slice(0, maxFrac);
  frac = frac.replace(/0+$/, "");
  const w = whole.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${neg ? "-" : ""}${w}${frac ? "." + frac : ""}`;
}

/** Fixed decimals (for balances that update; avoids width jitter). */
export function formatFixed(v: bigint | string, decimals: number, places = 4): string {
  const x = typeof v === "string" ? BigInt(v) : v;
  const base = 10n ** BigInt(decimals);
  const whole = x / base;
  const frac = (x % base).toString().padStart(decimals, "0").slice(0, places);
  const w = whole.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return places > 0 ? `${w}.${frac}` : w;
}

export function parseUnits(s: string, decimals: number): bigint {
  const t = s.trim().replace(/,/g, "");
  if (!/^\d*(\.\d*)?$/.test(t) || t === "" || t === ".") throw new Error("enter a number");
  const [w, f = ""] = t.split(".");
  if (f.length > decimals) throw new Error(`at most ${decimals} decimal places`);
  return BigInt(w || "0") * 10n ** BigInt(decimals) + BigInt((f + "0".repeat(decimals)).slice(0, decimals) || "0");
}

export const short = (s: string, n = 4) => (s.length > 2 * n + 3 ? `${s.slice(0, n)}...${s.slice(-n)}` : s);
