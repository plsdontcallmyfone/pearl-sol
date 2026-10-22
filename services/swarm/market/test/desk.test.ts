// House market desk (docs/MARKET.md 6): hires stay inside each buyer's daily cap and name only
// the judge; requests are built from chain numbers; filtered output is never delivered; a
// delivery carries the paid thought's request id as thought_ref.
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { walletFromSeed } from "../../src/wallet.ts";
import { Desk, CADRE } from "../src/desk.ts";
import { PERSONAS } from "../src/personas.ts";
import { R, type Offer, type Hire } from "../../../../apps/explorer/src/market.ts";

const JUDGE = walletFromSeed(new Uint8Array(32).fill(9)).address;
const W = (i: number) => walletFromSeed(new Uint8Array(32).fill(i));

function fakeChain() {
  const sent: { args: Uint8Array; value: bigint; from: string }[] = [];
  const blobs = new Map<string, Buffer>();
  let n = 0;
  const c: any = {
    rpc: {
      call: async (m: string, p: any) => {
        if (m === "getChainInfo") return { height: 100 };
        if (m === "getBlock") return { txs: [1, 2, 3], fees: "3000", contracts: p.height % 2 ? { receipt_count: 1 } : null };
        if (m === "getBlob") { const b = blobs.get(p.blob_id); return b ? { data: b.toString("base64") } : null; }
        throw new Error(`unexpected ${m}`);
      },
      putBlob: async (d: Uint8Array) => { const id = (++n).toString(16).padStart(64, "0"); blobs.set(`blob:${id}`, Buffer.from(d)); return { uri: `blob:${id}`, blob_id: id }; },
    },
    sendContract: async (w: any, b: any) => { sent.push({ args: b.args, value: b.value, from: w.address }); return { txHash: `tx${sent.length}` }; },
    waitIncluded: async () => ({ height: 101, receipt: { ok: true, return: Buffer.alloc(8).toString("hex") } }),
    sendNative: async () => ({ txHash: "post" }),
  };
  return { c, sent, blobs };
}

const method = (a: Uint8Array) => Buffer.from(a.subarray(1, 1 + a[0])).toString();

function desk(dailyCap: bigint, brainText = "A clear explainer.") {
  const { c, sent, blobs } = fakeChain();
  const dir = mkdtempSync(join(tmpdir(), "desk-"));
  const agents = PERSONAS.slice(0, 3).map((p, i) => ({ persona: p, wallet: W(i + 1), brain: { think: async () => ({ text: brainText, requestId: "req-42", rows: 10, ttftMs: 1, totalMs: 2, textSha256: "x", charged: null }) } }));
  const d = new Desk(c, agents, { market: JUDGE, judge: JUDGE, judgeFee: CADRE / 2n, dailyCapCadre: dailyCap, hireEverySec: 0, stateFile: join(dir, "s.json"), postReceipts: false, now: () => 1_792_600_000_000 }, () => {});
  const offers: Offer[] = agents.map((a, i) => ({ id: i, provider: a.wallet.address, title: a.persona.offers[0].title, category: a.persona.offers[0].category, spec_uri: "", spec_hash: "00", price: (BigInt(a.persona.offers[0].priceCadre) * CADRE).toString(), turnaround: 20, policy: "buyer_names", evaluators: [], active: true, created: 1, updated: 1, hires: 0, completed: 0 }));
  d.ix.offers = offers;
  d.ix.quote = async (id: number, jf: bigint) => { const p = BigInt(offers[id].price); const fee = (p * 2000n + 9999n) / 10000n; return { price: p.toString(), fee: fee.toString(), judge_fee: jf.toString(), total: (p + fee + jf).toString() }; };
  return { d, sent, blobs, agents, offers, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

test("a hire names only the judge, pays exactly the quote, and the daily cap holds", async () => {
  const t = desk(10n * CADRE);
  const id = await t.d.hireOne();
  assert.equal(id, 0);
  assert.equal(t.sent.length, 1);
  assert.equal(method(t.sent[0].args), "hire");
  const r = new R(t.sent[0].args);
  r.o = 5;
  const offer = Number(r.u64()), price = r.u64();
  r.str(); r.hash();
  r.u64(); const jf = r.u64(); const ev = r.evals(); const th = r.u8();
  assert.notEqual(t.offers[offer].provider, t.sent[0].from, "never its own offer");
  assert.deepEqual(ev, [JUDGE]);
  assert.equal(th, 1);
  assert.equal(t.sent[0].value, price + (price * 2000n + 9999n) / 10000n + jf);
  // the buyer's cap: the same buyer would pass 10 CADRE on a second hire, so the next buyer goes
  t.d.st.nextBuyer = 0;
  await t.d.hireOne();
  assert.notEqual(t.sent[1]?.from, t.sent[0].from);
  t.cleanup();
});

test("a research request is chain numbers only", async () => {
  const t = desk(100n * CADRE);
  const txt = await t.d.request("research", t.offers[1]);
  assert.match(txt, /^Blocks 81 to 100 on Cadre devnet: 60 transactions, of which 10 contract calls; 60000 base units of fees/);
  t.cleanup();
});

test("filtered output is not delivered; clean output is, with the thought_ref", async () => {
  const leak = desk(100n * CADRE, "here is a key " + "a".repeat(64));
  const h: Hire = { id: 3, offer: 0, buyer: leak.agents[1].wallet.address, provider: leak.agents[0].wallet.address, price: "1", fee: "0", judge_fee: "0", total: "1", created: 1, deadline: 200, request_uri: "", request_hash: "", evaluators: [JUDGE], threshold: 1, status: "open", accepts: 0, rejects: 0, voted: [], accepted_by: [], result_hash: null, result_uri: null, delivered: null, closed: null };
  const req = await (leak.d as any).blob({ request: "explain escrow" });
  h.request_uri = req.uri;
  await leak.d.work(h);
  assert.equal(leak.sent.length, 0, "nothing delivered");
  assert.equal(leak.d.st.failed[3], 1);
  leak.cleanup();
  const ok = desk(100n * CADRE);
  const req2 = await (ok.d as any).blob({ request: "explain escrow" });
  await ok.d.work({ ...h, request_uri: req2.uri });
  assert.equal(ok.sent.length, 1);
  assert.equal(method(ok.sent[0].args), "deliver");
  assert.ok(Buffer.from(ok.sent[0].args).includes(Buffer.from("req-42")), "thought_ref in the call");
  const stored = [...ok.blobs.values()].map((b) => JSON.parse(b.toString())).find((j) => j.format === "cadre-delivery-v1");
  assert.equal(stored.thought_ref, "req-42");
  assert.equal(stored.house, true);
  ok.cleanup();
});
