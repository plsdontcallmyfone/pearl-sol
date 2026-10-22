// Agent-chain tx status (docs/SPEED.md 2.4/2.5): pending / preconfirmed / included / final.
//
// The node's getTx (and getTxStatus, from the weak-block upgrade) says `status` and, for a tx
// listed by weak blocks on the tip, a `preconf` object. Older nodes say only "pending" or
// "included" and no `final` flag: then final is derived from the chain's finalized height (or
// confirmations > finality_k). Shared by the explorer, cadre-api and the trading gate.

export type TxState = "pending" | "preconfirmed" | "included" | "final";
export type Confidence = "low" | "medium" | "high";

export interface Preconf {
  weak_count: number;
  weak_miners: number;
  weak_share: number;
  first_seen_ms: number | null;
  latest_age_ms: number | null;
  confidence: Confidence;
}

export interface TxStatus {
  tx_hash: string | null;
  status: TxState;
  preconf: Preconf | null;
  block_hash: string | null;
  height: number | null;
  confirmations: number | null;
  final: boolean;
  finality_k: number | null;
  /** "node": the node said it; "derived": final worked out from the finalized height or K. */
  final_source: "node" | "derived" | null;
  /** One honest line about what this status does and does not guarantee (SPEED.md 2.5). */
  risk: string;
}

/** Chain facts used to derive `final` from an older node's answer. */
export interface ChainFacts {
  height?: number;
  finalized_height?: number;
  finality_k?: number;
}

const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : typeof v === "string" && v !== "" && Number.isFinite(Number(v)) ? Number(v) : null);

/** Confidence from the counts, SPEED.md 2.4, when the node sent none (or an unknown label). */
export function confidenceOf(weak_count: number, weak_miners: number, weak_share: number): Confidence {
  if (weak_count >= 4 && weak_miners >= 2 && weak_share >= 0.9) return "high";
  if (weak_count >= 2 && weak_miners >= 2) return "medium";
  return "low";
}

export function parsePreconf(p: any): Preconf | null {
  if (!p || typeof p !== "object") return null;
  const weak_count = num(p.weak_count);
  if (weak_count === null || weak_count < 1) return null;
  const weak_miners = Math.max(1, num(p.weak_miners) ?? 1);
  const share = num(p.weak_share);
  const weak_share = share === null ? 0 : Math.min(1, Math.max(0, share));
  const c = p.confidence;
  const confidence: Confidence = c === "low" || c === "medium" || c === "high" ? c : confidenceOf(weak_count, weak_miners, weak_share);
  return { weak_count, weak_miners, weak_share, first_seen_ms: num(p.first_seen_ms), latest_age_ms: num(p.latest_age_ms), confidence };
}

const pct = (x: number) => `${Math.round(x * 100)}%`;

export function riskLine(s: Pick<TxStatus, "status" | "preconf" | "confirmations" | "finality_k" | "height">): string {
  switch (s.status) {
    case "pending":
      return "In the mempool; no miner has committed to it yet.";
    case "preconfirmed": {
      const p = s.preconf;
      if (!p || p.confidence === "low") return "Seen in one miner's weak block on the current tip. Not binding: the next block may still leave it out.";
      return `Listed by ${p.weak_count} weak blocks from ${p.weak_miners} miners (${pct(p.weak_share)} of recent weak blocks). Not binding until included: fine for small, reversible actions only.`;
    }
    case "included": {
      const k = s.finality_k;
      const c = s.confirmations;
      return `In block ${s.height ?? "?"}${c !== null ? `, ${c}${k !== null ? ` of ${k}` : ""} confirmations` : ""}. A reorg can still undo it until it is final.`;
    }
    case "final":
      return `Final: at or below the finalized height${s.finality_k !== null ? ` (${s.finality_k} blocks deep)` : ""}; it cannot be reorged.`;
  }
}

/** Normalize a getTx / getTxStatus answer. Returns null for an unknown tx (null answer). */
export function normalizeTxStatus(tx: any, chain?: ChainFacts | null): TxStatus | null {
  if (!tx || typeof tx !== "object") return null;
  const preconf = parsePreconf(tx.preconf);
  const height = num(tx.height);
  const confirmations = num(tx.confirmations);
  const k = num(tx.finality_k) ?? num(chain?.finality_k);
  let status: TxState;
  let final = false;
  let final_source: TxStatus["final_source"] = null;
  const raw = String(tx.status ?? "");
  if (raw === "final" || (tx.final === true && height !== null)) {
    status = "final";
    final = true;
    final_source = "node";
  } else if (raw === "included" || (height !== null && raw !== "pending" && raw !== "preconfirmed")) {
    status = "included";
    if (tx.final === false) {
      // the node says not final: trust it
    } else {
      const fh = num(chain?.finalized_height);
      if (height !== null && fh !== null && height <= fh && (confirmations === null || confirmations > 0)) final = true;
      else if (confirmations !== null && k !== null && confirmations > k) final = true;
      if (final) {
        status = "final";
        final_source = "derived";
      }
    }
  } else if (raw === "preconfirmed" || preconf) {
    status = "preconfirmed";
  } else {
    status = "pending";
  }
  const out: TxStatus = {
    tx_hash: typeof tx.tx_hash === "string" ? tx.tx_hash : typeof tx.hash === "string" ? tx.hash : null,
    status,
    preconf: status === "preconfirmed" ? preconf : null,
    block_hash: typeof tx.block_hash === "string" ? tx.block_hash : null,
    height: status === "included" || status === "final" ? height : null,
    confirmations: status === "included" || status === "final" ? confirmations : null,
    final,
    finality_k: k,
    final_source,
    risk: "",
  };
  out.risk = riskLine(out);
  return out;
}

/** Seconds, one decimal, for "age" labels. */
export function fmtAge(ms: number | null): string {
  if (ms === null) return "unknown age";
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s ago`;
  return `${Math.round(ms / 60_000)} min ago`;
}

/** Short human label: "preconfirmed (3 weak blocks, 2 miners, medium, 1.2 s ago)". */
export function statusLabel(s: TxStatus): string {
  switch (s.status) {
    case "pending":
      return "pending";
    case "preconfirmed": {
      const p = s.preconf;
      if (!p) return "preconfirmed";
      return `preconfirmed (${p.weak_count} weak block${p.weak_count === 1 ? "" : "s"}, ${p.weak_miners} miner${p.weak_miners === 1 ? "" : "s"}, ${p.confidence}, latest ${fmtAge(p.latest_age_ms)})`;
    }
    case "included":
      return `included (${s.confirmations ?? "?"}${s.finality_k !== null ? `/${s.finality_k}` : ""} confirmations)`;
    case "final":
      return "final";
  }
}
