// Activity status from the node's tx answer (docs/SPEED.md 2.4/2.5).
import { describe, expect, it } from "vitest";
import { activityStatus, confidenceOf, needsRefresh, PRECONF_NOTE } from "../src/chain/txstatus";
import type { TxInfo } from "../src/chain/rpc";
import type { ActivityItem } from "../src/shared/types";

const base: TxInfo = { tx_hash: "ab".repeat(32), status: "pending", kind: "Transfer", from: "x", nonce: 0, fee: "1000" };
const info = { finalized_height: 70, finality_k: 30 };

describe("activity status", () => {
  it("pending, preconfirmed with the risk note, included, final", () => {
    expect(activityStatus(base, info)).toEqual({ status: "pending" });
    const p = activityStatus({ ...base, status: "preconfirmed", preconf: { weak_count: 2, weak_miners: 2, weak_share: 1, confidence: "medium" } }, info)!;
    expect(p.status).toBe("preconfirmed");
    expect(p.detail).toContain("2 weak blocks from 2 miners, medium confidence");
    expect(p.detail).toContain(PRECONF_NOTE);
    expect(activityStatus({ ...base, status: "included", height: 90, confirmations: 11 }, info)).toEqual({ status: "confirmed", detail: "In block 90, 11 of 30 confirmations" });
    expect(activityStatus({ ...base, status: "final", height: 60, final: true }, info)!.status).toBe("final");
    expect(activityStatus(null, info)).toBeNull();
  });
  it("an older node: final derived from the finalized height; the node's final: false wins", () => {
    expect(activityStatus({ ...base, status: "included", height: 60, confirmations: 41 }, info)!.status).toBe("final");
    expect(activityStatus({ ...base, status: "included", height: 60, confirmations: 41, final: false }, info)!.status).toBe("confirmed");
    expect(activityStatus({ ...base, status: "included", height: 60, confirmations: 31 }, { finalized_height: 0, finality_k: 30 })!.status).toBe("final");
  });
  it("confidence from counts and which items still refresh", () => {
    expect(confidenceOf(1, 1, 1)).toBe("low");
    expect(confidenceOf(4, 2, 0.9)).toBe("high");
    const a = (status: ActivityItem["status"], at = Date.now()): ActivityItem => ({ id: "1", account: "a", network: "n", chain: "pearl", kind: "send", title: "t", ref: "r", status, at });
    expect(needsRefresh(a("pending"))).toBe(true);
    expect(needsRefresh(a("preconfirmed"))).toBe(true);
    expect(needsRefresh(a("confirmed"))).toBe(true);
    expect(needsRefresh(a("confirmed", Date.now() - 7 * 3600_000))).toBe(false);
    expect(needsRefresh(a("final"))).toBe(false);
    expect(needsRefresh({ ...a("pending"), chain: "solana" })).toBe(false);
  });
  it("copy has no em dash", () => {
    expect(PRECONF_NOTE.includes(String.fromCharCode(0x2014))).toBe(false);
  });
});
