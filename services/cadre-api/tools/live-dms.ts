// Live check of the DM routes and builders (SPEC 4.10, docs/API.md "DMs and channels") against a
// running cadre-api over a chain with the DM kinds. It makes fresh test keys (written to KEYS_DIR,
// never printed), funds them with BridgeUnlock txs signed by the localnet attester key, then signs
// every builder's window.cadre transaction itself and sends it with POST /v1/tx/chain.
//
//   API=http://127.0.0.1:11750 NODE_RPC=http://127.0.0.1:11730 KEYS_DIR=~/dms-api/keys \
//   ATTESTER_KEY=keys/localnet/attester-keypair.json node services/cadre-api/tools/live-dms.ts
//
// Prints one line per check and a JSON summary (tx hashes and heights) at the end.

import { createPrivateKey, createPublicKey, hkdfSync, randomBytes, sign as edSign } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { b58encode, hex } from "../../../apps/explorer/src/codec.ts";
import { DM_KIND, dmSigningBytes, type DmBody } from "../../../apps/explorer/src/dm.ts";
import { socialFee, socialSigningBytes, type SocialBody } from "../../../apps/explorer/src/social.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";

const API = process.env.API ?? "http://127.0.0.1:11750";
const NODE = process.env.NODE_RPC ?? "http://127.0.0.1:11730";
const KEYS = process.env.KEYS_DIR ?? join(process.env.HOME ?? ".", "dms-api", "keys");
const ATTESTER = process.env.ATTESTER_KEY ?? "keys/localnet/attester-keypair.json";
const ED_PKCS8 = Buffer.from("302e020100300506032b657004220420", "hex");
const X_PKCS8 = Buffer.from("302e020100300506032b656e04220420", "hex");
const NS = Buffer.from("pearl-sol:tx:v1");
const Z = "0".repeat(64);
const ZK = b58encode(new Uint8Array(32));

interface Key { name: string; seed: Buffer; pub: string; enc: string }
function pubOf(seed: Buffer) {
  const k = createPrivateKey({ key: Buffer.concat([ED_PKCS8, seed]), format: "der", type: "pkcs8" });
  return b58encode(Buffer.from(createPublicKey(k).export({ format: "jwk" }).x!, "base64url"));
}
function encOf(seed: Buffer) {
  const sec = Buffer.from(hkdfSync("sha256", seed, Buffer.from("cadre-dm-v1"), Buffer.from("x25519 enc_key"), 32));
  const k = createPrivateKey({ key: Buffer.concat([X_PKCS8, sec]), format: "der", type: "pkcs8" });
  return b58encode(Buffer.from(createPublicKey(k).export({ format: "jwk" }).x!, "base64url"));
}
function loadKey(name: string): Key {
  mkdirSync(KEYS, { recursive: true, mode: 0o700 });
  const f = join(KEYS, `${name}.json`);
  if (!existsSync(f)) writeFileSync(f, JSON.stringify({ seed_hex: randomBytes(32).toString("hex") }), { mode: 0o600 });
  const seed = Buffer.from(JSON.parse(readFileSync(f, "utf8")).seed_hex, "hex");
  return { name, seed, pub: pubOf(seed), enc: encOf(seed) };
}
const edSig = (seed: Buffer, msg: Uint8Array) => edSign(null, msg, createPrivateKey({ key: Buffer.concat([ED_PKCS8, seed]), format: "der", type: "pkcs8" }));
const signTx = (k: Key, sb: Uint8Array) => Buffer.concat([sb, edSig(k.seed, Buffer.concat([Buffer.from([NS.length]), NS, sb]))]);

async function rpc(method: string, params: unknown) {
  const r = await fetch(NODE, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
  const j: any = await r.json();
  if (j.error) throw new Error(`${method}: ${j.error.message}`);
  return j.result;
}
let bust = 0;
async function get(p: string) {
  const r = await fetch(API + p + (p.includes("?") ? "&" : "?") + `_=${++bust}`);
  return { status: r.status, body: (await r.json().catch(() => null)) as any };
}
async function post(p: string, b: unknown) {
  const r = await fetch(API + p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) });
  return { status: r.status, body: (await r.json().catch(() => null)) as any };
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

let pass = 0, fail = 0;
const log: Record<string, unknown> = {};
function check(name: string, ok: boolean, detail = "") {
  if (ok) pass++;
  else fail++;
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
}

/** Sign a builder reply's window.cadre transaction and send it; wait until the API's index has it. */
async function sendBuilt(k: Key, built: any, label: string) {
  const { fee, ...body } = built.window_pearl.params.transaction;
  const sb = body.kind in DM_KIND ? dmSigningBytes(built.chain_id, k.pub, built.nonce, fee, body as DmBody) : socialSigningBytes(built.chain_id, k.pub, built.nonce, fee, body as SocialBody);
  const tx = signTx(k, sb);
  const s = await post("/v1/tx/chain", { tx: tx.toString("base64") });
  if (s.status !== 200) throw new Error(`${label}: send ${s.status} ${JSON.stringify(s.body)}`);
  return s.body.tx_hash as string;
}
async function included(hash: string, label: string) {
  for (let i = 0; i < 60; i++) {
    const t = await rpc("getTx", { tx_hash: hash });
    if (t?.status === "included") {
      // the API's indexer polls every 3 s
      for (let j = 0; j < 20; j++) {
        const r = await get("/v1/ready");
        if ((r.body?.checks?.index_complete?.detail?.blocks_indexed ?? 0) >= t.height) break;
        await sleep(1000);
      }
      await sleep(1500);
      log[label] = { tx_hash: hash, height: t.height };
      return t.height as number;
    }
    await sleep(1000);
  }
  throw new Error(`${label}: ${hash} not included after 60 s`);
}
async function build(k: Key, path: string, b: any, label: string) {
  const r = await post(path, { from: k.pub, ...b });
  if (r.status !== 200) throw new Error(`${label}: ${path} ${r.status} ${JSON.stringify(r.body?.error ?? r.body)}`);
  return r.body;
}
async function run(k: Key, path: string, b: any, label: string) {
  const built = await build(k, path, b, label);
  const h = await sendBuilt(k, built, label);
  const height = await included(h, label);
  return { built, hash: h, height };
}

async function fund(k: Key, att: { seed: Buffer; pub: string }, chainId: number) {
  const acct = await rpc("getAccount", { account: k.pub });
  if (BigInt(acct.balance) >= 5_000_000_000n) return null;
  const nonce = BigInt("0x" + randomBytes(5).toString("hex")) + (1n << 44n);
  const amountSpl = 20_000_000n; // 20 CADRE in chain units (x1000)
  const u64 = (v: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(v); return b; };
  const recip = Buffer.from(createPublicKey(createPrivateKey({ key: Buffer.concat([ED_PKCS8, k.seed]), format: "der", type: "pkcs8" })).export({ format: "jwk" }).x!, "base64url");
  const msg = Buffer.concat([Buffer.from("pearl-sol:deposit:v1"), u64(nonce), recip, u64(amountSpl)]);
  const attPub = Buffer.from(createPublicKey(createPrivateKey({ key: Buffer.concat([ED_PKCS8, att.seed]), format: "der", type: "pkcs8" })).export({ format: "jwk" }).x!, "base64url");
  const body = Buffer.concat([u64(nonce), recip, u64(amountSpl), Buffer.from([0, 1]), attPub, edSig(att.seed, msg)]);
  const head = Buffer.alloc(54);
  head[0] = 1;
  head.writeUInt32LE(chainId, 1);
  recip.copy(head, 5);
  head.writeBigUInt64LE(BigInt(acct.nonce), 37);
  head.writeBigUInt64LE(0n, 45);
  head[53] = 6;
  const tx = signTx(k, Buffer.concat([head, body]));
  const r = await rpc("sendTx", { tx: tx.toString("base64") });
  return r.tx_hash as string;
}

async function setProfile(k: Key, chainId: number, o: { name: string; handle: string; dmPrice: string; dmPolicy: number; agent: boolean }) {
  const params = await rpc("getParams", {});
  const acct = await rpc("getAccount", { account: k.pub });
  const body: SocialBody = { kind: "SetProfile", handle: o.handle, name: o.name, profileUri: "", profileHash: Z, flags: o.agent ? 1 : 0, operator: ZK, encKey: k.enc, dmPrice: o.dmPrice, dmPolicy: o.dmPolicy, threadFreeBlocks: 0 };
  const fee = socialFee(params, body);
  const tx = signTx(k, socialSigningBytes(chainId, k.pub, acct.nonce, fee, body));
  const s = await post("/v1/tx/chain", { tx: tx.toString("base64") });
  if (s.status !== 200) throw new Error(`profile ${k.name}: ${JSON.stringify(s.body)}`);
  return s.body.tx_hash as string;
}

async function main() {
  const info = await rpc("getChainInfo", {});
  const chainId = info.chain_id as number;
  const attJson = JSON.parse(readFileSync(ATTESTER, "utf8"));
  const att = { seed: Buffer.from(attJson.slice(0, 32)), pub: b58encode(Uint8Array.from(attJson.slice(32))) };
  const tag = randomBytes(3).toString("hex");
  const [AG, HU, CO, PE, PY, M1, M2, PL] = ["agent", "human", "cold", "pest", "payer", "member1", "member2", "plain"].map((n) => loadKey(`${n}-${tag}`));
  console.log(`run ${tag} on chain ${chainId} at height ${info.height}; keys in ${KEYS} (not printed)`);

  // 0. fund every key, then profiles: AG priced (5 CADRE... 0.005 CADRE) with an enc_key; HU, M1, M2 with enc_keys
  const fundHashes = await Promise.all([AG, HU, CO, PE, PY, M1, M2, PL].map((k) => fund(k, att, chainId)));
  for (const [i, h] of fundHashes.entries()) if (h) await included(h, `fund_${i}`);
  const price = "5000000";
  const ph = await Promise.all([
    setProfile(AG, chainId, { name: "Fog Oracle", handle: `fog_${tag}`, dmPrice: price, dmPolicy: 1, agent: true }),
    setProfile(HU, chainId, { name: "Harbour Human", handle: `hb_${tag}`, dmPrice: "0", dmPolicy: 0, agent: false }),
    setProfile(M1, chainId, { name: "Member One", handle: `m1_${tag}`, dmPrice: "0", dmPolicy: 0, agent: false }),
    setProfile(M2, chainId, { name: "Member Two", handle: `m2_${tag}`, dmPrice: "0", dmPolicy: 0, agent: false }),
  ]);
  for (const [i, h] of ph.entries()) await included(h, `profile_${["agent", "human", "member1", "member2"][i]}`);

  // GET routes that need no history
  const p = await get("/v1/dm/params");
  check("GET /v1/dm/params", p.status === 200 && p.body.active === true && typeof p.body.msg_inline_max === "number" && p.body.social_min_fee !== undefined, JSON.stringify({ msg_inline_max: p.body.msg_inline_max, reclaim_min: p.body.dm_reclaim_min }));
  const reclaimMin = Number(p.body.dm_reclaim_min);
  const g0 = await get(`/v1/dm/gate?from=${CO.pub}&to=${AG.pub}`);
  check("GET /v1/dm/gate priced cold DM", g0.status === 200 && g0.body.required_pay === price && g0.body.sealed_ok === true, `${g0.body.reason}, required_pay ${g0.body.required_pay}`);

  // SSE stream on AG before any DM to it
  const ac = new AbortController();
  let sse = "";
  const sseDone = (async () => {
    try {
      const r = await fetch(`${API}/v1/inbox/${AG.pub}/stream`, { signal: ac.signal });
      const rd = r.body!.getReader();
      for (;;) {
        const { value, done } = await rd.read();
        if (done) break;
        sse += new TextDecoder().decode(value);
      }
    } catch {
      /* aborted */
    }
  })();

  // 1. plain DM: PL -> HU (HU open, free)
  const plain = await run(PL, "/v1/tx/send-message", { to: HU.pub, body: `hello from the harbour ${tag}` }, "plain_dm");
  const pm = await get(`/v1/messages/${plain.built.message_id}`);
  check("plain DM indexed (GET /v1/messages)", pm.status === 200 && pm.body.body === `hello from the harbour ${tag}` && pm.body.mode === "plain" && pm.body.status === "delivered", `height ${pm.body?.height}, status ${pm.body?.status}`);
  const inHU = await get(`/v1/inbox/${HU.pub}`);
  const thHU = inHU.body.threads.find((t: any) => t.peer.account === PL.pub);
  check("inbox: a first DM from a stranger lands in requests", thHU?.box === "requests", thHU?.box);
  const inPL = await get(`/v1/inbox/${PL.pub}`);
  check("inbox: the sender's side is primary", inPL.body.threads.find((t: any) => t.peer.account === HU.pub)?.box === "primary");

  // 2. priced DM refused, then paid at once (no escrow)
  const ref = await post("/v1/tx/send-message", { from: CO.pub, to: AG.pub, body: "free question?" });
  check("priced DM without pay refused 409", ref.status === 409 && ref.body.error.code === "refused", ref.body?.error?.message);
  const paid = await run(CO, "/v1/tx/send-message", { to: AG.pub, body: "paid question", pay: price }, "priced_dm_paid");
  const pmsg = await get(`/v1/messages/${paid.built.message_id}`);
  check("priced DM paid and settled at once", pmsg.body.pay === price && pmsg.body.escrow === null);

  // 3. escrow, then a reply in time releases it ("pending" sending state checked on the reply)
  const esc = await run(PY, "/v1/tx/send-message", { to: AG.pub, body: "escrowed question", pay: price, reclaim_after: reclaimMin + 20 }, "escrow_dm");
  const e1 = await get(`/v1/messages/${esc.built.message_id}`);
  check("escrow open", e1.body.escrow?.state === "open" && e1.body.escrow.deadline === esc.height + reclaimMin + 20, JSON.stringify(e1.body.escrow));
  const ein = await get(`/v1/inbox/${AG.pub}`);
  const pyth = ein.body.threads.find((t: any) => t.peer.account === PY.pub);
  check("inbox: unanswered paid DM counted, escrow listed in", pyth?.unanswered_paid === 1 && ein.body.escrows.in.some((e: any) => e.message_id === esc.built.message_id), `box ${pyth?.box}`);
  const rb = await build(AG, "/v1/tx/send-message", { to: PY.pub, body: "answer: the fog lifts at dawn", reply_to: esc.built.message_id }, "escrow_reply");
  const rh = await sendBuilt(AG, rb, "escrow_reply");
  let sawPending = false;
  for (let i = 0; i < 20 && !sawPending; i++) {
    const t = await get(`/v1/threads/${AG.pub}/${PY.pub}`);
    if (t.body.pending?.some((x: any) => x.tx_hash === rh && x.status === "sending")) sawPending = true;
    else if (t.body.messages?.some((m: any) => m.tx_hash === rh)) break;
    else await sleep(400);
  }
  check("pending: the reply shows as sending before inclusion", sawPending);
  await included(rh, "escrow_reply");
  const e2 = await get(`/v1/messages/${esc.built.message_id}`);
  check("escrow released by the reply", e2.body.escrow?.state === "released" && e2.body.escrow.settled_tx === rh, JSON.stringify(e2.body.escrow));
  const chainEsc = await rpc("getMessageEscrow", { message_id: esc.built.message_id });
  check("chain agrees: escrow record gone", chainEsc === null);

  // 4. escrow, then the sender reclaims after the window
  const esc2 = await run(CO, "/v1/tx/send-message", { to: AG.pub, body: "will you answer this one?", pay: price, reclaim_after: reclaimMin }, "escrow_to_reclaim");
  const early = await post("/v1/tx/reclaim-message", { from: CO.pub, message_id: esc2.built.message_id });
  check("reclaim inside the window refused 409", early.status === 409, early.body?.error?.message);
  // 5. recipient decline (in parallel with the wait)
  const esc3 = await run(PE, "/v1/tx/send-message", { to: AG.pub, body: "buy my token", pay: price, reclaim_after: reclaimMin + 50 }, "escrow_declined");
  const dec = await run(AG, "/v1/tx/reclaim-message", { message_id: esc3.built.message_id }, "decline");
  check("decline builder flagged declining", dec.built.declining === true);
  const e3 = await get(`/v1/messages/${esc3.built.message_id}`);
  check("declined escrow refunded with declined=true", e3.body.escrow?.state === "refunded" && e3.body.escrow.declined === true && e3.body.escrow.settled_tx === dec.hash);
  // wait for the window of esc2 to end
  for (;;) {
    const m = await get(`/v1/messages/${esc2.built.message_id}`);
    if (m.body.escrow?.state === "expired") break;
    await sleep(3000);
  }
  check("escrow expired after the window", true);
  const rc = await run(CO, "/v1/tx/reclaim-message", { message_id: esc2.built.message_id }, "reclaim_after_window");
  const e4 = await get(`/v1/messages/${esc2.built.message_id}`);
  check("reclaimed escrow refunded (declined=false)", e4.body.escrow?.state === "refunded" && e4.body.escrow.declined === false && rc.built.declining === false);
  const escl = await get(`/v1/escrows/${AG.pub}?role=recipient&state=refunded`);
  check("GET /v1/escrows filter", escl.status === 200 && escl.body.escrows.length === 2, `${escl.body.escrows.length} refunded`);

  // 6. consent: deny refused, clear
  await run(AG, "/v1/tx/set-consent", { items: [{ peer: PE.pub, state: "deny" }, { peer: HU.pub, state: "allow" }] }, "consent_deny");
  const blk = await post("/v1/tx/send-message", { from: PE.pub, to: AG.pub, body: "again", pay: price });
  check("denied sender refused 409 by the builder", blk.status === 409 && /blocked/.test(blk.body.error.message), blk.body?.error?.message);
  // a signed tx past the builder is refused by the node's sendTx admission
  const acctPE = await rpc("getAccount", { account: PE.pub });
  const forced: DmBody = { kind: "SendMessage", to: AG.pub, channel: Z, mode: 0, replyTo: Z, pay: price, reclaimAfter: 0, body: "forced", blobUri: "", blobHash: Z, thoughtRef: "" };
  const fsend = await post("/v1/tx/chain", { tx: signTx(PE, dmSigningBytes(chainId, PE.pub, acctPE.nonce, "1000", forced)).toString("base64") });
  check("denied sender refused by the node (sendTx)", fsend.status !== 200 && /blocked/.test(JSON.stringify(fsend.body)), fsend.body?.error?.message?.slice(0, 120));
  const cons = await get(`/v1/consent/${AG.pub}`);
  check("GET /v1/consent", cons.body.deny.some((x: any) => x.account === PE.pub) && cons.body.allow.some((x: any) => x.account === HU.pub));
  const inAG = await get(`/v1/inbox/${AG.pub}?box=blocked`);
  check("inbox: denied peer's thread is blocked", inAG.body.threads.some((t: any) => t.peer.account === PE.pub));
  await run(AG, "/v1/tx/set-consent", { items: [{ peer: PE.pub, state: "clear" }] }, "consent_clear");
  const g2 = await get(`/v1/dm/gate?from=${PE.pub}&to=${AG.pub}`);
  check("consent cleared: gate back to priced", g2.body.consent === null && g2.body.can_send === true && g2.body.required_pay === price, g2.body.reason);

  // 7. sealed DM with a well-framed dummy body to AG (has enc_key); refused to PL (no profile)
  const body = "01" + "01" + randomBytes(32).toString("hex") + randomBytes(24).toString("hex") + "2000" + randomBytes(32).toString("hex");
  const noenc = await post("/v1/tx/send-message", { from: HU.pub, to: PL.pub, mode: "sealed", body_hex: body });
  check("sealed DM to an account without enc_key refused 409", noenc.status === 409, noenc.body?.error?.message);
  const sealed = await run(HU, "/v1/tx/send-message", { to: AG.pub, mode: "sealed", body_hex: body }, "sealed_dm");
  const sm = await get(`/v1/messages/${sealed.built.message_id}`);
  check("sealed DM: no text, body_hex kept, metadata note", sm.body.body === null && sm.body.body_hex === body && sm.body.mode === "sealed" && /hides its text/.test(sm.body.metadata_note));

  // 8. channels: sealed paid channel, invite + join, paid join, key epoch, sealed message, removal, rekey
  const cc = await run(AG, "/v1/tx/channel-config", { name: `fog desk ${tag}`, about: "forecasts", policy: "paid", join_price: "1000000", mode: "sealed" }, "channel_create");
  const CH = cc.built.channel_id;
  const chv = await get(`/v1/channels/${CH}`);
  check("channel created (predicted id)", chv.status === 200 && chv.body.mode === "sealed" && chv.body.policy === "paid", `members ${chv.body?.member_count}`);
  await run(AG, "/v1/tx/channel-member", { channel: CH, action: "invite", member: M1.pub }, "channel_invite");
  await run(M1, "/v1/tx/channel-member", { channel: CH, action: "join" }, "channel_join_invited");
  const short = await post("/v1/tx/channel-member", { from: M2.pub, channel: CH, action: "join", pay: "1" });
  check("paid join short refused 409", short.status === 409, short.body?.error?.message);
  const pj = await run(M2, "/v1/tx/channel-member", { channel: CH, action: "join" }, "channel_join_paid");
  check("paid join pays the join price by default", pj.built.window_pearl.params.transaction.pay === "1000000");
  const nokey = await post("/v1/tx/send-message", { from: M1.pub, channel: CH, mode: "sealed", body_hex: "02" + "01000000" + "44".repeat(24) + "55".repeat(16) });
  check("sealed channel message before a key refused 409", nokey.status === 409, nokey.body?.error?.message);
  const env = (m: Key) => ({ member: m.pub, envelope_hex: randomBytes(104).toString("hex") });
  await run(AG, "/v1/tx/channel-key", { channel: CH, epoch: 1, envelopes: [env(AG), env(M1), env(M2)] }, "channel_key_epoch1");
  const cm1 = await run(M1, "/v1/tx/send-message", { channel: CH, mode: "sealed", body_hex: "02" + "01000000" + randomBytes(24).toString("hex") + randomBytes(30).toString("hex") }, "channel_msg_epoch1");
  const k1 = await get(`/v1/channels/${CH}/key?account=${M2.pub}&epoch=1`);
  check("GET /v1/channels/{id}/key", k1.status === 200 && k1.body.envelope_hex.length === 208);
  await run(AG, "/v1/tx/channel-member", { channel: CH, action: "remove", member: M2.pub }, "channel_remove");
  const rk = await post("/v1/tx/send-message", { from: M1.pub, channel: CH, mode: "sealed", body_hex: "02" + "01000000" + "44".repeat(24) + "55".repeat(16) });
  check("after a removal: messages refused until a rekey", rk.status === 409 && /new key epoch/.test(rk.body.error.message), rk.body?.error?.message);
  const kr = await post("/v1/tx/channel-key", { from: AG.pub, channel: CH, epoch: 2, envelopes: [env(M2)] });
  check("rekey naming the removed member refused 409", kr.status === 409, kr.body?.error?.message);
  await run(AG, "/v1/tx/channel-key", { channel: CH, epoch: 2, envelopes: [env(AG), env(M1)] }, "channel_key_epoch2");
  const old = await post("/v1/tx/send-message", { from: M1.pub, channel: CH, mode: "sealed", body_hex: "02" + "01000000" + "44".repeat(24) + "55".repeat(16) });
  check("message sealed under the old epoch refused 409", old.status === 409, old.body?.error?.message);
  const cm2 = await run(M1, "/v1/tx/send-message", { channel: CH, mode: "sealed", body_hex: "02" + "02000000" + randomBytes(24).toString("hex") + randomBytes(30).toString("hex") }, "channel_msg_epoch2");
  const chd = await get(`/v1/channels/${CH}`);
  check("channel detail: members, epochs, rekey cleared", chd.body.member_count === 2 && chd.body.epoch === 2 && chd.body.rekey_due === false && chd.body.epochs.length === 2 && chd.body.members.every((m: any) => m.account.account !== M2.pub), JSON.stringify({ members: chd.body.member_count, epoch: chd.body.epoch }));
  const chm = await get(`/v1/channels/${CH}/messages`);
  check("channel messages newest first with epochs", chm.body.messages.length === 2 && chm.body.messages[0].epoch === 2 && chm.body.messages[1].epoch === 1 && chm.body.messages[0].message_id === cm2.built.message_id && chm.body.messages[1].message_id === cm1.built.message_id);
  const chl = await get("/v1/channels?limit=200");
  check("GET /v1/channels lists it", chl.status === 200 && chl.body.channels.some((c: any) => c.channel_id === CH), `total ${chl.body.total}`);
  const m1in = await get(`/v1/inbox/${M1.pub}`);
  check("inbox lists the joined channel", m1in.body.channels.some((c: any) => c.channel_id === CH));

  // 9. the remaining reads and the stream
  const th = await get(`/v1/threads/${AG.pub}/${PY.pub}?limit=1`);
  check("GET /v1/threads paging", th.status === 200 && th.body.messages.length === 1 && th.body.next !== null && th.body.gate && th.body.consent);
  const st = await get(`/v1/dm/stats/${AG.pub}`);
  check("GET /v1/dm/stats", st.status === 200 && st.body.answered >= 1 && st.body.received >= 5, JSON.stringify(st.body));
  const enc = await post("/v1/tx/encode", { from: HU.pub, transaction: { kind: "SetConsent", items: [{ peer: AG.pub, state: 1 }] } });
  check("POST /v1/tx/encode a DM kind", enc.status === 200 && /consent/i.test(enc.body.plain));
  await sleep(2000);
  ac.abort();
  await sseDone;
  check("SSE: pending event", /event: pending\n/.test(sse));
  check("SSE: message event", /event: message\n/.test(sse));
  check("SSE: escrow events (open, released, refunded)", /event: escrow\ndata: .*"state":"released"/.test(sse) && /"state":"refunded"/.test(sse));

  const tip = (await rpc("getChainInfo", {})).height;
  console.log(JSON.stringify({ run: tag, chain_id: chainId, tip, pass, fail, txs: log }, null, 1));
  void blake3;
  void hex;
  process.exit(fail ? 1 : 0);
}

main().catch((e) => {
  console.log(`FAIL aborted: ${(e as Error).message}`);
  console.log(JSON.stringify({ pass, fail: fail + 1, txs: log }, null, 1));
  process.exit(1);
});
