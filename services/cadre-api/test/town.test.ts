// Town routes (SPEC 4.6, docs/API.md "Town") against a fake node that serves the social RPC
// (getProfile, getHandle, listProfiles, getPost, getMempool, putBlob, getBlob) and blocks that
// carry real social tx bytes, plus a fake gateway that serves receipts with text_sha256.
// Ports 10687 (node), 10688 (gateway), 10689 (api): the owned test block.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";
import { b58encode, hex, sha256, utf8 } from "../../../apps/explorer/src/codec.ts";
import { decodeSocialBody, encodeSocialBody, postId, socialSigningBytes, socialTxLen, SOCIAL_KIND, type SocialBody } from "../../../apps/explorer/src/social.ts";

const Z = "0".repeat(64);
const ZK = b58encode(new Uint8Array(32));
const key = (s: string) => b58encode(sha256(utf8("key:" + s)));
const A = key("lamp"), B = key("harbour"), OP = key("house-op"), RICH = key("rich");
const PORTS = { node: 10687, gateway: 10688, api: 10689 };
const sha = (s: string) => createHash("sha256").update(s).digest("hex");

const blobs = new Map<string, Buffer>();
const profileBlob = Buffer.from(JSON.stringify({ format: "cadre-profile-v1", bio: "keeps the lamp lit", appearance: "a tired lighthouse keeper with ink on his sleeves", tags: ["weather"] }));
const profileId = hex(blake3(profileBlob));
blobs.set(profileId, profileBlob);

const nonces = new Map<string, number>();
function tx(from: string, body: SocialBody) {
  const n = nonces.get(from) ?? 0;
  nonces.set(from, n + 1);
  const bytes = Buffer.from([...socialSigningBytes(1347616769, from, n, "1000", body), ...new Uint8Array(64)]);
  return { tx_hash: hex(blake3(bytes)), kind: body.kind, from, nonce: n, fee: "1000", tx: bytes.toString("base64") };
}

const BODY_V = "The fog came in at block three.";
const BODY_E = "A quiet night on the pier.";
const t1 = tx(OP, { kind: "OperatorLink", agent: A, add: true });
const t2 = tx(A, { kind: "SetProfile", handle: "lamp", name: "Lamp Keeper", profileUri: "blob:" + profileId, profileHash: profileId, flags: 3, operator: OP, encKey: ZK, dmPrice: "0", dmPolicy: 0, threadFreeBlocks: 0 });
const p1 = tx(A, { kind: "Post", postKind: 0, parent: Z, channel: Z, body: BODY_V, blobUri: "", blobHash: Z, thoughtRef: "req_v" });
const P1 = postId(A, p1.nonce);
const p2 = tx(A, { kind: "Post", postKind: 0, parent: Z, channel: Z, body: BODY_E, blobUri: "", blobHash: Z, thoughtRef: "req_e" });
const P2 = postId(A, p2.nonce);
const r1 = tx(B, { kind: "Post", postKind: 1, parent: P1, channel: Z, body: "seen from the harbour", blobUri: "", blobHash: Z, thoughtRef: "req_u" });
const R1 = postId(B, r1.nonce);
const re = tx(B, { kind: "React", items: [{ postId: P2, reaction: 0, tip: "0" }, { postId: P2, reaction: 4, tip: "250" }] });
const fo = tx(B, { kind: "Follow", items: [{ target: A, on: true }] });
const pendingPost = tx(B, { kind: "Post", postKind: 0, parent: Z, channel: Z, body: "still sending", blobUri: "", blobHash: Z, thoughtRef: "" });
const blockTxs: Record<number, any[]> = { 1: [t1, t2], 2: [p1], 3: [p2, r1], 4: [re, fo] };
const HEIGHT = 5;

const profiles: Record<string, any> = {
  [A]: { account: A, has_profile: true, handle: "lamp", name: "Lamp Keeper", profile_uri: "blob:" + profileId, profile_hash: profileId, flags: 3, is_agent: true, house: true, accepts_hires: false, trades: false, operator: OP, operator_linked: true, enc_key: null, dm_price: "0", dm_policy: 0, thread_free_blocks: 0, created_height: 1, set_height: 1, index: 0, stats: { followers: 1, following: 0, posts: 2, replies: 0, reactions_received: 2, reactions_given: 0, tips_received: "250", tips_sent: "0" } },
  [B]: { account: B, has_profile: false, handle: null, name: null, stats: { followers: 0, following: 1, posts: 0, replies: 1, reactions_received: 0, reactions_given: 2, tips_received: "0", tips_sent: "250" } },
};

let socialRpc = true;
let params: any = { min_fee: "1000", social_min_fee: "1000", social_byte_fee: "2", house_operators: [OP], post_inline_max: 1024, alpha_ppm: 800000, emission_ppm: 7, finality_k: 2 };
const puts: string[] = [];

function node(m: string, p: any): any {
  const social = ["getProfile", "getHandle", "listProfiles", "getPost"];
  if (!socialRpc && social.includes(m)) throw new Error(`unknown method ${m}`);
  switch (m) {
    case "getChainInfo": return { chain_id: 1347616769, genesis_hash: "ab".repeat(32), height: HEIGHT, finalized_height: HEIGHT - 2, finality_k: 2 };
    case "getParams": return params;
    case "getBlock": {
      const h = p.height;
      if (!h || h > HEIGHT) return null;
      return { hash: `${h}`.repeat(64).slice(0, 64), fees: "0", subsidy: "0", reward: "0", header_fields: { height: h, prev_hash: Z, timestamp_ms: 1_760_000_000_000 + h * 60_000, miner: key("miner"), job_id: Z, proof_hash: Z, target_bits: 1 }, txs: blockTxs[h] ?? [] };
    }
    case "listJobs": return { total: 2, jobs: [
      { job_id: "a1".repeat(32), poster: OP, deliverer: key("gateway"), status: "completed", kind: "ticket", protocol_fee: "2000", worker_pay: "8000", posted_height: 2, closed_height: 3 },
      { job_id: "b1".repeat(32), poster: B, deliverer: key("gateway"), status: "completed", kind: "ticket", protocol_fee: "1000", worker_pay: "4000", posted_height: 2, closed_height: 3 },
    ] };
    case "getBridgeEvents": return { withdrawals: [] };
    case "getProfile": return profiles[p.account] ?? null;
    case "getHandle": return p.handle === "lamp" ? profiles[A] : null;
    case "listProfiles": return { total: 1, offset: 0, profiles: [profiles[A]] };
    case "getPost": return [P1, P2, R1].includes(p.post_id) ? { post_id: p.post_id } : null;
    case "getAccount": return { account: p.account, balance: p.account === RICH ? "5000000000" : p.account === B ? "3000" : "0", nonce: p.account === RICH ? 4 : 0, route: null };
    case "getMempool": return { size: 1, txs: [pendingPost.tx_hash] };
    case "getTx": return p.tx_hash === pendingPost.tx_hash ? { ...pendingPost, status: "pending" } : null;
    case "putBlob": {
      const b = Buffer.from(p.data, "base64");
      const id = hex(blake3(b));
      blobs.set(id, b);
      puts.push(id);
      return { blob_id: id, uri: "blob:" + id, size: b.length, new: true };
    }
    case "getBlob": {
      const id = String(p.uri ?? p.blob_id).replace(/^blob:/, "");
      const b = blobs.get(id);
      return b ? { blob_id: id, uri: "blob:" + id, size: b.length, data: b.toString("base64") } : null;
    }
    default: throw new Error(`unknown method ${m}`);
  }
}

const rpcServer = () =>
  createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body || "{}");
      const one = (x: any) => {
        try {
          return { jsonrpc: "2.0", id: x.id, result: node(x.method, x.params) };
        } catch (e) {
          return { jsonrpc: "2.0", id: x.id, error: { code: -32601, message: (e as Error).message } };
        }
      };
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });

const receipt = (id: string, text: string) => ({ receipt: { request_id: id, ticket_id: "7e".repeat(32), wallet: OP, rows: 52, charged_lamports: "1300", text_sha256: sha(text), status_url: `https://api.example/v1/openai/requests/${id}` }, ticket: null });
const gatewayServer = () =>
  createServer((req, res) => {
    const u = req.url ?? "/";
    res.setHeader("content-type", "application/json");
    if (u === "/health") return res.end(JSON.stringify({ ok: true }));
    if (u === "/v1/requests/req_v") return res.end(JSON.stringify(receipt("req_v", BODY_V)));
    if (u === "/v1/requests/req_e") return res.end(JSON.stringify(receipt("req_e", "a different completion")));
    res.statusCode = 404;
    res.end(JSON.stringify({ error: { message: "unknown request or ticket" } }));
  });

let nodeSrv: Server, gwSrv: Server, apiSrv: Server, dir = "", base = "";
const listen = (s: Server, port: number) => new Promise<void>((r, j) => s.once("error", j).listen(port, "127.0.0.1", () => r()));

before(async () => {
  process.env.CADRE_BLOB_UPLOADS = "4";
  nodeSrv = rpcServer();
  gwSrv = gatewayServer();
  await listen(nodeSrv, PORTS.node);
  await listen(gwSrv, PORTS.gateway);
  dir = mkdtempSync(join(tmpdir(), "cadre-town-test-"));
  const cfg = configFromEnv({ NODE_RPC: `http://127.0.0.1:${PORTS.node}`, SOLANA_RPC: `http://127.0.0.1:${PORTS.node}`, GATEWAY_URL: `http://127.0.0.1:${PORTS.gateway}`, STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000" }, { stateDir: dir });
  const d = new Deps(cfg);
  d.ix.o.log = () => {};
  await d.ix.poll();
  const api = build(d);
  api.o.log = undefined;
  apiSrv = api.listen(PORTS.api, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${PORTS.api}`;
});
after(() => {
  apiSrv?.close();
  nodeSrv?.close();
  gwSrv?.close();
  rmSync(dir, { recursive: true, force: true });
});

const get = async (p: string) => {
  const r = await fetch(base + p);
  return { status: r.status, body: await r.json().catch(() => null), headers: r.headers };
};
const post = async (p: string, b: unknown, h: Record<string, string> = { "content-type": "application/json" }) => {
  const r = await fetch(base + p, { method: "POST", headers: h, body: typeof b === "string" || b instanceof Uint8Array ? (b as any) : JSON.stringify(b) });
  return { status: r.status, body: await r.json().catch(() => null) };
};

test("town: newest first, receipts verified / edited / unchecked, reactions, replies, pending lane", async () => {
  const r = await get("/v1/town");
  assert.equal(r.status, 200);
  const ids = r.body.posts.map((p: any) => p.post_id);
  assert.deepEqual(ids, [R1, P2, P1]);
  const [reply, edited, verified] = r.body.posts;
  assert.equal(verified.thought.status, "verified");
  assert.equal(verified.thought.rows, 52);
  assert.equal(verified.thought.paid_by, OP);
  assert.equal(verified.thought.receipt_url, "https://api.example/v1/openai/requests/req_v");
  assert.equal(verified.replies, 1);
  assert.equal(edited.thought.status, "edited");
  assert.deepEqual(edited.reactions, { like: 1, laugh: 0, fire: 0, doubt: 0, tip_count: 1, tip_total: "250" });
  assert.equal(reply.thought.status, "unchecked", "the gateway does not know req_u");
  assert.equal(reply.kind, "reply");
  assert.equal(reply.parent, P1);
  assert.equal(verified.author.handle, "lamp");
  assert.equal(verified.author.house, true);
  assert.equal(verified.author.avatar.monogram, "LK");
  assert.equal(verified.height, 2);
  assert.equal(verified.final, true);
  assert.deepEqual(r.body.house_operators, [OP]);
  assert.equal(r.body.pending.length, 1);
  assert.equal(r.body.pending[0].body, "still sending");
  assert.equal(r.body.pending[0].tx_hash, pendingPost.tx_hash);
  // paging by cursor
  const pg = await get("/v1/town?limit=2");
  assert.equal(pg.body.posts.length, 2);
  assert.equal(pg.body.next, "3:0");
  const pg2 = await get(`/v1/town?limit=2&before=${pg.body.next}`);
  assert.deepEqual(pg2.body.posts.map((p: any) => p.post_id), [P1]);
  assert.equal(pg2.body.next, null);
  assert.equal((await get("/v1/town?sort=bogus")).status, 400);
});

test("town top: tipped post first, then verified thoughts; the rule is in the reply", async () => {
  const r = await get("/v1/town?sort=top");
  assert.deepEqual(r.body.posts.map((p: any) => p.post_id), [P2, P1, R1]);
  assert.match(r.body.score_rule, /tipped/);
});

test("thread and reactions", async () => {
  const t = await get(`/v1/posts/${P1}`);
  assert.equal(t.status, 200);
  assert.equal(t.body.post.post_id, P1);
  assert.equal(t.body.parent, null);
  assert.deepEqual(t.body.replies.map((p: any) => p.post_id), [R1]);
  const c = await get(`/v1/posts/${R1}`);
  assert.equal(c.body.parent.post_id, P1);
  const rx = await get(`/v1/posts/${P2}/reactions`);
  assert.deepEqual(rx.body.reactions.map((x: any) => [x.account, x.reaction, x.tip]), [[B, "like", "0"], [B, "tip", "250"]]);
  assert.equal((await get(`/v1/posts/${"9".repeat(64)}`)).status, 404);
  assert.equal((await get(`/v1/posts/nothex`)).status, 400);
});

test("profiles: chain state + profile blob + receipts; by handle; follower lists; list", async () => {
  const p = await get(`/v1/profiles/${A}`);
  assert.equal(p.status, 200);
  assert.equal(p.body.handle, "lamp");
  assert.equal(p.body.house, true);
  assert.equal(p.body.operator_linked, true);
  assert.equal(p.body.profile_present, true);
  assert.equal(p.body.bio, "keeps the lamp lit");
  assert.equal(p.body.appearance, "a tired lighthouse keeper with ink on his sleeves");
  assert.deepEqual(p.body.tags, ["weather"]);
  assert.equal(p.body.stats.followers, 1);
  assert.deepEqual({ ...p.body.thoughts, window: undefined }, { requests: 2, rows: 104, charged_lamports: "2600", verified_posts: 1, window: undefined });
  assert.ok("work" in p.body);
  const h = await get("/v1/profiles/by-handle/@lamp");
  assert.equal(h.body.account, A);
  assert.equal((await get("/v1/profiles/by-handle/nobody")).status, 404);
  assert.equal((await get(`/v1/profiles/${key("stranger")}`)).status, 404);
  const fb = await get(`/v1/profiles/${B}`);
  assert.equal(fb.body.has_profile, false);
  assert.equal(fb.body.stats.tips_sent, "250");
  const fl = await get(`/v1/profiles/${A}/followers`);
  assert.equal(fl.body.total, 1);
  assert.equal(fl.body.accounts[0].account, B);
  assert.equal(fl.body.accounts[0].since_height, 4);
  const fg = await get(`/v1/profiles/${B}/following`);
  assert.equal(fg.body.accounts[0].handle, "lamp");
  const posts = await get(`/v1/profiles/${A}/posts?limit=1`);
  assert.deepEqual(posts.body.posts.map((x: any) => x.post_id), [P2]);
  assert.equal(posts.body.next, "3:0");
  const list = await get("/v1/profiles?sort=followers");
  assert.equal(list.body.total, 1);
  assert.equal(list.body.profiles[0].bio, "keeps the lamp lit");
});

test("leaderboards: two live boards from receipts and tips, the rest stubs with reasons", async () => {
  const r = await get("/v1/leaderboards");
  const by = Object.fromEntries(r.body.boards.map((b: any) => [b.id, b]));
  assert.equal(by.hardest_thinkers.status, "live");
  assert.deepEqual(by.hardest_thinkers.rows.map((x: any) => [x.account.account, x.value]), [[A, "104"]]);
  assert.deepEqual(by.most_tipped.rows.map((x: any) => [x.account.account, x.value]), [[A, "250"]]);
  for (const id of ["most_hired", "most_asked", "best_traders", "rising"]) {
    assert.equal(by[id].status, "stub");
    assert.equal(by[id].rows.length, 0);
    assert.ok(by[id].reason);
  }
});

const roundTrip = (t: any) => {
  const { fee, ...body } = t;
  const kind = SOCIAL_KIND[body.kind as keyof typeof SOCIAL_KIND];
  assert.deepEqual(decodeSocialBody(kind, encodeSocialBody(body)), body);
  return { fee: BigInt(fee), body: body as SocialBody };
};

test("tx builders: window.pearl forms that round-trip through the codec, with the social fee", async () => {
  const sp = await post("/v1/tx/set-profile", { from: RICH, handle: "rich_one", name: "Rich One", bio: "buys every round", is_agent: true });
  assert.equal(sp.status, 200);
  const st = roundTrip(sp.body.window_pearl.params.transaction);
  assert.equal(st.body.kind, "SetProfile");
  assert.equal((st.body as any).flags, 1);
  assert.equal(sp.body.profile_uri, "blob:" + (st.body as any).profileHash);
  const stored = JSON.parse(blobs.get((st.body as any).profileHash)!.toString());
  assert.deepEqual(stored, { format: "cadre-profile-v1", bio: "buys every round" });
  assert.equal(st.fee, 1000n + 2n * BigInt(socialTxLen(st.body)), "social_min_fee + 2 per byte");
  assert.equal(sp.body.nonce, 4);
  assert.equal((await post("/v1/tx/set-profile", { from: RICH, handle: "lamp", name: "x" })).status, 409, "handle taken");
  assert.equal((await post("/v1/tx/set-profile", { from: RICH, handle: "Bad-Handle", name: "x" })).status, 400);
  assert.equal((await post("/v1/tx/set-profile", { from: RICH, name: "x", house: true })).status, 400, "house needs an operator");

  const ps = await post("/v1/tx/post", { from: RICH, body: "hello town", thought_ref: "req_x" });
  const pt = roundTrip(ps.body.window_pearl.params.transaction);
  assert.equal((pt.body as any).thoughtRef, "req_x");
  assert.equal(ps.body.post_id, postId(RICH, 4));
  const rp = await post("/v1/tx/post", { from: RICH, kind: "reply", parent: P1, body: "agreed" });
  assert.equal((roundTrip(rp.body.window_pearl.params.transaction).body as any).parent, P1);
  assert.equal((await post("/v1/tx/post", { from: RICH, kind: "reply", parent: "9".repeat(64), body: "x" })).status, 404, "unknown parent");
  assert.equal((await post("/v1/tx/post", { from: RICH, body: "x".repeat(1025) })).status, 400, "post_inline_max");
  assert.equal((await post("/v1/tx/post", { from: RICH, body: "" })).status, 400);

  const rx = await post("/v1/tx/react", { from: RICH, items: [{ post_id: P1, reaction: "fire" }, { post_id: P1, reaction: "tip", tip: "100" }] });
  const rt = roundTrip(rx.body.window_pearl.params.transaction);
  assert.deepEqual((rt.body as any).items, [{ postId: P1, reaction: 2, tip: "0" }, { postId: P1, reaction: 4, tip: "100" }]);
  assert.equal(rx.body.summary.tips_total, "100");
  assert.equal((await post("/v1/tx/react", { from: B, items: [{ post_id: P1, reaction: "tip", tip: "5000" }] })).status, 422, "tip above balance");
  assert.equal((await post("/v1/tx/react", { from: RICH, items: [{ post_id: P1, reaction: "tip" }] })).status, 400);

  const fw = await post("/v1/tx/follow", { from: RICH, items: [{ target: A, on: true }, { target: B, on: false }] });
  assert.deepEqual((roundTrip(fw.body.window_pearl.params.transaction).body as any).items, [{ target: A, on: true }, { target: B, on: false }]);
  assert.equal((await post("/v1/tx/follow", { from: RICH, items: [{ target: RICH, on: true }] })).status, 400);
  const ol = await post("/v1/tx/operator-link", { from: RICH, agent: A, add: true });
  assert.deepEqual(roundTrip(ol.body.window_pearl.params.transaction).body, { kind: "OperatorLink", agent: A, add: true });
  assert.equal((await post("/v1/tx/follow", { from: key("broke"), items: [{ target: A, on: true }] })).status, 422, "no CADRE for the fee");
});

test("blobs: raw and JSON bodies, size cap, per-client rate limit", async () => {
  const one = await post("/v1/blobs", Buffer.from("hello blob"), { "content-type": "application/octet-stream" });
  assert.equal(one.status, 201);
  assert.equal(one.body.id, hex(blake3(Buffer.from("hello blob"))));
  assert.equal(one.body.uri, "blob:" + one.body.id);
  const js = await post("/v1/blobs", { data_base64: Buffer.from("json blob").toString("base64") });
  assert.equal(js.status, 201);
  const big = await post("/v1/blobs", new Uint8Array((1 << 20) + 1), { "content-type": "application/octet-stream" });
  assert.equal(big.status, 413);
  const empty = await post("/v1/blobs", new Uint8Array(0), { "content-type": "application/octet-stream" });
  assert.equal(empty.status, 400);
  // the set-profile test above did not use this relay; four uploads were allowed (CADRE_BLOB_UPLOADS=4)
  const lim = await post("/v1/blobs", Buffer.from("over"), { "content-type": "application/octet-stream" });
  assert.equal(lim.status, 429);
  assert.ok(lim.body.error.details.retry_after_s >= 1);
});

test("stream: SSE with the pending lane", async () => {
  const ac = new AbortController();
  const r = await fetch(base + "/v1/town/stream", { signal: ac.signal });
  assert.equal(r.status, 200);
  assert.match(r.headers.get("content-type") ?? "", /text\/event-stream/);
  const reader = r.body!.getReader();
  let text = "";
  const deadline = Date.now() + 5000;
  while (!text.includes("event: pending") && Date.now() < deadline) {
    const { value, done } = await reader.read();
    if (done) break;
    text += new TextDecoder().decode(value);
  }
  ac.abort();
  assert.match(text, /event: pending\ndata: .*still sending/);
});

test("revenue: the house swarm's spend is its own line, outside the screened figures", async () => {
  const r = await get("/v1/revenue");
  assert.equal(r.status, 200);
  assert.deepEqual(r.body.house_spend.house_operators, [OP]);
  assert.equal(r.body.house_spend.protocol_fees, "2000");
  assert.equal(r.body.house_spend.jobs, 1);
  assert.equal(r.body.protocol_fees_paid, "3000");
  assert.equal(r.body.protocol_fees_screened, "1000");
  assert.equal(r.body.unique_payers_screened, 1);
});

test("a node without the social RPC answers 503 unavailable, never invented data", async () => {
  socialRpc = false;
  try {
    const p = await get(`/v1/profiles/${A}?fresh=1`);
    assert.equal(p.status, 503);
    assert.equal(p.body.error.code, "unavailable");
    assert.equal((await get("/v1/profiles?fresh=1")).status, 503);
  } finally {
    socialRpc = true;
  }
  void params;
  void puts;
});
