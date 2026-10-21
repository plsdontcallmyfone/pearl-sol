// DM live check driver (not a test file). Against a node with the DM kinds (the box's DM test chain)
// and a swarm running there with SWARM_KEYS=$KEYS:
//   node test/live/live-dms.ts setup              fund the operator and a test "human" key (BridgeUnlock signed
//                                                 by the localnet attester key $ATTESTER_KEY), human SetProfile with its enc_key
//   node test/live/live-dms.ts paid               plain escrowed DM to agent 0, sealed escrowed DM to agent 1; waits for the
//                                                 replies; prints heights, tx hashes, balances, escrow state, the opened sealed reply
//   node test/live/live-dms.ts declined <i>       an escrowed DM to agent i; waits for the agent's ReclaimMessage; prints the refund
// Keys stay in $KEYS (0600 files, outside the repo); nothing secret is printed.
import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { b58encode } from "../../src/b58.ts";
import { decodeTx, feeFor, frame, messageId, NS_TX, signTx, type Body } from "../../src/chaintx.ts";
import { CadreApi, NodeRpc } from "../../src/clients.ts";
import { sealDm } from "../../src/dmcrypto.ts";
import { PERSONAS } from "../../src/personas.ts";
import { createKeyFile, loadWallet, type Wallet } from "../../src/wallet.ts";

const env = process.env;
const node = new NodeRpc(env.CHAIN_RPC ?? "http://127.0.0.1:11730");
const api = new CadreApi(env.CADRE_API ?? "http://127.0.0.1:11750");
const KEYS = env.KEYS ?? resolve(env.HOME ?? ".", "dms-swarm/keys");
const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const out = (event: string, x: Record<string, unknown>) => console.log(JSON.stringify({ t: new Date().toISOString(), event, ...x }));

let params: any, chainId = 0;
async function init() {
  params = await node.call("getParams", {});
  chainId = Number((await node.call("getChainInfo", {})).chain_id);
}
const bal = async (a: string) => BigInt((await node.call("getAccount", { account: a }))?.balance ?? 0);
const height = async () => Number((await node.call("getChainInfo", {})).height);

async function send(w: Wallet, body: Body): Promise<{ tx: string; nonce: bigint }> {
  const acc = await node.call("getAccount", { account: w.address });
  const nonce = BigInt(acc?.nonce ?? 0);
  const r = await node.sendTx(signTx(w, chainId, nonce, feeFor(body, params), body));
  return { tx: r.tx_hash, nonce };
}

async function included(tx: string, timeoutMs = 120_000): Promise<any> {
  const t0 = Date.now();
  for (;;) {
    const t = await node.call("getTx", { tx_hash: tx });
    if (t?.status === "included") return t;
    if (Date.now() - t0 > timeoutMs) throw new Error(`tx ${tx} not included in ${timeoutMs} ms`);
    await sleep(1000);
  }
}

/** BridgeUnlock (kind 6) to `to`, attested by the localnet attester key, sent by a throwaway relayer key with fee 0. */
async function unlock(to: Uint8Array, amountSpl: bigint) {
  const att = loadWallet(env.ATTESTER_KEY ?? resolve(env.HOME ?? ".", "dms-build/keys/localnet/attester-keypair.json"));
  createKeyFile(resolve(KEYS, "relayer.json"));
  const rel = loadWallet(resolve(KEYS, "relayer.json"));
  const sn = BigInt(Date.now()) * 1000n + BigInt(randomBytes(1)[0]);
  const u64 = (v: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(v); return b; };
  const msg = Buffer.concat([Buffer.from("pearl-sol:deposit:v1"), u64(sn), to, u64(amountSpl)]);
  const sig = att.sign(msg);
  const bodyBytes = Buffer.concat([u64(sn), to, u64(amountSpl), Buffer.from([0, 1]), att.publicKey, sig]);
  const nonce = BigInt((await node.call("getAccount", { account: rel.address }))?.nonce ?? 0);
  const c = Buffer.alloc(4); c.writeUInt32LE(chainId);
  const signing = Buffer.concat([Buffer.from([1]), c, rel.publicKey, u64(nonce), u64(0n), Buffer.from([6]), bodyBytes]);
  const tx = Buffer.concat([signing, rel.sign(frame(NS_TX, signing))]);
  const r = await node.sendTx(tx);
  await included(r.tx_hash);
  out("bridge_unlock", { to: b58encode(to), amount_spl: amountSpl.toString(), tx: r.tx_hash });
}

function agents(): Wallet[] {
  const n = Number(env.SWARM_AGENTS ?? 3);
  return PERSONAS.slice(0, n).map((p) => loadWallet(resolve(KEYS, `agent-${p.handle}.json`)));
}

async function setup() {
  const op = loadWallet(resolve(KEYS, "operator.json"));
  if ((await bal(op.address)) < 5_000_000_000n) await unlock(op.publicKey, 10_000_000n); // 10 CADRE
  createKeyFile(resolve(KEYS, "human.json"));
  const h = loadWallet(resolve(KEYS, "human.json"));
  if ((await bal(h.address)) < 5_000_000_000n) await unlock(h.publicKey, 10_000_000n);
  const p = await node.call("getProfile", { account: h.address });
  if (p?.enc_key !== b58encode(h.encKey)) {
    const r = await send(h, { kind: "SetProfile", handle: "", name: "DM live-check human", profileUri: "", profileHash: new Uint8Array(32), flags: 0, operator: new Uint8Array(32), encKey: h.encKey });
    await included(r.tx);
    out("human_profile", { account: h.address, enc_key: b58encode(h.encKey), tx: r.tx });
  }
  out("setup_done", { operator: op.address, operator_balance: (await bal(op.address)).toString(), human: h.address, human_balance: (await bal(h.address)).toString() });
}

async function waitDmProfile(a: Wallet) {
  for (let i = 0; i < 120; i++) {
    const p = await node.call("getProfile", { account: a.address });
    if (p?.enc_key === b58encode(a.encKey)) return p;
    await sleep(2000);
  }
  throw new Error(`agent ${a.address} never published its enc_key`);
}

async function dm(h: Wallet, a: Wallet, text: string, sealed: boolean, reclaimAfter: number) {
  const gate = await node.call("getDmGate", { from: h.address, to: a.address });
  const pay = BigInt(gate.required_pay);
  const body = sealed ? sealDm({ from: h.publicKey, to: a.publicKey, toEncKey: a.encKey, selfEncKey: h.encKey, text }) : Uint8Array.from(Buffer.from(text));
  const before = { human: await bal(h.address), agent: await bal(a.address) };
  const r = await send(h, { kind: "SendMessage", to: a.publicKey, mode: sealed ? 1 : 0, pay, reclaimAfter: pay > 0n ? reclaimAfter : 0, body });
  const t = await included(r.tx);
  const id = hex(messageId(h.publicKey, r.nonce));
  out("dm_sent", { message_id: id, to: a.address, mode: sealed ? "sealed" : "plain", pay: pay.toString(), gate_reason: gate.reason, reclaim_after: reclaimAfter, tx: r.tx, height: t.height, escrow: await node.call("getMessageEscrow", { message_id: id }), human_balance_before: before.human.toString(), human_balance_after: (await bal(h.address)).toString(), agent_balance_before: before.agent.toString() });
  return { id, pay, before, sentHeight: Number(t.height) };
}

/** The agent's SendMessage with reply_to = id, from the human's account txs. */
async function waitReply(h: Wallet, a: Wallet, id: string, timeoutMs: number) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    const r = await node.call("getAccountTxs", { account: h.address, limit: 50 });
    for (const t of r?.txs ?? []) {
      if (t.from !== a.address || t.kind !== "SendMessage") continue;
      const d = decodeTx(Uint8Array.from(Buffer.from(t.tx, "base64")));
      if (d.body.kind === "SendMessage" && hex(d.body.replyTo!) === id) return { t, d };
    }
    await sleep(2000);
  }
  throw new Error(`no reply to ${id} in ${timeoutMs} ms`);
}

async function paid() {
  const h = loadWallet(resolve(KEYS, "human.json"));
  const [a0, a1] = agents();
  for (const a of [a0, a1]) await waitDmProfile(a);
  const plain = await dm(h, a0, "Good evening. What did you see on the water today?", false, 120);
  const sealed = await dm(h, a1, "A private question: is the old lighthouse still lit?", true, 120);
  for (const [a, m, mode] of [[a0, plain, "plain"], [a1, sealed, "sealed"]] as const) {
    const { t, d } = await waitReply(h, a, m.id, 300_000);
    const b = d.body as any;
    const after = await bal(a.address);
    const fee = d.fee;
    const text = b.mode === 1 ? h.openDm(b.body, a.publicKey, h.publicKey) : Buffer.from(b.body).toString("utf8");
    const viaApi = await api.get(`/v1/messages/${m.id}`).catch(() => null);
    out("dm_answered", {
      mode, message_id: m.id, reply_tx: t.tx_hash, reply_height: t.height, sent_height: m.sentHeight, reply_mode: b.mode === 1 ? "sealed" : "plain", thought_ref: b.thoughtRef || null,
      reply_text: text, opened_by_sender: b.mode === 1 ? text !== null : null,
      escrow_after: await node.call("getMessageEscrow", { message_id: m.id }), api_escrow: viaApi?.body?.escrow ?? null,
      agent_balance_before: m.before.agent.toString(), agent_balance_after: after.toString(), reply_fee: fee.toString(), pay: m.pay.toString(),
      agent_delta_plus_fee: (after - m.before.agent + fee).toString(),
    });
  }
}

async function declined(i: number) {
  const h = loadWallet(resolve(KEYS, "human.json"));
  const a = agents()[i];
  await waitDmProfile(a);
  const m = await dm(h, a, "Please answer this one too, it is paid.", false, 200);
  const afterSend = await bal(h.address);
  const t0 = Date.now();
  while (Date.now() - t0 < 300_000) {
    const r = await node.call("getAccountTxs", { account: a.address, limit: 30 });
    for (const t of r?.txs ?? []) {
      if (t.from !== a.address || t.kind !== "ReclaimMessage") continue;
      const d = decodeTx(Uint8Array.from(Buffer.from(t.tx, "base64")));
      if (d.body.kind === "ReclaimMessage" && hex(d.body.messageId) === m.id) {
        const now = await bal(h.address);
        const viaApi = await api.get(`/v1/messages/${m.id}`).catch(() => null);
        out("dm_declined", { message_id: m.id, reclaim_tx: t.tx_hash, reclaim_height: t.height, sent_height: m.sentHeight, deadline: m.sentHeight + 200, pay: m.pay.toString(), human_balance_after_send: afterSend.toString(), human_balance_after_refund: now.toString(), refunded: (now - afterSend).toString(), escrow_after: await node.call("getMessageEscrow", { message_id: m.id }), api_escrow: viaApi?.body?.escrow ?? null });
        return;
      }
    }
    await sleep(2000);
  }
  throw new Error("no ReclaimMessage from the agent");
}

await init();
const [cmd, arg] = process.argv.slice(2);
if (!existsSync(KEYS)) throw new Error(`no key dir ${KEYS}`);
if (cmd === "setup") await setup();
else if (cmd === "paid") await paid();
else if (cmd === "declined") await declined(Number(arg ?? 2));
else throw new Error("usage: setup | paid | declined <i>");
