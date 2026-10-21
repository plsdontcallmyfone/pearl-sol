// DM routes (SPEC 4.10, docs/API.md "DMs and channels") against a fake node that serves the DM
// RPC (getDmParams, getDmGate, getDmStats, getDmThread, getMessageEscrow, getChannel,
// listChannels, getChannelMember) and blocks that carry real DM tx bytes.
// Ports 10686 (node) and 10688 (api): the owned test block.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";
import { b58encode, hex, sha256, utf8 } from "../../../apps/explorer/src/codec.ts";
import { DM_KIND, channelId, decodeDmTx, dmFee, dmSigningBytes, messageId, type DmBody } from "../../../apps/explorer/src/dm.ts";
import { socialSigningBytes, type SocialBody } from "../../../apps/explorer/src/social.ts";

const Z = "0".repeat(64);
const ZK = b58encode(new Uint8Array(32));
const key = (s: string) => b58encode(sha256(utf8("key:" + s)));
const A = key("agent"), B = key("human"), C = key("cold"), D = key("pest"), E = key("payer"), POOR = key("poor");
const ENC_A = key("enc-a");
const PORTS = { node: 10686, api: 10688 };

const nonces = new Map<string, number>();
function tx(from: string, body: DmBody | SocialBody) {
  const n = nonces.get(from) ?? 0;
  nonces.set(from, n + 1);
  const sb = body.kind in DM_KIND ? dmSigningBytes(1347616769, from, n, "1000", body as DmBody) : socialSigningBytes(1347616769, from, n, "1000", body as SocialBody);
  const bytes = Buffer.from([...sb, ...new Uint8Array(64)]);
  return { tx_hash: hex(blake3(bytes)), kind: body.kind, from, nonce: n, fee: "1000", tx: bytes.toString("base64") };
}
const dm = (to: string, text: string, o: Partial<Extract<DmBody, { kind: "SendMessage" }>> = {}): DmBody => ({ kind: "SendMessage", to, channel: Z, mode: 0, replyTo: Z, pay: "0", reclaimAfter: 0, body: text, blobUri: "", blobHash: Z, thoughtRef: "", ...o });
const SEALED = "01" + "01" + "11".repeat(32) + "22".repeat(24) + "1400" + "33".repeat(20);

const prof = tx(A, { kind: "SetProfile", handle: "oracle", name: "Fog Oracle", profileUri: "", profileHash: Z, flags: 1, operator: ZK, encKey: ENC_A, dmPrice: "5000", dmPolicy: 1, threadFreeBlocks: 0 });
const m1 = tx(B, dm(A, "will the fog lift?", { pay: "5000", reclaimAfter: 10 }));
const M1 = messageId(B, m1.nonce);
const pest = tx(D, dm(A, "buy my token"));
const r1 = tx(A, dm(B, "at dawn", { replyTo: M1 }));
const m2 = tx(C, dm(A, "quick question", { pay: "5000", reclaimAfter: 10 }));
const M2 = messageId(C, m2.nonce);
const deny = tx(A, { kind: "SetConsent", items: [{ peer: D, state: 2 }] });
const decline = tx(A, { kind: "ReclaimMessage", messageId: M2 });
const sealed = tx(B, dm(A, "", { mode: 1, body: undefined, bodyHex: SEALED }));
const cc = tx(A, { kind: "ChannelConfig", channelId: Z, name: "fog desk", about: "forecasts", policy: 0, joinPrice: "0", mode: 0, admins: [] });
const CH = channelId(A, cc.nonce);
const join1 = tx(B, { kind: "ChannelMember", channel: CH, action: 0, member: ZK, pay: "0" });
const cm = tx(A, dm(ZK, "morning: thick fog", { channel: CH }));
const m3 = tx(E, dm(A, "a paid one, still open", { pay: "6000", reclaimAfter: 20 }));
const M3 = messageId(E, m3.nonce);
const pendingTx = tx(D, dm(A, "still sending"));
const blockTxs: Record<number, any[]> = { 1: [prof], 2: [m1, pest], 3: [r1, m2, deny], 4: [decline, sealed], 5: [cc], 6: [join1, cm], 7: [m3] };
let HEIGHT = 8;

let dmRpc = true;
const params = { min_fee: "1000", social_min_fee: "1000", social_byte_fee: "2", house_operators: [], post_inline_max: 1024, finality_k: 2 };
const dmParams = { msg_inline_max: 1024, dm_reclaim_min: 10, dm_reclaim_max: 10080, dm_max_open_escrows: 256, dm_protocol_bps: 0, channel_sealed_max_members: 250, channel_create_fee: "0", channel_max_admins: 8, activation_height: 0, active: true };
const chan = { channel_id: CH, creator: A, name: "fog desk", about: "forecasts", policy: 0, policy_name: "open", join_price: "0", mode: 0, sealed: false, admins: [], members: 2, epoch: 0, epoch_height: 0, rekey_due: false, messages: 1, created_height: 5, set_height: 5, index: 0 };

function gate(from: string, to: string) {
  const g: any = { from, to, next_height: HEIGHT + 1, can_send: true, required_pay: "0", reason: "open", dm_policy: 0, dm_price: "0", consent: null, in_thread: false, enc_key: null, sealed_ok: false, msg_inline_max: 1024, reclaim_min: 10, reclaim_max: 10080 };
  if (to === A) {
    Object.assign(g, { dm_policy: 1, dm_price: "5000", enc_key: ENC_A, sealed_ok: true });
    if (from === D) Object.assign(g, { can_send: false, reason: "the recipient has blocked you", consent: "deny" });
    else if (from === B) Object.assign(g, { reason: "free: in a thread the recipient started, followed, or allowed", in_thread: true });
    else Object.assign(g, { required_pay: "5000", reason: "priced cold DM" });
  }
  return g;
}

function node(m: string, p: any): any {
  if (!dmRpc && /^(getDm|getMessage|getChannel|listChannels|getConsent)/.test(m)) throw new Error(`unknown method ${m}`);
  switch (m) {
    case "getChainInfo": return { chain_id: 1347616769, genesis_hash: "cd".repeat(32), height: HEIGHT, finalized_height: HEIGHT - 2, finality_k: 2 };
    case "getParams": return params;
    case "getBlock": {
      const h = p.height;
      if (!h || h > HEIGHT) return null;
      return { hash: `${h}`.repeat(64).slice(0, 64), fees: "0", subsidy: "0", reward: "0", header_fields: { height: h, prev_hash: Z, timestamp_ms: 1_760_000_000_000 + h * 5000, miner: key("miner"), job_id: Z, proof_hash: Z, target_bits: 1 }, txs: blockTxs[h] ?? [] };
    }
    case "listJobs": return { total: 0, jobs: [] };
    case "getBridgeEvents": return { withdrawals: [] };
    case "getProfile": return p.account === A ? { account: A, has_profile: true, handle: "oracle", name: "Fog Oracle", enc_key: ENC_A, dm_price: "5000", dm_policy: 1 } : null;
    case "getAccount": return { account: p.account, balance: p.account === POOR ? "10" : "1000000000", nonce: p.account === C ? 3 : 1, route: null };
    case "getMempool": return { size: 1, txs: [pendingTx.tx_hash] };
    case "getTx": return p.tx_hash === pendingTx.tx_hash ? { ...pendingTx, status: "pending" } : null;
    case "getDmParams": return dmParams;
    case "getDmGate": return gate(p.from, p.to);
    case "getDmStats": return { account: p.account, sent: 1, received: 3, pay_in: "5000", pay_out: "0", open_escrows: 0, locked: "0", answered: 1, refunded: 0 };
    case "getDmThread": return { a: p.a, b: p.b, last_a_to_b: 3, last_b_to_a: 4, consent_a_for_b: null, consent_b_for_a: null };
    case "getMessageEscrow": return p.message_id === M3 ? { message_id: M3, from: E, to: A, pay: "6000", height: 7, deadline: 27, reply_open: true, blocks_left: 19, reclaimable: false } : null;
    case "getChannel": return p.channel === CH ? chan : null;
    case "listChannels": return { total: 1, offset: 0, channels: [chan] };
    case "getChannelMember": return { channel_id: p.channel, account: p.account, member: p.channel === CH && [A, B].includes(p.account), since_height: 5, invited: false, invited_height: null };
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

let nodeSrv: Server, apiSrv: Server, dir = "", base = "", deps: Deps;
const listen = (s: Server, port: number) => new Promise<void>((r, j) => s.once("error", j).listen(port, "127.0.0.1", () => r()));

before(async () => {
  nodeSrv = rpcServer();
  await listen(nodeSrv, PORTS.node);
  dir = mkdtempSync(join(tmpdir(), "cadre-dms-test-"));
  const cfg = configFromEnv({ NODE_RPC: `http://127.0.0.1:${PORTS.node}`, SOLANA_RPC: `http://127.0.0.1:${PORTS.node}`, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000" }, { stateDir: dir });
  deps = new Deps(cfg);
  deps.ix.o.log = () => {};
  await deps.ix.poll();
  const api = build(deps);
  api.o.log = undefined;
  apiSrv = api.listen(PORTS.api, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${PORTS.api}`;
});
after(() => {
  apiSrv?.close();
  nodeSrv?.close();
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

test("inbox: primary / requests / blocked boxes, pending, open escrows, stats, enc_key", async () => {
  const r = await get(`/v1/inbox/${A}`);
  assert.equal(r.status, 200);
  const by = Object.fromEntries(r.body.threads.map((t: any) => [t.peer.account, t]));
  assert.equal(by[B].box, "primary", "A wrote to B");
  assert.equal(by[C].box, "requests", "C wrote first, A never wrote, follows or allowed");
  assert.equal(by[D].box, "blocked", "A denied D");
  assert.equal(by[E].box, "requests");
  assert.equal(by[E].unanswered_paid, 1);
  assert.deepEqual(by[E].open_escrows, { in: 1, out: 0 });
  assert.equal(by[B].sealed, true, "the last message in the B thread is sealed");
  assert.equal(by[B].last.body, null);
  assert.equal(by[B].count, 3);
  assert.equal(r.body.threads[0].peer.account, E, "newest thread first");
  assert.deepEqual(r.body.counts, { primary: 1, requests: 2, blocked: 1 });
  assert.equal(r.body.escrows.in.length, 1);
  assert.equal(r.body.escrows.in[0].message_id, M3);
  assert.equal(r.body.escrows.in[0].state, "open");
  assert.equal(r.body.escrows.in[0].blocks_left, 27 - HEIGHT);
  assert.equal(r.body.pending.length, 1);
  assert.equal(r.body.pending[0].status, "sending");
  assert.equal(r.body.pending[0].body, "still sending");
  assert.equal(r.body.channels.length, 1);
  assert.equal(r.body.channels[0].name, "fog desk");
  assert.equal(r.body.enc_key, ENC_A);
  assert.equal(r.body.stats.answered, 1);
  assert.match(r.body.metadata_note, /sealed message hides its text/);
  const req = await get(`/v1/inbox/${A}?box=requests`);
  assert.deepEqual(req.body.threads.map((t: any) => t.peer.account).sort(), [C, E].sort());
  assert.equal((await get(`/v1/inbox/${A}?box=nope`)).status, 400);
});

test("thread: newest first, released escrow names the reply, consent and gate, paging", async () => {
  const r = await get(`/v1/threads/${B}/${A}`);
  assert.equal(r.status, 200);
  assert.deepEqual(r.body.messages.map((m: any) => m.mode), ["sealed", "plain", "plain"]);
  const first = r.body.messages[2];
  assert.equal(first.message_id, M1);
  assert.equal(first.body, "will the fog lift?");
  assert.equal(first.from.account, B);
  assert.equal(first.to.handle, "oracle");
  assert.equal(first.escrow.state, "released");
  assert.equal(first.escrow.settled_tx, r1.tx_hash);
  assert.equal(first.status, "final");
  assert.equal(r.body.messages[1].reply_to, M1);
  assert.equal(r.body.messages[0].body_hex, SEALED);
  assert.equal(r.body.gate.in_thread, true);
  assert.deepEqual(r.body.consent, { a_for_b: null, b_for_a: null });
  const pg = await get(`/v1/threads/${B}/${A}?limit=2`);
  assert.equal(pg.body.messages.length, 2);
  assert.equal(pg.body.next, "3:0");
  const pg2 = await get(`/v1/threads/${B}/${A}?limit=2&before=${pg.body.next}`);
  assert.deepEqual(pg2.body.messages.map((m: any) => m.message_id), [M1]);
  assert.equal(pg2.body.next, null);
});

test("messages, escrows (refunded by a decline), consent lists", async () => {
  const m = await get(`/v1/messages/${M2}`);
  assert.equal(m.status, 200);
  assert.equal(m.body.escrow.state, "refunded");
  assert.equal(m.body.escrow.declined, true);
  assert.equal(m.body.escrow.settled_tx, decline.tx_hash);
  assert.equal((await get(`/v1/messages/${"ee".repeat(32)}`)).status, 404);
  const e = await get(`/v1/escrows/${A}?role=recipient&state=refunded`);
  assert.deepEqual(e.body.escrows.map((x: any) => x.message_id), [M2]);
  const open = await get(`/v1/escrows/${E}?role=sender&state=open`);
  assert.deepEqual(open.body.escrows.map((x: any) => x.message_id), [M3]);
  const c = await get(`/v1/consent/${A}`);
  assert.deepEqual(c.body.deny.map((x: any) => x.account), [D]);
  assert.equal(c.body.deny[0].height, 3);
  assert.deepEqual(c.body.allow, []);
});

test("channels: list, detail with members, messages", async () => {
  const l = await get("/v1/channels");
  assert.equal(l.status, 200);
  assert.equal(l.body.total, 1);
  assert.equal(l.body.channels[0].policy, "open");
  assert.equal(l.body.channels[0].last.body, "morning: thick fog");
  const d = await get(`/v1/channels/${CH}`);
  assert.equal(d.body.member_count, 2);
  assert.deepEqual(d.body.members.map((x: any) => [x.account.account, x.admin]), [[A, true], [B, false]]);
  const ms = await get(`/v1/channels/${CH}/messages`);
  assert.equal(ms.body.messages[0].channel, CH);
  assert.equal(ms.body.messages[0].to, null);
  assert.equal((await get(`/v1/channels/${CH}/key?account=${A}`)).status, 404, "a plain channel has no keys");
  assert.equal((await get(`/v1/channels/${"ab".repeat(32)}`)).status, 404);
});

test("builders: gate refusals, priced DM, sealed checks, insufficient funds, the encoded tx", async () => {
  const blocked = await post("/v1/tx/send-message", { from: D, to: A, body: "again" });
  assert.equal(blocked.status, 409);
  assert.equal(blocked.body.error.code, "refused");
  assert.match(blocked.body.error.message, /blocked you/);
  const short = await post("/v1/tx/send-message", { from: C, to: A, body: "hi" });
  assert.equal(short.status, 409);
  assert.match(short.body.error.message, /price of 5000/);
  const paid = await post("/v1/tx/send-message", { from: C, to: A, body: "hi", pay: "5000", reclaim_after: 30 });
  assert.equal(paid.status, 200);
  assert.equal(paid.body.message_id, messageId(C, 3));
  assert.equal(paid.body.gate.required_pay, "5000");
  const t = paid.body.window_pearl.params.transaction;
  assert.equal(t.kind, "SendMessage");
  assert.equal(t.body, "hi");
  assert.equal(t.reclaimAfter, 30);
  const { fee, ...bodyOnly } = t;
  assert.equal(fee, dmFee(params, bodyOnly).toString());
  const sb = dmSigningBytes(1347616769, C, 3, fee, bodyOnly);
  assert.deepEqual(decodeDmTx(Uint8Array.from([...sb, ...new Uint8Array(64)])).body, { ...bodyOnly, bodyHex: Buffer.from("hi").toString("hex") });
  assert.equal((await post("/v1/tx/send-message", { from: C, to: A, body: "hi", pay: "5000", reclaim_after: 3 })).status, 400, "below dm_reclaim_min");
  const noEnc = await post("/v1/tx/send-message", { from: A, to: B, mode: "sealed", body_hex: SEALED });
  assert.equal(noEnc.status, 409);
  assert.match(noEnc.body.error.message, /enc_key/);
  const sOk = await post("/v1/tx/send-message", { from: B, to: A, mode: "sealed", body_hex: SEALED });
  assert.equal(sOk.status, 200);
  assert.equal(sOk.body.window_pearl.params.transaction.bodyHex, SEALED);
  assert.equal(sOk.body.window_pearl.params.transaction.body, undefined);
  assert.equal((await post("/v1/tx/send-message", { from: B, to: A, mode: "sealed", body_hex: "0102" })).status, 400, "bad framing");
  assert.equal((await post("/v1/tx/send-message", { from: B, to: A, mode: "sealed", body: "plaintext" })).status, 400, "the API never seals");
  const poor = await post("/v1/tx/send-message", { from: POOR, to: B, body: "hi" });
  assert.equal(poor.status, 422);
  // channel message: members only
  assert.equal((await post("/v1/tx/send-message", { from: C, channel: CH, body: "hi" })).status, 409);
  assert.equal((await post("/v1/tx/send-message", { from: B, channel: CH, body: "hi" })).status, 200);
  // reclaim: the window is open for the sender, the recipient may decline
  const early = await post("/v1/tx/reclaim-message", { from: E, message_id: M3 });
  assert.equal(early.status, 409);
  assert.match(early.body.error.message, /open until height 27/);
  const dec = await post("/v1/tx/reclaim-message", { from: A, message_id: M3 });
  assert.equal(dec.status, 200);
  assert.equal(dec.body.declining, true);
  assert.equal((await post("/v1/tx/reclaim-message", { from: A, message_id: M2 })).status, 409, "no escrow left");
  // consent
  const cs = await post("/v1/tx/set-consent", { from: A, items: [{ peer: C, state: "allow" }, { peer: D, state: "clear" }] });
  assert.equal(cs.status, 200);
  assert.deepEqual(cs.body.window_pearl.params.transaction.items, [{ peer: C, state: 1 }, { peer: D, state: 0 }]);
  assert.equal((await post("/v1/tx/set-consent", { from: A, items: [{ peer: A, state: "deny" }] })).status, 400);
  // channels
  const cr = await post("/v1/tx/channel-config", { from: B, name: "harbour", policy: "paid", join_price: "100", mode: "sealed" });
  assert.equal(cr.status, 200);
  assert.equal(cr.body.channel_id, channelId(B, 1));
  assert.equal((await post("/v1/tx/channel-config", { from: B, name: "x", policy: "paid" })).status, 400, "paid needs a price");
  assert.equal((await post("/v1/tx/channel-config", { from: B, channel_id: CH, name: "renamed" })).status, 409, "not an admin");
  assert.equal((await post("/v1/tx/channel-member", { from: B, channel: CH, action: "join" })).status, 409, "already a member");
  const jn = await post("/v1/tx/channel-member", { from: C, channel: CH, action: "join" });
  assert.equal(jn.status, 200);
  assert.equal(jn.body.window_pearl.params.transaction.member, ZK);
  assert.equal((await post("/v1/tx/channel-member", { from: A, channel: CH, action: "remove", member: A })).status, 409, "never the creator");
  assert.equal((await post("/v1/tx/channel-key", { from: A, channel: CH, epoch: 1, envelopes: [{ member: B, envelope_hex: "ab".repeat(104) }] })).status, 409, "plain channel");
});

test("/v1/tx/encode handles the DM kinds", async () => {
  const t = { kind: "SendMessage", to: A, channel: Z, mode: 0, replyTo: Z, pay: "0", reclaimAfter: 0, body: "encode me", blobUri: "", blobHash: Z, thoughtRef: "" };
  const r = await post("/v1/tx/encode", { from: B, transaction: t });
  assert.equal(r.status, 200);
  const fee = dmFee(params, t as DmBody);
  assert.equal(r.body.fee, fee.toString());
  assert.equal(r.body.signing_bytes, Buffer.from(dmSigningBytes(1347616769, B, 1, fee, t as DmBody)).toString("base64"));
  assert.match(r.body.plain, /Send "encode me" to /);
  const k = await post("/v1/tx/encode", { from: A, transaction: { kind: "SetConsent", items: [{ peer: D, state: 2 }] } });
  assert.equal(k.status, 200);
  assert.match(k.body.plain, /deny/);
});

test("stream: pending, then message and escrow events as blocks land", async () => {
  const ac = new AbortController();
  const r = await fetch(base + `/v1/inbox/${A}/stream`, { signal: ac.signal });
  assert.equal(r.status, 200);
  assert.match(r.headers.get("content-type") ?? "", /text\/event-stream/);
  const reader = r.body!.getReader();
  let text = "";
  const until = async (re: RegExp) => {
    const deadline = Date.now() + 5000;
    while (!re.test(text) && Date.now() < deadline) {
      const { value, done } = await reader.read();
      if (done) break;
      text += new TextDecoder().decode(value);
    }
  };
  await until(/event: pending/);
  assert.match(text, /event: pending\ndata: .*still sending/);
  // a reply from A releases E's escrow at height 9
  blockTxs[9] = [tx(A, dm(E, "answered", { replyTo: M3 }))];
  HEIGHT = 9;
  await deps.ix.poll();
  await until(/event: escrow/);
  ac.abort();
  assert.match(text, /event: message\ndata: .*"answered"/);
  assert.match(text, /event: escrow\ndata: .*"state":"released"/);
});

test("a node without the DM RPC answers 503 unavailable, never invented data", async () => {
  dmRpc = false;
  try {
    const g = await get(`/v1/dm/gate?from=${A}&to=${B}&fresh=1`);
    assert.equal(g.status, 503);
    assert.equal(g.body.error.code, "unavailable");
    assert.equal((await get("/v1/dm/params?fresh=1")).status, 503);
    assert.equal((await get(`/v1/inbox/${A}?fresh=1`)).status, 503);
  } finally {
    dmRpc = true;
  }
  const p = await get("/v1/dm/params?fresh=2");
  assert.equal(p.body.social_byte_fee, "2");
  assert.equal(p.body.msg_inline_max, 1024);
});
