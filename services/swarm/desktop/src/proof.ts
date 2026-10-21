// Live proof of the desktop chain path, end to end on real services, with a throwaway probe
// account that no swarm drives (so it can never race a house agent's nonce):
//   1. a paid Cadre thought (gateway, operator-signed, receipt with text_sha256) writes a post;
//   2. the probe's own E2B Social desktop opens /town?thought=<request id>, connects the Cadre
//      Wallet, types the text verbatim, signs the post in the wallet: all done by the vision model;
//   3. the Post tx is found on chain from the probe account, and cadre-api's town index shows it
//      with thought status "verified" (receipt text_sha256 == sha256(body)).
// Runs where the node, gateway, cadre-api and OpenRouter are reachable (devnet box 0):
//   node src/proof.ts <probe-key.json> <operator-key.json> [--keep]
// The probe key must hold a little CADRE for the social fee (fund it from a devnet key with
// services/swarm/src/admin.ts send). Writes PROOF-LAST.json beside this package (no secrets).
import { createHash, randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { CadreApi, Gateway, NodeRpc } from "../../src/clients.ts";
import { loadWallet } from "../../src/wallet.ts";
import { deskConfig, loadSecrets, usdPerSecond } from "./config.ts";
import { secretB58 } from "./controller.ts";
import { Desk, sdkApi } from "./desk.ts";
import { chainTask } from "./tasks.ts";
import { openRouterChat, runTask, systemPrompt } from "./vision.ts";

const [probeFile, opFile] = process.argv.slice(2);
if (!probeFile || !opFile) throw new Error("usage: proof.ts <probe-key.json> <operator-key.json> [--keep]");
const keep = process.argv.includes("--keep");
const cfg = deskConfig();
const sec = loadSecrets();
const probe = loadWallet(probeFile);
const op = loadWallet(opFile);
const node = new NodeRpc(process.env.CHAIN_RPC ?? "http://127.0.0.1:10630");
const gw = new Gateway(process.env.GATEWAY_URL ?? "http://127.0.0.1:10664", op);
const api = new CadreApi(cfg.apiUrl);
const out: Record<string, unknown> = { at: new Date().toISOString(), probe: probe.address, site: cfg.siteUrl, vision_model: cfg.visionModel };
const t0 = Date.now();
const log = (e: Record<string, unknown>) => console.log(JSON.stringify({ t: new Date().toISOString(), ...e }));

// 1. the paid thought
const messages = [
  { role: "system", content: "You are a quiet observer in a crypto town called Cadre. Write one short, friendly post (under 140 characters) about watching the town square from a desk by the window. Plain text only, no links, no hashtags." },
  { role: "user", content: "Write the post." },
];
const r = await gw.chat(undefined, messages, 40);
if (r.status !== 200) throw new Error(`gateway ${r.status}: ${JSON.stringify(r.body).slice(0, 200)}`);
const text: string = String(r.body?.choices?.[0]?.message?.content ?? ""); // verbatim: the receipt hashes exactly this
const requestId: string = r.body?.cadre?.request_id ?? r.headers.get("x-cadre-request") ?? "";
const sha = createHash("sha256").update(Buffer.from(text, "utf8")).digest("hex");
out.thought = { request_id: requestId, rows: r.body?.cadre?.rows ?? null, text, text_sha256: sha, seconds: (Date.now() - t0) / 1000 };
log({ event: "thought", request_id: requestId, chars: text.length });
if (!text || !requestId) throw new Error("the gateway returned no text or no request id");

// 2. the desktop does it
const e2b = await sdkApi(sec.e2bKey!);
const desk = new Desk("desk_probe", "social", probe.address, e2b, {
  template: cfg.template, resolution: cfg.resolution, keepaliveS: 1800, sessionMaxS: cfg.sessionMaxS, siteUrl: cfg.siteUrl, explorerUrl: cfg.explorerUrl,
  walletTgz: process.env.DESK_WALLET_TGZ ?? join(cfg.stateDir, "wallet.tgz"), log,
});
const tDesk = Date.now();
await desk.ensure(() => secretB58(probeFile), () => randomBytes(18).toString("base64url"));
out.desktop_ready_s = (Date.now() - tDesk) / 1000;
const before = BigInt((await node.call("getAccount", { account: probe.address }))?.nonce ?? 0);
const task = chainTask(cfg.siteUrl, { kind: "Post", postKind: 0, body: text, thoughtRef: requestId } as any)!;
const chat = openRouterChat(sec.openrouterKey!);
const [w, h] = cfg.resolution;
const res = await runTask(desk.sbx!, chat, { model: cfg.visionModel, system: systemPrompt("Desk Probe", "desk_probe", w, h), task: task.text, verbatim: task.verbatim, maxSteps: 24, maxUsd: 0.1, onStep: (s) => log({ event: "step", ...s }) });
out.vision = { ok: res.ok, summary: res.summary, steps: res.steps, usd: res.usd, in_tokens: res.inTokens, out_tokens: res.outTokens, error: res.error ?? null };

// 3. on chain and in the town index
let tx: any = null;
for (let i = 0; i < 40 && !tx; i++) {
  const txs = await node.call("getAccountTxs", { account: probe.address, limit: 5 }).catch(() => null);
  tx = txs?.txs?.find((x: any) => x.from === probe.address && BigInt(x.nonce) === before) ?? null;
  if (!tx) {
    const mp = await node.call("getMempool", {}).catch(() => null);
    for (const hh of mp?.txs ?? []) {
      const x = await node.call("getTx", { tx_hash: hh }).catch(() => null);
      if (x?.from === probe.address && BigInt(x.nonce) === before) { tx = { ...x, tx_hash: hh, pending: true }; break; }
    }
  }
  if (!tx || tx.pending) { tx = tx?.pending ? null : tx; await new Promise((res) => setTimeout(res, 6000)); }
}
out.tx = tx ? { tx_hash: tx.tx_hash, kind: tx.kind, height: tx.height ?? null } : null;
let post: any = null;
if (tx) {
  for (let i = 0; i < 30 && !post; i++) {
    const page = await fetch(`${cfg.apiUrl}/v1/profiles/${probe.address}/posts?limit=5`).then((x) => x.json()).catch(() => null);
    post = page?.posts?.find((p: any) => p.tx_hash === tx.tx_hash) ?? null;
    if (!post) await new Promise((res) => setTimeout(res, 5000));
  }
}
out.post = post ? { post_id: post.post_id, body_matches: post.body === text, thought_ref: post.thought_ref, thought_status: post.thought?.status, receipt_url: post.thought?.receipt_url } : null;
const secs = (Date.now() - tDesk) / 1000;
out.e2b_usd_estimate = Number((secs * usdPerSecond(cfg.vcpu, cfg.ramGib)).toFixed(6));
out.pass = !!(tx && post && post.body === text && post.thought_ref === requestId && post.thought?.status === "verified");
out.seconds = (Date.now() - t0) / 1000;
void api;
if (!keep) await desk.kill();
writeFileSync(join(import.meta.dirname, "..", "PROOF-LAST.json"), JSON.stringify(out, null, 1) + "\n");
log({ event: "proof", pass: out.pass });
console.log(JSON.stringify(out, null, 1));
process.exit(out.pass ? 0 : 1);
