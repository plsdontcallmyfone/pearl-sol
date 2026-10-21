// Tx status for wallets, agents and the site (docs/SPEED.md 2.4/2.5): pending / preconfirmed
// (weak-block evidence on the current tip) / included (N of K confirmations) / final.
// GET /v1/tx/chain/{hash}/status asks the node's getTxStatus and falls back to getTx on a node
// from before the weak-block upgrade; `final` is then derived from the finalized height.

import { Api, ApiError, badRequest, notFound } from "../http.ts";
import type { Deps } from "../deps.ts";
import { normalizeTxStatus, type ChainFacts, type TxStatus } from "../../../../apps/explorer/src/txstatus.ts";

const unknownMethod = (e: unknown) => /unknown method|method not found/i.test((e as Error)?.message ?? String(e));

async function facts(d: Deps): Promise<ChainFacts | null> {
  const ci = d.ix?.chainInfo;
  if (ci && typeof ci.height === "number") return ci;
  try {
    return await d.chain<any>("getChainInfo", {});
  } catch {
    return null;
  }
}

/** Normalized status of `hash` (lowercase hex), or null when the node does not know it. */
export async function txStatus(d: Deps, hash: string): Promise<(TxStatus & { node_api: "getTxStatus" | "getTx" }) | null> {
  let raw: any;
  let node_api: "getTxStatus" | "getTx" = "getTxStatus";
  try {
    raw = await d.chain<any>("getTxStatus", { tx_hash: hash });
  } catch (e) {
    if (!unknownMethod(e)) throw e;
    node_api = "getTx";
    raw = await d.chain<any>("getTx", { tx_hash: hash });
  }
  const s = normalizeTxStatus(raw ? { tx_hash: hash, ...raw } : null, await facts(d));
  return s ? { ...s, tx_hash: hash, node_api } : null;
}

/** The normalized status added to a getTx answer (GET /v1/tx/chain/{hash}). */
export async function withTxStatus(d: Deps, t: any): Promise<any> {
  return { ...t, tx_status: normalizeTxStatus(t, await facts(d)) };
}

export function registerTxStatus(api: Api, d: Deps) {
  api.get("/v1/tx/chain/:hash/status", async (c) => {
    const h = c.params.hash;
    if (!/^[0-9a-fA-F]{64}$/.test(h)) throw badRequest("hash is 64 hex");
    let s;
    try {
      s = await txStatus(d, h.toLowerCase());
    } catch (e) {
      throw new ApiError(502, "node_error", `the node did not answer: ${((e as Error).message ?? String(e)).slice(0, 200)}`);
    }
    if (!s) throw notFound("tx not known to the node (not in the mempool, not in a weak block on the tip, not in the canonical chain)");
    return s;
  }, { cache: 1 });
}
