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

import { CxError, parseAddress, toBase58 } from "../../../packages/cx-address/src/index.ts";
import { ADDRESS_FIELDS, addCx } from "../../../apps/explorer/src/cxjson.ts";

export { ADDRESS_FIELDS, addCx };
import { ApiError } from "./http.ts";

/** Request body fields read as Cadre-chain addresses (Cx turned into base58). */
export const INPUT_FIELDS = new Set([...ADDRESS_FIELDS, "accounts", "agents", "payee", "evaluator", "release_key"]);

const CX_SHAPE = /^Cx[0-9a-fA-F]{64}$/;

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
