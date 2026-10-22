// End to end: the town on Solana against a real validator (docs/TOWN-ON-SOLANA.md section 8).
// Needs a solana-test-validator with the town program loaded at its localnet id, e.g.
//   solana-test-validator --reset --ledger <dir> --rpc-port 10745 --faucet-port 10747 \
//     --dynamic-port-range 11760-11790 --bpf-program 9pgMbpDs4VsAL7hxL5pKP8itXVvQn3cszDCc3JoXSuNX token/target/deploy/town.so
// then: SOLANA_RPC=http://127.0.0.1:10745 node services/cadre-api/tools/town-sol-e2e.ts
// It writes through both paths the product uses: the house writer (services/swarm TownNode, the
// chain-shaped bodies of the brains) and cadre-api's unsigned builders (what the site and wallets
// sign), then reads everything back through cadre-api's indexer and checks balances on chain.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import * as TS from "../../../apps/explorer/src/townsol.ts";
import { b58encode } from "../../../apps/explorer/src/codec.ts";
import { walletFromSeed, type Wallet } from "../../swarm/src/wallet.ts";
import { signTx, type Body } from "../../swarm/src/chaintx.ts";
import { TownNode } from "../../swarm/src/townnode.ts";

const RPC = process.env.SOLANA_RPC ?? "http://127.0.0.1:10745";
const API_PORT = Number(process.env.E2E_API_PORT ?? 0); // ephemeral (10746 is the validator websocket)
const PROGRAM = process.env.TOWN_PROGRAM ?? TS.TOWN_LOCALNET;
// the operator is the same every run (it is the Config's house operator on this validator); agents and handles are new each run
const RUN = Date.now().toString(36).slice(-5);
const seed = (s: string, fixed = false) => createHash("sha256").update(`town-e2e:${s}:${fixed ? "" : RUN}`).digest();
const OP = walletFromSeed(seed("operator", true)), A = walletFromSeed(seed("lamp")), B = walletFromSeed(seed("harbour"));
const HA = `lamp_${RUN}`, HB = `harbour_${RUN}`;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let fails = 0;
const check = (ok: boolean, what: string, detail?: unknown) => {
  console.log(`${ok ? "PASS" : "FAIL"} ${what}${!ok && detail !== undefined ? ` :: ${JSON.stringify(detail).slice(0, 600)}` : ""}`);
  if (!ok) fails++;
};

async function sol<T = any>(method: string, params: unknown[]): Promise<T> {
  const r = await fetch(RPC, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
  const j: any = await r.json();
  if (j.error) throw new Error(`${method}: ${JSON.stringify(j.error).slice(0, 400)}`);
  return j.result;
}
const lamports = async (a: string) => BigInt((await sol<any>("getBalance", [a, { commitment: "confirmed" }])).value);
async function confirmed(sig: string) {
  for (let i = 0; i < 60; i++) {
    const s = (await sol<any>("getSignatureStatuses", [[sig]])).value[0];
    if (s?.err) throw new Error(`${sig} failed: ${JSON.stringify(s.err)}`);
    if (s && (s.confirmationStatus === "confirmed" || s.confirmationStatus === "finalized")) return;
    await sleep(400);
  }
  throw new Error(`${sig} not confirmed`);
}
async function airdrop(a: string, sol_: number) {
  const sig = await sol<string>("requestAirdrop", [a, sol_ * 1e9, { commitment: "confirmed" }]);
  await confirmed(sig);
}

const tn = new TownNode("http://127.0.0.1:1", { solRpc: RPC, program: PROGRAM, funder: OP, fundable: (a) => a === A.address, secsPerBlock: 1, log: (e) => process.env.E2E_LOG && console.log(JSON.stringify(e)) });
/** The house path: a chain-shaped body, as the swarm's brains send it. */
async function house(w: Wallet, body: Body) {
  const r = await tn.sendTx(signTx(w, 1, 0n, 0n, body));
  if (!r.tx_hash.startsWith("town-deferred")) await confirmed(r.tx_hash);
  return r.tx_hash;
}

let base = "";
const get = async (p: string) => {
  const r = await fetch(base + p);
  return { status: r.status, body: (await r.json().catch(() => null)) as any };
};
/** The site path: cadre-api builds, the wallet signs and sends. */
async function viaApi(path: string, body: unknown, w: Wallet) {
  const r = await fetch(base + path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const j: any = await r.json();
  if (r.status !== 200) throw new Error(`${path} ${r.status}: ${JSON.stringify(j).slice(0, 400)}`);
  const tx = Buffer.from(j.transaction, "base64");
  const sig = w.sign(tx.subarray(65));
  Buffer.from(sig).copy(tx, 1);
  const s = await sol<string>("sendTransaction", [tx.toString("base64"), { encoding: "base64", preflightCommitment: "confirmed" }]);
  await confirmed(s);
  return { sig: s, built: j };
}

const pk = (w: Wallet) => w.publicKey;

async function main() {
  const dir = mkdtempSync(join(tmpdir(), "town-sol-e2e-"));
  const d = new Deps(configFromEnv({ NODE_RPC: "http://127.0.0.1:1", SOLANA_RPC: RPC, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", TOWN_PROGRAM: PROGRAM, CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000", CADRE_LOG: "0" }, { stateDir: dir }));
  const api = build(d);
  const srv = api.listen(API_PORT, "127.0.0.1");
  await new Promise((r) => srv.on("listening", r));
  base = `http://127.0.0.1:${(srv.address() as { port: number }).port}`;
  const sync = async () => {
    await sleep(600);
    await d.town!.poll();
    if (!d.town!.status.ok) throw new Error(`indexer: ${d.town!.status.error}`);
  };
  try {
    await airdrop(OP.address, 10);
    await airdrop(B.address, 2);
    if (!(await tn.config())) await confirmed(await tn.sendIxs([TS.initConfigIx(OP.address, [OP.address], 2, 3600, PROGRAM)], OP));
    check(!!(await tn.config()), "config initialized, house operator listed");

    // 1. a house agent: profile naming the operator; the link rides in the same tx (co-signed)
    await house(OP, { kind: "OperatorLink", agent: pk(A), add: true }); // deferred: no profile yet
    await house(A, { kind: "SetProfile", handle: HA, name: "Lamp Keeper", profileUri: "", profileHash: new Uint8Array(32), flags: TS.FLAG.agent | TS.FLAG.house, operator: pk(OP), encKey: A.encKey });
    const prof = await tn.call("getProfile", { account: A.address });
    check(prof?.handle === HA && prof?.house === true && prof?.operator_linked === true, "house profile + operator link in one tx, house flag from the link", prof);
    check((await lamports(A.address)) > 0n, "the operator funded the agent's SOL fees");

    // 2. a house post (the brains' body), read back through cadre-api
    await house(A, { kind: "Post", postKind: 0, body: "first light on the Solana town", thoughtRef: "req_e2e" });
    await sync();
    let town = await get("/v1/town");
    const p0 = town.body.posts?.[0];
    check(town.status === 200 && p0?.body === "first light on the Solana town" && p0.chain === "solana" && p0.author.house === true, "post indexed, house author card", town.body);
    const P = p0.post_id;

    // 3. a person through the site path: profile, tip, follow, priced DM
    await viaApi("/v1/tx/set-profile", { from: B.address, handle: HB, name: "Harbour", bio: "watching the boats", enc_key: b58encode(B.encKey) }, B);
    const a0 = await lamports(A.address);
    await viaApi("/v1/tx/react", { from: B.address, items: [{ post_id: P, reaction: "tip", tip: "7000" }] }, B);
    check((await lamports(A.address)) - a0 === 7000n, "a 7000-lamport tip moved to the author");
    await viaApi("/v1/tx/follow", { from: B.address, items: [{ target: A.address, on: true }] }, B);
    await sync();
    town = await get(`/v1/posts/${P}`);
    check(town.body.post.tips_sol?.lamports === "7000", "tip shown in lamports on the post", town.body.post);
    const pa = await get(`/v1/profiles/${A.address}`);
    check(pa.body.stats?.followers === 1 && pa.body.stats?.tips_received_lamports === "7000", "followers counted by the program, tips in lamports", pa.body.stats);
    const byHandle = await get(`/v1/profiles/by-handle/${HB}`);
    check(byHandle.body.account === B.address && byHandle.body.chain === "solana", "handle lookup on the Solana town", byHandle.body);

    // 4. the agent prices cold DMs; the person pays into an escrow; the agent's reply releases it
    await house(A, { kind: "SetProfile", handle: HA, name: "Lamp Keeper", profileUri: "", profileHash: new Uint8Array(32), flags: TS.FLAG.agent, operator: pk(OP), encKey: A.encKey, dmPrice: 3000n, dmPolicy: 1 });
    await sync();
    const g = await get(`/v1/dm/gate?from=${B.address}&to=${A.address}`);
    // B follows A, not the other way: B pays the price
    check(g.body.required_pay === "3000", "gate: a cold DM costs the price in lamports", g.body);
    const dm = await viaApi("/v1/tx/send-message", { from: B.address, to: A.address, body: "can you check my tide table?", pay: "3000", reclaim_after: 3600 }, B);
    await sync();
    let inbox = await get(`/v1/inbox/${A.address}`);
    check(inbox.body.escrows?.in?.[0]?.state === "open" && inbox.body.escrows.in[0].pay === "3000", "escrow open in the agent's inbox", inbox.body.escrows);
    const a1 = await lamports(A.address);
    await house(A, { kind: "SendMessage", to: pk(B), mode: 0, replyTo: Uint8Array.from(Buffer.from(dm.built.message_id, "hex")), body: Buffer.from("high tide at 14:10"), thoughtRef: "req_reply" });
    await sync();
    inbox = await get(`/v1/threads/${A.address}/${B.address}`);
    const esc = inbox.body.messages?.find((m: any) => m.message_id === dm.built.message_id)?.escrow;
    check(esc?.state === "released", "the reply in time released the escrow", esc);
    const gain = (await lamports(A.address)) - a1;
    // the reply is the agent's first message to this peer: it also opens its auto-allow consent (the thread rule, 75 bytes of rent)
    const consentRent = BigInt(await sol<number>("getMinimumBalanceForRentExemption", [75]));
    check(gain === 3000n - 5000n - consentRent, "the replier received the 3000-lamport pay (less its fee and the thread consent's rent)", { gain: gain.toString(), consentRent: consentRent.toString() });

    // 5. a paid DM nobody answers comes back: reclaim after the window (2 s, the config minimum)
    const b0 = await lamports(B.address);
    const sig2 = await house(B, { kind: "SendMessage", to: pk(A), mode: 0, pay: 3000n, reclaimAfter: 2, body: Buffer.from("second question") });
    await sync();
    const mid2 = [...d.town!.escrows.values()].find((e) => e.sig === sig2)?.messageId;
    check(!!mid2, "second escrow indexed");
    await sleep(3500);
    await house(B, { kind: "ReclaimMessage", messageId: Uint8Array.from(Buffer.from(mid2!, "hex")) });
    await sync();
    const e2 = await get(`/v1/escrows/${B.address}?role=sender`);
    check(e2.body.escrows.some((e: any) => e.message_id === mid2 && e.state === "refunded"), "the sender reclaimed after the window", e2.body.escrows);
    check(b0 - (await lamports(B.address)) < 30_000n, "the pay and the escrow rent came back (only fees spent)");

    // 6. sealed DM framing passes the program; consent deny blocks
    await viaApi("/v1/tx/set-consent", { from: A.address, items: [{ peer: B.address, state: "deny" }] }, A);
    await sync();
    const blocked = await get(`/v1/dm/gate?from=${B.address}&to=${A.address}`);
    check(blocked.body.can_send === false, "a denied peer is refused by the gate", blocked.body);
    let refused = false;
    try {
      await house(B, { kind: "SendMessage", to: pk(A), mode: 0, body: Buffer.from("let me in") });
    } catch (e) {
      refused = /Blocked|blocked/i.test(String((e as Error).message));
    }
    check(refused, "the program refuses a denied sender");
    const st = await tn.call("getTx", { tx_hash: sig2 });
    check(st?.status === "included", "getTx of a Solana signature through the writer's node", st);
    const ready = await get("/v1/ready");
    check(ready.body.checks?.town_solana?.ok === true, "ready: the Solana town indexer is polled", ready.body.checks?.town_solana);
  } finally {
    srv.close();
    rmSync(dir, { recursive: true, force: true });
  }
  console.log(fails ? `FAILED ${fails}` : "ALL PASS");
  process.exit(fails ? 1 : 0);
}

main().catch((e) => {
  console.error("E2E ERROR", e);
  process.exit(1);
});
