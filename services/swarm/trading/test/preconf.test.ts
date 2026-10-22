// Pre-confirmation risk gate (src/preconf.ts, pearl-sol docs/SPEED.md 2.5).
import { test } from "node:test";
import assert from "node:assert/strict";
import { policyFromEnv, readStatus, required, satisfies, statedRisk, waitSettled } from "../src/preconf.ts";
import { normalizeTxStatus } from "../../../../apps/explorer/src/txstatus.ts";

const U = 1_000_000_000n;
const H = "ab".repeat(32);
const pol = { preconfCap: 10n * U, includedCap: 1000n * U, minConfidence: "medium" as const };
const pre = (confidence: string, n = 3) => normalizeTxStatus({ status: "preconfirmed", preconf: { weak_count: n, weak_miners: 2, weak_share: 1, latest_age_ms: 100, confidence } })!;

test("policy: defaults, env, and the needed stage by notional", () => {
  assert.deepEqual(policyFromEnv({}), pol);
  const e = policyFromEnv({ TRADE_PRECONF_CAP: "0", TRADE_INCLUDED_CAP: "5", TRADE_PRECONF_MIN_CONFIDENCE: "high" });
  assert.deepEqual(e, { preconfCap: 0n, includedCap: 5n, minConfidence: "high" });
  assert.equal(required(10n * U, pol), "preconfirmed");
  assert.equal(required(10n * U + 1n, pol), "included");
  assert.equal(required(1001n * U, pol), "final");
  assert.equal(required(1n, e), "included", "cap 0 turns preconfirmed acting off");
});

test("satisfies: confidence floor for preconfirmed, ranks for the rest", () => {
  assert.equal(satisfies(pre("low", 1), "preconfirmed", pol), false);
  assert.equal(satisfies(pre("medium"), "preconfirmed", pol), true);
  assert.equal(satisfies(pre("medium"), "preconfirmed", { ...pol, minConfidence: "high" }), false);
  assert.equal(satisfies(pre("high"), "included", pol), false);
  const inc = normalizeTxStatus({ status: "included", height: 5, confirmations: 2, finality_k: 30 })!;
  assert.equal(satisfies(inc, "preconfirmed", pol), true);
  assert.equal(satisfies(inc, "final", pol), false);
  assert.match(statedRisk(pre("medium"), "preconfirmed"), /not binding/);
  assert.match(statedRisk(inc, "included"), /2 of 30 confirmations/);
  assert.ok(!statedRisk(inc, "included").includes(String.fromCharCode(0x2014)));
});

function scripted(steps: any[], old = false) {
  let i = 0;
  return {
    calls: [] as string[],
    async call(m: string) {
      this.calls.push(m);
      if (m === "getChainInfo") return { height: 100, finalized_height: 70, finality_k: 30 };
      if (m === "getTxStatus" && old) throw new Error("unknown method getTxStatus");
      return steps[Math.min(i++, steps.length - 1)];
    },
  };
}

test("waitSettled: a small trade acts on medium preconfirmed, a large one waits for final", async () => {
  const steps = [{ status: "pending" }, { status: "preconfirmed", preconf: { weak_count: 1, weak_miners: 1, weak_share: 1, confidence: "low" } }, { status: "preconfirmed", preconf: { weak_count: 3, weak_miners: 2, weak_share: 1, confidence: "medium" } }, { status: "included", height: 99, confirmations: 2, final: false }, { status: "final", height: 99, final: true }];
  const small = await waitSettled(scripted(steps), H, 5n * U, pol, { pollMs: 1 });
  assert.equal(small.status, "preconfirmed");
  assert.equal(small.tx_status.preconf!.confidence, "medium");
  assert.match(small.risk, /preconfirmed/);
  const mid = await waitSettled(scripted(steps), H, 50n * U, pol, { pollMs: 1 });
  assert.equal(mid.status, "included");
  const big = await waitSettled(scripted(steps), H, 5000n * U, pol, { pollMs: 1 });
  assert.equal(big.status, "final");
  await assert.rejects(waitSettled(scripted([{ status: "pending" }]), H, 1n, pol, { pollMs: 1, timeoutMs: 20 }), /still pending/);
});

test("an older node: getTx fallback, final derived from the finalized height", async () => {
  const r = scripted([{ status: "included", height: 60, confirmations: 41 }], true);
  const s = await readStatus(r, H);
  assert.equal(s!.status, "final");
  assert.ok(r.calls.includes("getTx") && r.calls.includes("getChainInfo"));
});
