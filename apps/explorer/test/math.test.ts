import { test } from "node:test";
import assert from "node:assert/strict";
import { capCheck, computeTotals, cumulativeSubsidy, formatUnits, groupFeed, lcStatus, mulPpm, pct } from "../src/math.ts";

test("formatUnits rounds, trims and groups", () => {
  assert.equal(formatUnits(0n, 9), "0");
  assert.equal(formatUnits(1_000_000_000n, 9), "1");
  assert.equal(formatUnits(1_234_567_891_234n, 9, 4), "1,234.5679");
  assert.equal(formatUnits(999_999_999n, 9, 4), "1");
  assert.equal(formatUnits(10_008_389n, 9, 4), "0.01");
  assert.equal(formatUnits(10_008_389n, 9, 6), "0.010008");
  assert.equal(formatUnits(1n, 9, 4), "<0.0001");
  assert.equal(formatUnits(284_879_935_361n, 6, 2), "284,879.94");
  assert.equal(formatUnits("56975987072000", 9, 2), "56,975.99");
});

test("pct never rounds a nonzero share to 0", () => {
  assert.equal(pct(0, 10), "0%");
  assert.equal(pct(1, 23620), "0.004%");
  assert.equal(pct(1, 2), "50%");
  assert.equal(pct(5, 5), "100%");
  assert.equal(pct(999999, 1000000), "99.9999%");
  assert.equal(pct(3, 0), "0%");
});

const job = (o: Partial<Record<string, any>> = {}) => ({
  job_id: "aa",
  poster: "P1",
  status: "completed",
  worker_pay: "227903948289000",
  protocol_fee: "56975987072000",
  subsidy_paid: "45580789657600",
  cap_remaining: "0",
  blocks_mined: 1,
  max_blocks: 5,
  posted_height: 234,
  ...o,
});

test("capCheck: cap = subsidy_paid + cap_remaining = alpha x P at post time", () => {
  // the live VPS job: P 56,975.987072, alpha 0.8, one block took the whole cap
  const c = capCheck(job(), 800_000);
  assert.equal(c.cap, 45_580_789_657_600n);
  assert.equal(c.capAtCurrentAlpha, mulPpm(56_975_987_072_000n, 800_000));
  assert.equal(c.cap, c.capAtCurrentAlpha);
  assert.equal(c.ok, true);
  assert.equal(c.subsidyPpmOfP, 800_000n);
});

test("capCheck flags a subsidy above the cap or a cap above P", () => {
  assert.equal(capCheck(job({ subsidy_paid: "60000000000000", cap_remaining: "0", protocol_fee: "56975987072000" })).ok, false);
  assert.equal(capCheck(job({ subsidy_paid: "0", cap_remaining: "1", protocol_fee: "0" })).ok, false);
  assert.equal(capCheck(job({ subsidy_paid: "10", cap_remaining: "70", protocol_fee: "100" })).ok, true);
});

test("cumulativeSubsidy walks job blocks in height order", () => {
  const r = cumulativeSubsidy(
    [
      { height: 12, subsidy: "30" },
      { height: 10, subsidy: "50" },
      { height: 15, subsidy: "10" },
    ],
    80n,
  );
  assert.deepEqual(r.map((x) => [x.height, x.cumulative, x.withinCap]), [
    [10, 50n, true],
    [12, 80n, true],
    [15, 90n, false],
  ]);
});

test("computeTotals: paid vs fallback, SOL by status, P vs subsidy, unique payers", () => {
  const blocks = [
    { height: 0, jobId: null, fees: "0", subsidy: "0" }, // genesis is not counted
    { height: 1, jobId: null, fees: "1000", subsidy: "0" },
    { height: 2, jobId: "j1", fees: "0", subsidy: "40" },
    { height: 3, jobId: "j1", fees: "1000", subsidy: "40" },
    { height: 4, jobId: "j2", fees: "0", subsidy: "8" },
    { height: 5, jobId: null, fees: "0", subsidy: "0" },
  ];
  const jobs = [
    job({ job_id: "j1", poster: "alice", protocol_fee: "100", worker_pay: "400", subsidy_paid: "80", cap_remaining: "0" }),
    job({ job_id: "j2", poster: "bob", protocol_fee: "10", worker_pay: "40", subsidy_paid: "8", cap_remaining: "0" }),
    job({ job_id: "j3", poster: "alice", protocol_fee: "10", worker_pay: "40", subsidy_paid: "0", cap_remaining: "8", status: "open" }),
  ];
  const orders = [
    { nonce: "1", status: "finalized", buyer: "svc", recipient: "alice", solSpent: "10000000", tokens: "500", protocolFee: "100", workerPay: "400", jobId: "j1" },
    { nonce: "2", status: "pending", buyer: "svc", recipient: "alice", solSpent: "5000000", tokens: "50", protocolFee: "10", workerPay: "40", jobId: "j3" },
    { nonce: "3", status: "refunded", buyer: "svc2", recipient: "carol", solSpent: "7000000", tokens: "70", protocolFee: "14", workerPay: "56", jobId: "jx" },
  ];
  const t = computeTotals(blocks, jobs, orders, new Set(["j1", "j3"]));
  assert.deepEqual(t.blocks, { total: 5, paid: 3, fallback: 2, paidShare: "60%", indexedFrom: 1, indexedTo: 5 });
  assert.equal(t.storefront.solSettled, 10_000_000n);
  assert.equal(t.storefront.solPending, 5_000_000n);
  assert.equal(t.storefront.solRefunded, 7_000_000n);
  assert.equal(t.storefront.tokensBoughtSpl, 550n); // refunded tokens went back
  assert.deepEqual(t.storefront.byStatus, { finalized: 1, pending: 1, refunded: 1 });
  assert.equal(t.fees.protocolFeesPaid, 120n);
  assert.equal(t.fees.subsidyPaid, 88n);
  assert.equal(t.fees.subsidyFromJobs, 88n);
  assert.equal(t.fees.blockFees, 2000n);
  assert.equal(t.fees.subsidyPpmOfFees, 733_333n);
  assert.equal(t.fees.jobsOverCap, 0);
  assert.equal(t.payers.unique, 2); // alice, bob (carol's order never became a job)
  assert.equal(t.payers.uniqueSolanaBuyers, 1); // svc2 only bought a refunded order
  assert.equal(t.payers.viaStorefront, 1);
  assert.equal(t.payers.topPayer, "alice");
  assert.equal(t.payers.topPayerShare, "91.7%");
  assert.equal(t.payers.top10Share, "100%");
  assert.deepEqual(t.jobs, { total: 3, byStatus: { completed: 2, open: 1 }, storefront: 2, direct: 1 });
});

test("groupFeed collapses runs of fallback blocks, never paid ones", () => {
  const b = (height: number, jobId: string | null) => ({ height, jobId, fees: "1", subsidy: jobId ? "5" : "0" });
  const rows = groupFeed([b(10, null), b(9, null), b(8, null), b(7, "j"), b(6, null), b(5, "j"), b(4, null), b(3, null)], 2);
  assert.deepEqual(
    rows.map((r) => (r.kind === "block" ? `b${r.block.height}` : `run${r.to}-${r.from}x${r.count}:${r.fees}`)),
    ["run10-8x3:3", "b7", "b6", "b5", "run4-3x2:2"],
  );
  // a gap in heights (not yet indexed) splits a run
  const gap = groupFeed([b(10, null), b(9, null), b(5, null), b(4, null)], 2);
  assert.equal(gap.length, 2);
});

test("lcStatus: record, accumulator range, checkpoint, not relayed", () => {
  const lc = { checkpointHeight: 226, tipHeight: 12354, finalHeight: 12347 };
  const acc = { startHeight: 226, count: 12122 }; // final headers 226..12347
  assert.equal(lcStatus(100, true, null, null, null).state, "none");
  assert.equal(lcStatus(100, true, lc, acc, null).state, "before-checkpoint");
  assert.equal(lcStatus(236, true, lc, acc, null).state, "accumulated");
  assert.equal(lcStatus(12347, true, lc, acc, null).state, "accumulated");
  assert.equal(lcStatus(12350, false, lc, acc, null).state, "no-record");
  assert.equal(lcStatus(12350, false, lc, acc, { verified: true, final: false }).state, "verified");
  assert.equal(lcStatus(12350, false, lc, acc, { verified: false, final: false }).state, "pending");
  assert.equal(lcStatus(12340, true, lc, acc, { verified: true, final: true }).state, "final");
  const s = lcStatus(23620, false, lc, acc, null);
  assert.equal(s.state, "not-relayed");
  assert.equal((s as any).behind, 23620 - 12354);
  // a non-final block in the accumulator range is not claimed as accumulated
  assert.equal(lcStatus(236, false, lc, acc, null).state, "no-record");
});
