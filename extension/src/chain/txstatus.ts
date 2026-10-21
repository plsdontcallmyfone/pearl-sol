// Agent-chain tx status for the activity list (docs/SPEED.md 2.4/2.5): pending, preconfirmed
// (weak blocks on the current tip list the tx), included ("confirmed" in stored activity),
// final. An older node has no preconfirmed status and no `final` flag: final is then derived
// from the chain's finalized height.

import type { ChainInfo, TxInfo } from "./rpc";
import type { ActivityItem } from "../shared/types";

export type Confidence = "low" | "medium" | "high";
export interface Preconf {
  weak_count: number;
  weak_miners: number;
  weak_share: number;
  first_seen_ms?: number | null;
  latest_age_ms?: number | null;
  confidence: Confidence;
}

export function confidenceOf(count: number, miners: number, share: number): Confidence {
  if (count >= 4 && miners >= 2 && share >= 0.9) return "high";
  if (count >= 2 && miners >= 2) return "medium";
  return "low";
}

function preconfOf(p: Partial<Preconf> | null | undefined): Preconf | null {
  if (!p || typeof p.weak_count !== "number" || p.weak_count < 1) return null;
  const miners = Math.max(1, p.weak_miners ?? 1);
  const share = Math.min(1, Math.max(0, p.weak_share ?? 0));
  const c = p.confidence === "low" || p.confidence === "medium" || p.confidence === "high" ? p.confidence : confidenceOf(p.weak_count, miners, share);
  return { weak_count: p.weak_count, weak_miners: miners, weak_share: share, first_seen_ms: p.first_seen_ms ?? null, latest_age_ms: p.latest_age_ms ?? null, confidence: c };
}

/** One honest line per status (SPEED.md 2.5). */
export const PRECONF_NOTE = "Preconfirmed: a miner is working on it in the current tip. Not binding until it is in a block.";

export function activityStatus(t: TxInfo | null, info?: Pick<ChainInfo, "finalized_height" | "finality_k"> | null): { status: ActivityItem["status"]; detail?: string } | null {
  if (!t) return null;
  const pre = preconfOf(t.preconf);
  if (t.status === "final" || (t.final === true && t.height !== undefined)) return { status: "final", detail: `Final at block ${t.height}` };
  if (t.status === "included" || t.height !== undefined) {
    const k = info?.finality_k;
    const derived =
      t.final !== false &&
      t.height !== undefined &&
      ((info && t.height <= info.finalized_height) || (t.confirmations !== undefined && k !== undefined && t.confirmations > k));
    if (derived) return { status: "final", detail: `Final at block ${t.height}` };
    return { status: "confirmed", detail: `In block ${t.height}${t.confirmations !== undefined ? `, ${t.confirmations}${k !== undefined ? ` of ${k}` : ""} confirmations` : ""}` };
  }
  if (t.status === "preconfirmed" || pre) {
    const d = pre ? `${pre.weak_count} weak block${pre.weak_count === 1 ? "" : "s"} from ${pre.weak_miners} miner${pre.weak_miners === 1 ? "" : "s"}, ${pre.confidence} confidence. ` : "";
    return { status: "preconfirmed", detail: `${d}${PRECONF_NOTE}` };
  }
  return { status: "pending" };
}

/** Activity items still worth asking the node about (final is the last stage). */
export function needsRefresh(a: ActivityItem, now = Date.now()): boolean {
  if (a.chain !== "pearl") return false;
  if (a.status === "pending" || a.status === "preconfirmed") return true;
  // included items until they are final; finality is about 30 min, stop after 6 h
  return a.status === "confirmed" && now - a.at < 6 * 3600_000;
}
