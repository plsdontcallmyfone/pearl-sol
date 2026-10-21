// Cx address fields in JSON (docs/SPEC.md 2.1): `k_cx` siblings next to Cadre-chain address
// fields, base58 kept for the transition. Shared by the explorer's /api and cadre-api replies.
// Solana-side fields (owner, sender, payer, buyer, mint, vault, pool, lender, solana_*, Solana
// signatures and PDAs) are not in the list and stay base58 only.

import { parseAddress, toBase58, toCx } from "../../../packages/cx-address/src/index.ts";

const B58_KEY = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/;

/** Reply field names that hold a Cadre-chain account address. */
export const ADDRESS_FIELDS = new Set([
  "account", "address", "admin", "admins", "agent", "a", "b", "attesters", "author", "contract", "creator",
  "deliverer", "deployer", "escrow", "evaluator", "evaluators", "follower", "from", "house_operators", "member",
  "members", "miner", "operator", "peer", "poster", "recipient", "registrant", "release_key", "signers", "target", "to",
  "refund_to", "payout",
]);

function cxOf(s: unknown): string | null {
  if (typeof s !== "string" || !B58_KEY.test(s)) return null;
  try {
    return toCx(parseAddress(s));
  } catch {
    return null;
  }
}

/** Subtrees left exactly as built: wallet request payloads (window.pearl forms) are signed as is. */
export const SKIP_FIELDS = new Set(["window_pearl", "transaction", "solana"]);

/** A copy of v with `k_cx` siblings for Cadre address fields, recursively (v is not changed:
 *  replies often hand out indexer records directly). */
export function addCx(v: any): any {
  if (Array.isArray(v)) return v.map(addCx);
  if (!v || typeof v !== "object" || v instanceof Uint8Array || Buffer.isBuffer(v)) return v;
  if (typeof v.toJSON === "function") return v;
  if (v instanceof Map || v instanceof Set) return v;
  const out: Record<string, unknown> = {};
  for (const k of Object.keys(v)) {
    const x = v[k];
    out[k] = x && typeof x === "object" && !SKIP_FIELDS.has(k) ? addCx(x) : x;
    if (!ADDRESS_FIELDS.has(k) || `${k}_cx` in v) continue;
    if (typeof x === "string") {
      const c = cxOf(x);
      if (c) out[`${k}_cx`] = c;
    } else if (Array.isArray(x) && x.length && x.every((e) => typeof e === "string")) {
      const cs = x.map(cxOf);
      if (cs.every((c) => c)) out[`${k}_cx`] = cs;
    }
  }
  return out;
}

export const CX_SHAPE = /^Cx[0-9a-fA-F]{64}$/;

/** An input value: a Cx-shaped string becomes base58 (error text when its checksum fails);
 *  anything else is returned unchanged. */
export function cxInput(v: string): { value: string } | { error: string; code: string } {
  const t = v.trim();
  if (!CX_SHAPE.test(t)) return { value: v };
  try {
    return { value: toBase58(parseAddress(t)) };
  } catch (e) {
    return { error: (e as Error).message, code: (e as { code?: string }).code ?? "invalid" };
  }
}
