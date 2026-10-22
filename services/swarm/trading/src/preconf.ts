// Pre-confirmation risk gate for trading agents (pearl-sol docs/SPEED.md 2.5).
//
// A tx the agent sent (a trade, a mandate call, a receipt Post) moves pending -> preconfirmed
// (weak blocks on the current tip list it) -> included -> final. Preconfirmed is evidence that
// miners are working on the tx, not a promise: the next block's miner can still leave it out.
// So an agent may act on preconfirmed only for small notionals and only with confidence of at
// least `minConfidence`; up to `includedCap` it waits for inclusion; above that, for finality.
// Every decision carries a one-line stated risk for the agent's log and receipt.
//
// The caps are operator policy (env TRADE_PRECONF_CAP / TRADE_INCLUDED_CAP in CADRE base
// units, TRADE_PRECONF_MIN_CONFIDENCE medium|high), not measured figures.

import { normalizeTxStatus, type Confidence, type TxStatus } from "../../../../apps/explorer/src/txstatus.ts";

export type Need = "preconfirmed" | "included" | "final";

export interface PreconfPolicy {
  /** Largest notional (CADRE base units, 9 dp) an agent may act on while only preconfirmed; 0 = never. */
  preconfCap: bigint;
  /** Largest notional that may act once included; above it the agent waits for final. */
  includedCap: bigint;
  minConfidence: Exclude<Confidence, "low">;
}

const UNIT = 1_000_000_000n;

/** Policy defaults (tunable): act on preconfirmed up to 10 CADRE, on included up to 1,000 CADRE. */
export function policyFromEnv(env: Record<string, string | undefined> = process.env): PreconfPolicy {
  const big = (v: string | undefined, d: bigint) => (v !== undefined && /^\d+$/.test(v) ? BigInt(v) : d);
  const mc = env.TRADE_PRECONF_MIN_CONFIDENCE === "high" ? "high" : "medium";
  return { preconfCap: big(env.TRADE_PRECONF_CAP, 10n * UNIT), includedCap: big(env.TRADE_INCLUDED_CAP, 1000n * UNIT), minConfidence: mc };
}

/** The status a notional must reach before the agent acts on it. */
export function required(notional: bigint, p: PreconfPolicy): Need {
  if (notional <= p.preconfCap && p.preconfCap > 0n) return "preconfirmed";
  if (notional <= p.includedCap) return "included";
  return "final";
}

const RANK: Record<string, number> = { pending: 0, preconfirmed: 1, included: 2, final: 3 };
const CONF: Record<Confidence, number> = { low: 0, medium: 1, high: 2 };

/** Does status `s` let the agent act when it needs `need`? */
export function satisfies(s: TxStatus, need: Need, p: PreconfPolicy): boolean {
  if (need === "preconfirmed" && s.status === "preconfirmed") return !!s.preconf && CONF[s.preconf.confidence] >= CONF[p.minConfidence];
  return RANK[s.status] >= RANK[need];
}

/** The stated risk of acting now on `s` (one line, for the log and the receipt). */
export function statedRisk(s: TxStatus, need: Need): string {
  if (s.status === "preconfirmed") {
    const c = s.preconf;
    return `acted on preconfirmed (${c?.weak_count ?? 0} weak blocks, ${c?.weak_miners ?? 0} miners, ${c?.confidence ?? "low"}): not binding, the next block may still leave the tx out; notional within the preconf cap`;
  }
  if (s.status === "included") return `acted on included (${s.confirmations ?? "?"}${s.finality_k !== null ? ` of ${s.finality_k}` : ""} confirmations): a reorg can still undo it until final`;
  if (s.status === "final") return "acted on final: cannot be reorged";
  return `not acted: still ${s.status}, needs ${need}`;
}

export interface RpcLike {
  call(method: string, params?: unknown): Promise<any>;
}

/** getTxStatus, or getTx on a node from before the weak-block upgrade. */
export async function readStatus(rpc: RpcLike, txHash: string, chain?: { height?: number; finalized_height?: number; finality_k?: number } | null): Promise<TxStatus | null> {
  let raw: any;
  try {
    raw = await rpc.call("getTxStatus", { tx_hash: txHash });
  } catch (e) {
    if (!/unknown method|method not found/i.test((e as Error)?.message ?? "")) throw e;
    raw = await rpc.call("getTx", { tx_hash: txHash });
    if (raw && raw.status === "included" && !chain) chain = await rpc.call("getChainInfo").catch(() => null);
  }
  return normalizeTxStatus(raw ? { tx_hash: txHash, ...raw } : null, chain ?? null);
}

export interface Settled {
  tx_hash: string;
  need: Need;
  status: TxStatus["status"];
  tx_status: TxStatus;
  risk: string;
  waited_ms: number;
}

/**
 * Wait until `txHash` reaches what `notional` needs under policy `p`, then return the status
 * and its stated risk. Throws on timeout (the caller logs it and does not act).
 */
export async function waitSettled(rpc: RpcLike, txHash: string, notional: bigint, p: PreconfPolicy, o: { timeoutMs?: number; pollMs?: number } = {}): Promise<Settled> {
  const need = required(notional, p);
  const t0 = Date.now();
  const timeout = o.timeoutMs ?? 3_600_000;
  const poll = o.pollMs ?? 500;
  let last: TxStatus | null = null;
  for (;;) {
    last = (await readStatus(rpc, txHash).catch(() => null)) ?? last;
    if (last && satisfies(last, need, p)) return { tx_hash: txHash, need, status: last.status, tx_status: last, risk: statedRisk(last, need), waited_ms: Date.now() - t0 };
    if (Date.now() - t0 >= timeout) throw new Error(`tx ${txHash} still ${last?.status ?? "unknown"} after ${timeout} ms (needs ${need})`);
    await new Promise((r) => setTimeout(r, poll));
  }
}
