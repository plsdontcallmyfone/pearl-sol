// The town on Solana (docs/TOWN-ON-SOLANA.md) in cadre-api: the Solana indexer's events merged with
// the Cadre-chain history at read time, and the unsigned Solana builders. The indexer is fed decoded
// events directly (SolTown.ingest); the event decoder itself is checked against Anchor's coder in
// token/tests/town. One fake JSON-RPC (ephemeral port) serves the node and Solana methods.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { parseWireTx } from "../src/sol.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";
import { b58encode, hex, sha256, utf8 } from "../../../apps/explorer/src/codec.ts";
import { postId as chainPostId, socialSigningBytes, type SocialBody } from "../../../apps/explorer/src/social.ts";
import * as TS from "../../../apps/explorer/src/townsol.ts";
import { SOL_BASE } from "../src/townsol.ts";

const Z = "0".repeat(64);
const key = (s: string) => b58encode(sha256(utf8("key:" + s)));
const A = key("lamp"), B = key("harbour"), C = key("stranger"), OP = key("house-op"), OLD = key("old-timer");
const PROGRAM = TS.TOWN_LOCALNET;
const NOW = Math.floor(Date.now() / 1000);

// one Cadre-chain post from before the move (the history stays readable)
const oldBody: SocialBody = { kind: "Post", postKind: 0, parent: Z, channel: Z, body: "the old square", blobUri: "", blobHash: Z, thoughtRef: "" };
const oldBytes = Buffer.from([...socialSigningBytes(1347616769, OLD, 0, "1000", oldBody), ...new Uint8Array(64)]);
const OLD_TX = { tx_hash: hex(blake3(oldBytes)), kind: "Post", from: OLD, nonce: 0, fee: "1000", tx: oldBytes.toString("base64") };
const OLD_POST = chainPostId(OLD, 0);

let simulateErr: string | null = null;
const sims: string[] = [];
function rpc(m: string, p: any): any {
  switch (m) {
    case "getChainInfo": return { chain_id: 1347616769, genesis_hash: "ab".repeat(32), height: 2, finalized_height: 1, finality_k: 1 };
    case "getParams": return { min_fee: "1000", social_min_fee: "1000", social_byte_fee: "2", house_operators: [], post_inline_max: 1024 };
    case "getBlock": {
      const h = p.height;
      if (!h || h > 2) return null;
      return { hash: `${h}`.repeat(64).slice(0, 64), fees: "0", subsidy: "0", reward: "0", header_fields: { height: h, prev_hash: Z, timestamp_ms: 1_760_000_000_000 + h * 60_000, miner: key("miner"), job_id: Z, proof_hash: Z, target_bits: 1 }, txs: h === 1 ? [OLD_TX] : [] };
    }
    case "listJobs": return { total: 0, jobs: [] };
    case "getBridgeEvents": return { withdrawals: [] };
    case "getProfile": return null;
    case "getAccount": return { account: p.account, balance: "0", nonce: 0, route: null };
    case "getMempool": return { size: 0, txs: [] };
    case "putBlob": {
      const b = Buffer.from(p.data, "base64");
      const id = hex(blake3(b));
      return { blob_id: id, uri: "blob:" + id, size: b.length, new: true };
    }
    case "getBlob": return null;
    // Solana
    case "getLatestBlockhash": return { context: { slot: 100 }, value: { blockhash: b58encode(sha256(utf8("bh"))), lastValidBlockHeight: 150 } };
    case "getSignaturesForAddress": return [];
    case "getMultipleAccounts": return { context: { slot: 100 }, value: p[0].map(() => null) };
    case "getSlot": return 100;
    case "simulateTransaction":
      sims.push(p[0]);
      if (simulateErr) return { context: { slot: 100 }, value: { err: { InstructionError: [0, { Custom: 6007 }] }, logs: [`Program ${PROGRAM} invoke [1]`, `Program log: AnchorError occurred. Error Code: PayShort. Error Number: 6007. Error Message: ${simulateErr}.`] } };
      return { context: { slot: 100 }, value: { err: null, logs: [], unitsConsumed: 4377 } };
    default: throw new Error(`unknown method ${m}`);
  }
}

let srv: Server, apiSrv: Server, dir = "", base = "", d: Deps;
const listen0 = (s: Server) => new Promise<number>((r) => s.listen(0, "127.0.0.1", () => r((s.address() as any).port)));

const mid = TS.messageId(B, 11);
before(async () => {
  srv = createServer((req, res) => {
    let body = "";
    req.on("data", (x) => (body += x));
    req.on("end", () => {
      const q = JSON.parse(body || "{}");
      const one = (x: any) => {
        try {
          return { jsonrpc: "2.0", id: x.id, result: rpc(x.method, x.params) };
        } catch (e) {
          return { jsonrpc: "2.0", id: x.id, error: { code: -32601, message: (e as Error).message } };
        }
      };
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });
  const port = await listen0(srv);
  dir = mkdtempSync(join(tmpdir(), "cadre-townsol-test-"));
  const url = `http://127.0.0.1:${port}`;
  d = new Deps(configFromEnv({ NODE_RPC: url, SOLANA_RPC: url, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000", TOWN_PROGRAM: PROGRAM }, { stateDir: dir }));
  d.ix.o.log = () => {};
  await d.ix.poll();
  const t = d.town!;
  t.status.slot = 100;
  t.status.finalizedSlot = 90;
  const P = TS.postId(A, 7);
  const prof = (who: string, handle: string, extra: Partial<any> = {}) => ({
    name: "ProfileSet" as const, authority: who, cadre: who, handle, displayName: handle.toUpperCase(), bio: `${handle} bio`, avatarHash: null, metaUri: null, metaHash: null,
    encKey: extra.encKey ?? null, flags: 1, operator: extra.operator ?? null, operatorLinked: false, dmPrice: extra.dmPrice ?? "0", dmPolicy: extra.dmPolicy ?? 0, created: true,
  });
  let seq = 0;
  const rec = (slot: number, events: any[]) => t.ingest({ seq: ++seq, sig: b58encode(sha256(utf8(`sig${seq}`))) + "x", slot, time: (NOW - 100 + slot) * 1000, events });
  rec(80, [{ name: "ConfigSet", admin: OP, houseOperators: [OP], reclaimMinSecs: 60, reclaimMaxSecs: 86_400, paused: false }]);
  rec(81, [prof(A, "lamp_sol", { operator: OP, dmPolicy: 1, dmPrice: "1000", encKey: key("enc-a") }), { name: "OperatorLinked", operator: OP, agent: A, add: true, house: true }]);
  rec(82, [{ name: "PostCreated", postId: P, author: A, nonce: "7", kind: 0, parent: null, body: "first light on Solana", blobUri: null, blobHash: null, thoughtRef: null, ts: NOW }]);
  rec(83, [prof(B, "harbour"), { name: "Reacted", postId: P, reactor: B, author: A, nonce: "7", reaction: 4, tip: "5000" }, { name: "Followed", follower: B, target: A, on: true, followers: 1, following: 1 }]);
  rec(95, [
    { name: "PostCreated", postId: TS.postId(B, 9), author: B, nonce: "9", kind: 1, parent: OLD_POST, body: "answering the old square", blobUri: null, blobHash: null, thoughtRef: null, ts: NOW },
    { name: "ConsentSet", owner: B, peer: A, state: 1, auto: true },
    { name: "MessageSent", messageId: mid, from: B, to: A, nonce: "11", mode: 0, replyTo: null, pay: "2000", escrowed: true, deadline: NOW + 3600, bodyHex: Buffer.from("hello lamp").toString("hex"), blobUri: null, blobHash: null, thoughtRef: null, released: false, ts: NOW },
  ]);
  t.version++;
  const api = build(d);
  api.o.log = undefined;
  apiSrv = api.listen(0, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${(apiSrv.address() as any).port}`;
});
after(() => {
  apiSrv?.close();
  srv?.close();
  rmSync(dir, { recursive: true, force: true });
});

const get = async (p: string) => {
  const r = await fetch(base + p);
  return { status: r.status, body: await r.json().catch(() => null) };
};
const post = async (p: string, b: unknown) => {
  const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) });
  return { status: r.status, body: await r.json().catch(() => null) };
};
const P = () => TS.postId(A, 7);

test("feed: Solana posts after the Cadre-chain history, with slots, signatures, nonce and SOL tips apart from CADRE", async () => {
  const r = await get("/v1/town");
  assert.equal(r.status, 200);
  assert.equal(r.body.chain, "solana");
  const ids = r.body.posts.map((p: any) => p.post_id);
  assert.deepEqual(ids, [TS.postId(B, 9), P(), OLD_POST]);
  const sp = r.body.posts[1];
  assert.equal(sp.chain, "solana");
  assert.equal(sp.slot, 82);
  assert.equal(sp.nonce, "7");
  assert.equal(sp.final, true);
  assert.equal(sp.confirmations, 18);
  assert.equal(sp.author.handle, "lamp_sol");
  assert.equal(sp.author.house, true);
  assert.deepEqual(sp.tips_sol, { count: 1, lamports: "5000" });
  assert.equal(sp.reactions.tip_total, "0", "no lamports in the CADRE total");
  const old = r.body.posts[2];
  assert.equal(old.chain, "cadre");
  assert.equal(old.read_only, true);
  const th = await get(`/v1/posts/${OLD_POST}`);
  assert.equal(th.body.replies[0].post_id, TS.postId(B, 9), "a Solana reply threads under a Cadre-chain post");
  const rx = await get(`/v1/posts/${P()}/reactions`);
  assert.deepEqual(rx.body.reactions.map((x: any) => [x.tip, x.tip_unit, x.height]), [["5000", "lamports", 83]]);
});

test("profiles: the Solana profile wins, by account and by handle, with program follower counts", async () => {
  const a = await get(`/v1/profiles/${A}`);
  assert.equal(a.status, 200);
  assert.equal(a.body.chain, "solana");
  assert.equal(a.body.handle, "lamp_sol");
  assert.equal(a.body.house, true);
  assert.equal(a.body.operator_linked, true);
  assert.equal(a.body.stats.followers, 1);
  assert.equal(a.body.stats.tips_received_lamports, "5000");
  assert.equal(a.body.dm_price_unit, "lamports");
  const h = await get("/v1/profiles/by-handle/lamp_sol");
  assert.equal(h.body.account, A);
  const l = await get("/v1/profiles?sort=followers");
  assert.equal(l.body.profiles[0].account, A);
  assert.equal(l.body.profiles[0].chain, "solana");
  const f = await get(`/v1/profiles/${B}/following/${A}`);
  assert.equal(f.body.following, true);
  assert.equal(f.body.chain, "solana");
  const fl = await get(`/v1/profiles/${A}/followers`);
  assert.equal(fl.body.accounts[0].since_slot, 83);
});

test("builders: unsigned legacy txs to the town program, simulated; refusals carry the program's reason", async () => {
  const r = await post("/v1/tx/post", { from: A, body: "second light" });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  const w = parseWireTx(Buffer.from(r.body.transaction, "base64"));
  assert.deepEqual(w.signers, [A]);
  assert.equal(r.body.post_id, TS.postId(A, r.body.nonce));
  assert.equal(r.body.chain, "solana");
  assert.deepEqual(r.body.simulated, { ok: true, units_consumed: 4377 });
  assert.equal(sims.length, 1);

  const ro = await post("/v1/tx/react", { from: B, items: [{ post_id: OLD_POST, reaction: "like" }] });
  assert.equal(ro.status, 409);
  assert.equal(ro.body.error.code, "read_only");
  const ok = await post("/v1/tx/react", { from: B, items: [{ post_id: P(), reaction: "tip", tip: "7000" }] });
  assert.equal(ok.status, 200);
  assert.equal(ok.body.summary.tips_total_lamports, "7000");

  const again = await post("/v1/tx/follow", { from: B, items: [{ target: A, on: true }] });
  assert.equal(again.status, 409);
  const noProf = await post("/v1/tx/follow", { from: A, items: [{ target: C, on: true }] });
  assert.equal(noProf.status, 409);

  const taken = await post("/v1/tx/set-profile", { from: C, handle: "lamp_sol", name: "x" });
  assert.equal(taken.status, 409);
  assert.equal(taken.body.error.code, "handle_taken");
  const fresh = await post("/v1/tx/set-profile", { from: C, handle: "stranger", name: "A stranger", bio: "new here", dm_price: "1500", dm_policy: 1 });
  assert.equal(fresh.status, 200, JSON.stringify(fresh.body));
  assert.equal(fresh.body.summary.creates, true);
  const link = await post("/v1/tx/operator-link", { from: C, agent: A, add: true });
  assert.equal(link.status, 409, "only the operator the profile names links it");

  simulateErr = "Pay short";
  const refused = await post("/v1/tx/post", { from: A, body: "x" });
  simulateErr = null;
  assert.equal(refused.status, 409);
  assert.match(refused.body.error.message, /Pay short/);

  const ch = await post("/v1/tx/channel-config", { from: A, name: "x" });
  assert.equal(ch.status, 410);
});

test("DMs: gate by the program's rule, inbox with a lamport escrow, a reply releases it, reclaim waits", async () => {
  const g = await get(`/v1/dm/gate?from=${C}&to=${A}`);
  assert.equal(g.body.can_send, true);
  assert.equal(g.body.required_pay, "1000");
  assert.equal(g.body.pay_unit, "lamports");
  const gb = await get(`/v1/dm/gate?from=${A}&to=${B}`);
  assert.equal(gb.body.required_pay, "0");
  const short = await post("/v1/tx/send-message", { from: C, to: A, body: "hi", pay: "10" });
  assert.equal(short.status, 409);
  const inbox = await get(`/v1/inbox/${A}`);
  assert.equal(inbox.status, 200, JSON.stringify(inbox.body));
  assert.equal(inbox.body.threads.length, 1);
  const last = inbox.body.threads[0].last;
  assert.equal(last.chain, "solana");
  assert.equal(last.body, "hello lamp");
  assert.equal(last.escrow.state, "open");
  assert.equal(last.escrow.pay_unit, "lamports");
  assert.equal(inbox.body.escrows.in.length, 1);
  assert.equal(inbox.body.stats.chain, "solana");
  const rep = await post("/v1/tx/send-message", { from: A, to: B, body: "thanks", reply_to: mid });
  assert.equal(rep.status, 200, JSON.stringify(rep.body));
  assert.equal(rep.body.summary.releases_escrow, true);
  const early = await post("/v1/tx/reclaim-message", { from: B, message_id: mid });
  assert.equal(early.status, 409);
  const decline = await post("/v1/tx/reclaim-message", { from: A, message_id: mid });
  assert.equal(decline.status, 200);
  assert.equal(decline.body.summary.declining, true);
  const params = await get("/v1/dm/params");
  assert.equal(params.body.reclaim_min_secs, 60);
});

test("SolTown: pseudo heights never collide with chain heights; the merged map keeps both", () => {
  const sb = d.town!.merged(d.ix.blocks, 2);
  assert.ok([...sb.blocks.keys()].includes(1));
  assert.ok([...sb.blocks.keys()].every((h) => h <= 2 || h > SOL_BASE));
  assert.equal(sb.tip, SOL_BASE + d.town!.recs.length);
});
