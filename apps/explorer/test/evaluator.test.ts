// Evaluator job index (evaluator.ts): jobs naming an account as evaluator, per-status counts,
// `rejected` from indexed RejectResults, and the /api/jobs?evaluator= route.
import { test } from "node:test";
import assert from "node:assert/strict";
import { b58encode } from "../src/codec.ts";
import { evaluatorJobsModel, evaluatorIndex } from "../src/evaluator.ts";
import type { BlockRec, TxRec } from "../src/indexer.ts";

const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, k) => (k === 0 ? i : 7)));
const [EV1, EV2, BUYER, SELLER, MINER] = [1, 2, 3, 4, 9].map(key);
const jid = (c: string) => c.repeat(64).slice(0, 64);
const job = (id: string, o: any) => ({ job_id: id, kind: "job", poster: BUYER, deliverer: null, status: "open", worker_pay: "5000", protocol_fee: "1000", evaluators: [EV1], threshold: 1, posted_height: 10, index: 0, closed_height: 0, deadline_height: 500, result_hash: null, result_uri: "", ...o });
const tx = (kind: string, from: string, facts: any): TxRec => ({ hash: "h", kind, from, fee: "10", facts });
const block = (height: number, txs: TxRec[]): BlockRec => ({ height, hash: `b${height}`, prev: "", time: 1_700_000_000_000 + height, miner: MINER, jobId: null, fees: "0", subsidy: "0", reward: "0", proofHash: "", proofLen: 0, targetBits: 1, txs } as any);

function fakeIndex() {
  const jobs = [
    job(jid("a"), { status: "delivered", deliverer: SELLER, posted_height: 30 }),
    job(jid("b"), { status: "completed", deliverer: SELLER, evaluators: [EV1, EV2], threshold: 2, posted_height: 20 }),
    job(jid("c"), { status: "open", posted_height: 15 }), // rejected once, reopened
    job(jid("d"), { status: "expired", evaluators: [EV2], posted_height: 12 }),
    job(jid("e"), { status: "open", evaluators: [EV1, EV1], posted_height: 40 }), // duplicate key listed once
  ];
  const blocks = new Map<number, BlockRec>();
  blocks.set(16, block(16, [tx("SubmitResult", SELLER, { jobId: jid("c") })]));
  blocks.set(17, block(17, [tx("RejectResult", BUYER, { jobId: jid("c"), deliverer: SELLER, resultHash: "ab", signers: [EV1] })]));
  return { jobs: new Map(jobs.map((j) => [j.job_id, j])), blocks, orders: new Map(), chainInfo: { height: 50 }, status: { polls: 1 } } as any;
}

test("evaluator index: jobs naming the account, newest first, counts and statuses", () => {
  const ix = fakeIndex();
  const all = evaluatorJobsModel(ix, EV1);
  assert.deepEqual(all.jobs.map((j) => j.job_id), [jid("e"), jid("a"), jid("b"), jid("c")]);
  assert.deepEqual(all.counts, { all: 4, open: 2, delivered: 1, completed: 1, expired: 0, rejected: 1 });
  const del = evaluatorJobsModel(ix, EV1, "delivered");
  assert.deepEqual(del.jobs.map((j) => j.job_id), [jid("a")]);
  assert.equal(del.jobs[0].deliverer, SELLER);
  assert.deepEqual(del.jobs[0].evaluators, [EV1]);
  const rej = evaluatorJobsModel(ix, EV1, "rejected");
  assert.deepEqual(rej.jobs.map((j) => j.job_id), [jid("c")]);
  assert.equal(rej.jobs[0].status, "open");
  assert.equal(rej.jobs[0].rejects_indexed, 1);
  assert.deepEqual(rej.jobs[0].last_reject, { height: 17, deliverer: SELLER, result_hash: "ab", signers: [EV1] });
  assert.deepEqual(evaluatorJobsModel(ix, EV2).jobs.map((j) => j.job_id), [jid("b"), jid("d")]);
  assert.equal(evaluatorJobsModel(ix, EV2, "expired").counts.expired, 1);
  assert.equal(evaluatorJobsModel(ix, SELLER).jobs.length, 0, "deliverers are not evaluators");
});

test("evaluator index: cached until the jobs, blocks or poll count change", () => {
  const ix = fakeIndex();
  const a = evaluatorIndex(ix);
  assert.equal(evaluatorIndex(ix), a);
  ix.jobs.set(jid("f"), job(jid("f"), { posted_height: 45 }));
  const b = evaluatorIndex(ix);
  assert.notEqual(b, a);
  assert.equal(evaluatorJobsModel(ix, EV1).jobs[0].job_id, jid("f"));
  ix.status.polls++;
  assert.notEqual(evaluatorIndex(ix), b);
});
