// Cx addresses at the cadre-api edge (docs/SPEC.md 2.1, docs/API.md "Addresses").
//
// Input: a path param, query value or address-named JSON body field may be a Cx address
// (checksummed or all lowercase) or the legacy base58 form. Cx values are turned into base58
// before any handler runs, so routes, the explorer indexer and nodes that predate Cx keep
// working unchanged. A Cx-shaped value with the wrong casing is a 400 `bad_address` (code
// `checksum` in details) with the parser's message.
//
// Output: next to every Cadre-chain address field `k` in a JSON reply this adds `k_cx`; the
// base58 field stays (transition: clients parse it today). Solana-side fields (owner, sender,
// payer, mint, vault, pool, lender, solana_*, Solana signatures and links) are not touched.

import { CxError, parseAddress, toBase58, toCx } from "../../../packages/cx-address/src/index.ts";
import { ApiError } from "./http.ts";

/** Reply field names that hold a Cadre-chain account address. */
export const ADDRESS_FIELDS = new Set([
  "account", "address", "admin", "admins", "agent", "a", "b", "attesters", "author", "contract", "creator",
  "deliverer", "deployer", "escrow", "evaluator", "evaluators", "follower", "from", "house_operators", "member",
  "members", "miner", "operator", "peer", "poster", "recipient", "registrant", "release_key", "signers", "target", "to",
  "refund_to", "payout",
]);

/** Request body fields read as Cadre-chain addresses (Cx turned into base58). */
export const INPUT_FIELDS = new Set([...ADDRESS_FIELDS, "accounts", "agents", "payee", "evaluator", "release_key"]);

const CX_SHAPE = /^Cx[0-9a-fA-F]{64}$/;
const B58_KEY = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/;

export function badAddress(e: unknown, where: string): ApiError {
  const ce = e instanceof CxError ? e : null;
  return new ApiError(400, "bad_address", `${where}: ${(e as Error).message}`, { cx_code: ce?.code ?? "invalid" });
}

/** A Cx-shaped string to base58 (throws 400 on a checksum failure); anything else unchanged. */
export function cxToBase58(v: string, where: string): string {
  const t = v.trim();
  if (!t.startsWith("Cx")) return v;
  if (!CX_SHAPE.test(t)) return v;
  try {
    return toBase58(parseAddress(t));
  } catch (e) {
    throw badAddress(e, where);
  }
}

export function normalizeParams(params: Record<string, string>, query: URLSearchParams) {
  for (const k of Object.keys(params)) params[k] = cxToBase58(params[k], k);
  for (const [k, v] of [...query.entries()]) {
    const n = cxToBase58(v, k);
    if (n !== v) query.set(k, n);
  }
}

/** Address-named body fields (and arrays of them) from Cx to base58, recursively. */
export function normalizeBody(v: any, key = ""): any {
  if (typeof v === "string") return INPUT_FIELDS.has(key) ? cxToBase58(v, key) : v;
  if (Array.isArray(v)) return v.map((x) => normalizeBody(x, key));
  if (v && typeof v === "object") {
    for (const k of Object.keys(v)) v[k] = normalizeBody(v[k], k);
  }
  return v;
}

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
