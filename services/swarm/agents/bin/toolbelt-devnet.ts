// Devnet end-to-end for the agent toolbelt (docs/TOOLBELT.md 11): a hosted web-research agent sells
// a report on the market; a buyer hires it; the agent searches and fetches the web, writes the report
// with paid inference, delivers it, and commits its tool receipts on Cadre (contracts/toolbelt).
//
//   node bin/toolbelt-devnet.ts create   new agent key + v2 spec (offer + toolbelt web/files), signed by
//                                        the creator, sealed to the runtime, funded (SOL + CADRE), linked
//   node bin/toolbelt-devnet.ts hire     a new buyer key, funded CADRE, hires the agent's offer
//   node bin/toolbelt-devnet.ts verify   reads the delivery, the receipt batch blob and the contract
//   node bin/toolbelt-devnet.ts accept   the buyer's verdict
// Env: RUNTIME (default http://127.0.0.1:10730), CADRE_API, CHAIN_RPC, SOL_RPC_FILE, CREATOR_KEY,
// FUNDER_KEY (CADRE), OUT (state JSON, default keys/devnet/toolbelt/e2e.json). Devnet only; keys are
// files under keys/devnet (gitignored) and never printed.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { b58decode } from "../../src/b58.ts";
import { authMessage, NodeRpc } from "../../src/clients.ts";
import { feeFor, signTx, frame, NS_TX } from "../../src/chaintx.ts";
import { blake3 } from "../../src/blake3.ts";
import { createKeyFile, loadWallet, type Wallet } from "../../src/wallet.ts";
import { contractSigningBytes, Rd, Abi } from "../../trading/src/cadre.ts";
import { defaultSpecV2, specMessage, specSha256 } from "../src/spec.ts";
import { defaultToolbelt } from "../src/toolbelt/spec.ts";
import { checkBatch } from "../src/toolbelt/receipts.ts";
import { seal } from "../src/seal.ts";

const REPO = new URL("../../../../", import.meta.url).pathname;
const env = process.env;
// devnet keys are gitignored and live in the main checkout; a worktree has none
const KEYS = env.KEYS_DIR ?? (existsSync(`${REPO}keys/devnet`) ? `${REPO}keys/devnet` : `${env.HOME}/pearl-sol/keys/devnet`);
const RUNTIME = env.RUNTIME ?? "http://127.0.0.1:10730";
const API = env.CADRE_API ?? "https://api.64-95-12-116.sslip.io";
const node = new NodeRpc(env.CHAIN_RPC ?? "https://rpc.64-95-12-116.sslip.io");
const SOL = readFileSync(env.SOL_RPC_FILE ?? `${KEYS}/solana-rpc-url`, "utf8").trim();
const CREATOR = env.CREATOR_KEY ?? `${KEYS}/useragents/user-test.json`;
const FUNDER = env.FUNDER_KEY ?? `${KEYS}/ops-keypair.json`;
const OUT = env.OUT ?? `${KEYS}/toolbelt/e2e.json`;
const st: any = existsSync(OUT) ? JSON.parse(readFileSync(OUT, "utf8")) : {};
const save = () => writeFileSync(OUT, JSON.stringify(st, null, 1), { mode: 0o600 });
const log = (x: unknown) => console.log(JSON.stringify(x));
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function api(method: string, path: string, body?: unknown) {
  const r = await fetch(`${API}${path}`, { method, headers: { "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(60_000) });
  const j: any = await r.json().catch(() => null);
  if (!r.ok) throw new Error(`${method} ${path}: ${r.status} ${JSON.stringify(j).slice(0, 300)}`);
  return j;
}

async function sendNative(w: Wallet, body: any): Promise<string> {
  const info = await node.call("getChainInfo");
  const params = await node.call("getParams");
  for (let i = 0; i < 8; i++) {
    const a = await node.call("getAccount", { account: w.address });
    try {
      const r = await node.sendTx(signTx(w, info.chain_id, BigInt(a?.nonce ?? 0), feeFor(body, params), body));
      return r.tx_hash;
    } catch (e) {
      if (!/nonce|preconfirmed|pooled/i.test((e as Error).message)) throw e;
      await sleep(5000); // the funder key is shared with other sessions: wait for its pooled tx
    }
  }
  throw new Error("nonce stayed busy");
}

async function waitTx(h: string) {
  for (let i = 0; i < 120; i++) {
    const t = await node.call("getTx", { tx_hash: h }).catch(() => null);
    if (t?.status === "included") return t.height as number;
    await sleep(3000);
  }
  throw new Error(`tx ${h} not included`);
}

async function market(w: Wallet, body: Record<string, unknown>) {
  const built = await api("POST", "/v1/tx/market", { from: w.address, ...body });
  const t = built.window_pearl.params.transaction;
  const info = await node.call("getChainInfo");
  const a = await node.call("getAccount", { account: w.address });
  const sb = contractSigningBytes(info.chain_id, w.publicKey, BigInt(a?.nonce ?? 0), BigInt(t.fee), { kind: "Call", contract: t.contract, value: BigInt(t.value), gasLimit: BigInt(t.gasLimit), args: Uint8Array.from(Buffer.from(t.args, "hex")) });
  const r = await node.sendTx(Uint8Array.from([...sb, ...w.sign(frame(NS_TX, sb))]));
  return { tx: r.tx_hash as string, review: built.review_text as string };
}

async function create() {
  const creator = loadWallet(CREATOR);
  const handle = `tb_research_${Date.now().toString(36).slice(-4)}`;
  const keyFile = `${KEYS}/toolbelt/agent-${handle}.json`;
  const { address } = createKeyFile(keyFile);
  const tb = defaultToolbelt(["web", "files"], { batch_every: 32 });
  const spec = defaultSpecV2(address, creator.address, {
    handle, name: "Toolbelt Researcher", persona: "A careful web researcher who answers with short, sourced reports.", appearance: "a magnifying glass over a page",
    goals: ["sell accurate sourced reports"], topics: ["research"], skills: ["market", "x402"],
    actions: { post: false, reply: false, react: false, follow: false, tip: false, trade: false, memo: false, hire: false, mine: false, browse: false, offer: true },
    offers: [{ title: "Web research report (toolbelt)", category: "research", price_units: "100000000", turnaround_blocks: 120, description: "Researches the question on the open web and writes a short report.", deliverable: "3 to 5 sentences answering the question, citing the sources [1] [2] [3]." }],
    budget: { sol_lamports_per_day: "120000000", cadre_units_per_day: "2000000000", rows_per_day: 8192 },
    created_ms: Date.now(), toolbelt: tb,
  });
  const sha = specSha256(spec);
  const sig = Buffer.from(creator.sign(Buffer.from(specMessage(spec, sha), "utf8"))).toString("hex");
  const info = await (await fetch(`${RUNTIME}/v1/hosted`)).json() as any;
  const seed = Uint8Array.from(JSON.parse(readFileSync(keyFile, "utf8")).slice(0, 32));
  const sealed = seal(seed, Buffer.from(info.seal_pub, "base64"));
  seed.fill(0);
  const body = JSON.stringify({ sealed, spec: { spec, sha256: sha, sig } });
  const path = "/v1/hosted/agents";
  const ts = Date.now();
  const r = await fetch(`${RUNTIME}${path}`, { method: "POST", headers: { "content-type": "application/json", "x-cadre-wallet": creator.address, "x-cadre-timestamp": String(ts), "x-cadre-signature": Buffer.from(creator.sign(authMessage("POST", path, ts, body))).toString("hex") }, body });
  const j: any = await r.json();
  if (r.status !== 201) throw new Error(`create: ${r.status} ${JSON.stringify(j).slice(0, 300)}`);
  Object.assign(st, { handle, agent: address, keyFile, spec_sha256: sha, created_ms: ts });
  save();
  log({ step: "created", handle, agent: address, spec_sha256: sha });
  // SOL from the creator's devnet wallet (inference x402 + the toolbelt top-up + fees)
  const out = execFileSync("solana", ["transfer", "-u", SOL, "-k", CREATOR, address, "0.25", "--allow-unfunded-recipient", "--output", "json"], { encoding: "utf8" });
  st.sol_funding = JSON.parse(out).signature;
  // CADRE for Cadre fees (receipts commits, offer, delivery) from the funder
  const funder = loadWallet(FUNDER);
  st.cadre_funding = await sendNative(funder, { kind: "Transfer", to: b58decode(address), amount: 3_000_000_000n });
  await waitTx(st.cadre_funding);
  // the creator's attribution link (OperatorLink on Cadre); the creator holds CADRE for its fee
  st.operator_link = await sendNative(creator, { kind: "OperatorLink", agent: b58decode(address), add: true });
  save();
  log({ step: "funded", sol: st.sol_funding, cadre: st.cadre_funding, operator_link: st.operator_link });
}

async function hire() {
  const { address } = createKeyFile(`${KEYS}/toolbelt/buyer.json`);
  const buyer = loadWallet(`${KEYS}/toolbelt/buyer.json`);
  st.buyer = address;
  const bal = await node.call("getAccount", { account: address });
  if (BigInt(bal?.balance ?? 0) < 500_000_000n) { st.buyer_funding = await sendNative(loadWallet(FUNDER), { kind: "Transfer", to: b58decode(address), amount: 1_000_000_000n }); await waitTx(st.buyer_funding); }
  const offers = await api("GET", `/v1/market/offers?provider=${st.agent}`);
  const o = offers.offers[0];
  if (!o) throw new Error("the agent has not listed its offer yet");
  const text = env.REQUEST ?? "What is proof of useful work, and what did Komargodski and Weinstein propose for matrix multiplication? Answer in 3 to 5 sentences with sources.";
  const req = Buffer.from(JSON.stringify({ format: "cadre-request-v1", offer: o.id, title: o.title, buyer: address, request: text }));
  const put = await node.putBlob(req, true);
  const s = await market(buyer, { action: "hire", offer: o.id, price: o.price, request_uri: put.uri, request_hash: Buffer.from(blake3(req)).toString("hex"), deadline_blocks: 400, judge_fee: "0", evaluators: [address], threshold: 1 });
  st.hire_tx = s.tx;
  save();
  log({ step: "hired", offer: o.id, tx: s.tx, review: s.review });
}

async function verify() {
  const hires = await api("GET", `/v1/market/hires?provider=${st.agent}`);
  const h = hires.hires.find((x: any) => x.buyer === st.buyer) ?? hires.hires[0];
  const d = await api("GET", `/v1/market/hires/${h.id}`);
  st.hire = h.id;
  const blob = await node.call("getBlob", { uri: d.hire?.result_uri ?? d.result_uri ?? h.result_uri });
  const del = JSON.parse(Buffer.from(blob.data, "base64").toString("utf8"));
  const tbx = del.toolbelt;
  const bb = await node.call("getBlob", { uri: tbx.batch.blob });
  const batch = JSON.parse(Buffer.from(bb.data, "base64").toString("utf8"));
  const problems = checkBatch(batch);
  const contract = tbx.contract;
  const q = await node.call("queryContract", { address: contract, args: Abi.method("get").addr(st.agent).hex() });
  const rd = new Rd(q.return);
  const onchain = { next_seq: rd.u64().toString(), batches: rd.u64().toString(), price_total: rd.u64().toString(), last_root: rd.hash(), last_blob: rd.hash(), last_height: rd.u64().toString() };
  const commit = await node.call("getTx", { tx_hash: tbx.batch.tx }).catch(() => null);
  st.verify = { hire: h.id, status: h.status, delivery_uri: d.hire?.result_uri ?? h.result_uri, thought_ref: del.thought_ref, sources: tbx.sources, receipts: batch.receipts.map((r: any) => ({ seq: r.seq, tool: r.tool, price_lamports: r.price_lamports, cost_usd: r.cost_usd, thought_ref: r.thought_ref, decided: r.decided })), batch_root: batch.root, batch_problems: problems, root_on_chain_matches: onchain.last_root === batch.root || Number(onchain.batches) > 1, onchain, commit_tx: tbx.batch.tx, commit_height: commit?.height ?? null };
  save();
  log({ step: "verified", ...st.verify, report: del.text });
}

async function accept() {
  const buyer = loadWallet(`${KEYS}/toolbelt/buyer.json`);
  const s = await market(buyer, { action: "verdict", hire: st.hire, accept: true });
  st.verdict_tx = s.tx;
  save();
  log({ step: "accepted", tx: s.tx, review: s.review });
}

const cmd = process.argv[2];
const f: Record<string, () => Promise<void>> = { create, hire, verify, accept };
if (!f[cmd]) { console.error("usage: create | hire | verify | accept"); process.exit(2); }
f[cmd]().catch((e) => { console.error(String(e?.stack ?? e).slice(0, 800)); process.exit(1); });
