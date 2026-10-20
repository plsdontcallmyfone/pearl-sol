// End to end check of the PUBLIC Cadre devnet (docs/DEVNET.md), run from the Mac against
// the public endpoints only: Solana devnet RPC, https://rpc.<box0> (node), https://storefront.<box0>
// (x402), https://relayer.<box0>/health, https://explorer.<box0>. Nothing is started here; the
// stack runs on the boxes as systemd services. Run through scripts/devnet-e2e.sh (it exports the
// devnet program ids before the relayer codec module loads).
//
// Flow (every step PASS/FAIL, supply invariant after each):
//   1. preflight: endpoints up, genesis params = SPEC defaults, light client following
//   2. fund throwaway e2e keys from the ops key (SOL only)
//   3. user buys CADRE on the pump.fun devnet curve; bridge in (deposit -> relayer unlock, K deep)
//      in parallel with an x402 storefront order (-> job on the chain)
//   4. bridge out: BridgeLock half the user's chain balance -> release_proof on Solana
//      in parallel with: job mined (job blocks) + delivered + completed -> finalize_order_proof
//   5. buyback: settle_share releases the order's share; crank_buyback into the mining reserve
//   6. refund: chain floor raised (AdminConfig), a new order is held back and refunded on Solana
//      after REFUND_GRACE_MS; floor restored
//   7. explorer shows the job block as paid by the order, and the buyback
// Results: .devnet-e2e/result-<ts>.json (timings, relay lag samples, SOL spent by ops).

import { blake3 } from "@noble/hashes/blake3";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  ComputeBudgetProgram, Connection, Keypair, LAMPORTS_PER_SOL, PublicKey, SystemProgram, Transaction, TransactionInstruction,
  TransactionMessage, VersionedTransaction,
} from "@solana/web3.js";
import {
  BRIDGE_PROGRAM_ID, ORDER_LAYOUT, ORDER_STATUS_FINALIZED, ORDER_STATUS_REFUNDED, STOREFRONT_PROGRAM_ID, ata, bridgeConfigPda, bridgeRefundPda,
  decodeBridgeConfig, decodeOrder, orderPda, sharePda, tokenAccountAmount, u16le, u32le, u64le,
} from "../../services/relayer/src/codec.ts";
import { KIND, signTx, transferBody } from "../../services/relayer/src/agent-tx.ts";
import { NodeRpc } from "../../services/relayer/src/node.ts";
import { RpcChain } from "../../services/relayer/src/chain.ts";
import { Relayer } from "../../services/relayer/src/relayer.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const OUT = resolve(ROOT, ".devnet-e2e");
mkdirSync(resolve(OUT, "keys"), { recursive: true });
const env = process.env;
const BOX0 = env.BOX0_IP!;
const DASH = BOX0.replaceAll(".", "-");
const SOL_RPC = env.SOLANA_RPC!;
const NODE_RPC = env.E2E_NODE_RPC ?? `https://rpc.${DASH}.sslip.io`;
const STOREFRONT = `https://storefront.${DASH}.sslip.io`;
const RELAYER_HEALTH = `https://relayer.${DASH}.sslip.io/health`;
const EXPLORER = `https://explorer.${DASH}.sslip.io`;
const RESERVE_ID = new PublicKey(env.RESERVE_PROGRAM!);
const LIGHTCLIENT_ID = new PublicKey(env.LIGHTCLIENT_PROGRAM!);
const RUN = env.E2E_RUN ?? new Date().toISOString().replace(/[:.]/g, "-");
const JOB = { m: 64, n: 64, k: 2048, rank: 128, maxBlocks: 2, deadlineBlocks: 400 };
const BUY_SOL = BigInt(env.E2E_BUY_LAMPORTS ?? 50_000_000); // the user's curve buy, 0.05 SOL
const MIN = 60_000;

const conn = new Connection(SOL_RPC, "confirmed");
const node = NodeRpc.http(NODE_RPC);
const loadKp = (f: string) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(resolve(ROOT, f), "utf8"))));
const ops = loadKp("keys/devnet/ops-keypair.json"); // Solana fee payer + admin of the programs and the chain (AdminConfig)
function e2eKey(name: string): { kp: Keypair; file: string } {
  const file = resolve(OUT, "keys", `${name}.json`);
  if (!existsSync(file)) writeFileSync(file, JSON.stringify(Array.from(Keypair.generate().secretKey)), { mode: 0o600 });
  return { kp: loadKp(file), file };
}
const user = e2eKey("user");
const buyer = e2eKey("buyer");
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const info = (msg: string, extra?: unknown) => console.log(`    [${new Date().toISOString().slice(11, 19)}] ${msg}${extra === undefined ? "" : " " + JSON.stringify(extra, big)}`);
function big(_k: string, v: unknown) { return typeof v === "bigint" ? v.toString() : v; }

function run(cmd: string, args: string[], opts: { cwd?: string; env?: Record<string, string> } = {}): string {
  try {
    return execFileSync(cmd, args, { cwd: opts.cwd ?? ROOT, env: { ...process.env, ...opts.env }, encoding: "utf8", timeout: 600_000,
      stdio: ["ignore", "pipe", "pipe"], maxBuffer: 64 << 20 });
  } catch (e) {
    const x = e as { stdout?: string; stderr?: string; message: string };
    throw new Error(`${cmd} ${args.join(" ")} failed: ${x.message}\n${x.stdout ?? ""}\n${x.stderr ?? ""}`.slice(0, 4000));
  }
}
const lastJson = (out: string): any => JSON.parse(out.trim().split("\n").reverse().find((l) => l.trim().startsWith("{"))!);
async function waitFor<T>(desc: string, f: () => Promise<T | null | undefined | false>, timeoutMs: number, everyMs = 10_000): Promise<T> {
  const until = Date.now() + timeoutMs;
  let lastErr = "";
  for (;;) {
    try {
      const v = await f();
      if (v) return v as T;
    } catch (e) {
      lastErr = String((e as Error).message ?? e).slice(0, 300);
    }
    if (Date.now() > until) throw new Error(`timed out waiting for ${desc}${lastErr ? ` (last error: ${lastErr})` : ""}`);
    await sleep(everyMs);
  }
}
// Reads retry a dropped connection (run 4, 2026-10-20: one "fetch failed" from the Mac failed the
// preflight while every endpoint answered); writes never retry (a lost reply may have landed).
async function httpJson(url: string, init?: RequestInit, retry = !init?.method || init.method === "GET"): Promise<{ status: number; body: any }> {
  let r: Response;
  for (let i = 0; ; i++) {
    try {
      r = await fetch(url, { ...init, signal: AbortSignal.timeout(60_000) });
      break;
    } catch (e) {
      if (!retry || i >= 4) throw e;
      console.log(`    [${new Date().toISOString().slice(11, 19)}] ${new URL(url).host}: ${String((e as Error).message ?? e).slice(0, 120)}; retry ${i + 1}/4`);
      await sleep(2000 * 2 ** i);
    }
  }
  const t = await r.text();
  try { return { status: r.status, body: JSON.parse(t) }; } catch { return { status: r.status, body: t }; }
}
async function nodeCall(method: string, params: Record<string, unknown> = {}): Promise<any> {
  const r = await httpJson(NODE_RPC, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) }, method.startsWith("get"));
  if (r.body?.error) throw new Error(`node ${method}: ${r.body.error.message}`);
  return r.body.result;
}
const crankEnv = () => ({ RPC_URL: SOL_RPC, KEYPAIR: resolve(ROOT, "keys/devnet/ops-keypair.json"),
  RESERVE_IDL: resolve(ROOT, "deploy/devnet-idl/reserve.json"), STOREFRONT_IDL: resolve(ROOT, "deploy/devnet-idl/storefront.json") });
const crank = (args: string[]) => run("node", ["src/main.ts", ...args], { cwd: resolve(ROOT, "services/crank"), env: crankEnv() });
// sf-settle (lists the unsettled shares afresh each time) and sf-status (read only) are safe to run
// again; run 7 (2026-10-20 15:14 UTC) failed step 5 when sf-settle hit Solana devnet's public RPC 429
// after web3.js's own 4 retries
function crankRetry(args: string[]): string {
  for (let i = 0; ; i++) {
    try {
      return crank(args);
    } catch (e) {
      if (i >= 4 || !/\b429\b/.test(String((e as Error).message ?? e))) throw e;
      console.log(`    [${new Date().toISOString().slice(11, 19)}] crank ${args[0]}: Solana RPC 429; retry ${i + 1}/4 in ${15 * (i + 1)} s`);
      execFileSync("sleep", [String(15 * (i + 1))]);
    }
  }
}
const trade = (args: string[]) => lastJson(run("node", ["src/trade.ts", ...args], { cwd: resolve(ROOT, "services/crank"), env: { RPC_URL: SOL_RPC } }));
const relayerCli = (args: string[]) => run("node", ["src/cli.ts", ...args], { cwd: resolve(ROOT, "services/relayer"),
  env: { TOKEN_RPC: SOL_RPC, NODE_RPC, TOKEN_PAYER: resolve(ROOT, "keys/devnet/ops-keypair.json"), AGENT_KEY: resolve(ROOT, "keys/devnet/ops-keypair.json"),
    STATE_FILE: resolve(OUT, "relayer-cli-state.json") } });
/** Light-client status from header-relay on box 0 (read only). */
function lcStatus(): any {
  const out = run("bash", ["scripts/devnet-deploy.sh", "ssh", "0",
    `/opt/lode/bin/header-relay --node http://127.0.0.1:10630 --solana ${SOL_RPC} --payer /etc/lode/keys/ops.json status`]);
  return JSON.parse(out.trim().split("\n").pop()!);
}

// ------------------------------------------------------------ Solana helpers

async function send(ixs: TransactionInstruction[], signers: Keypair[]): Promise<string> {
  const tx = new Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), ...ixs);
  const { blockhash, lastValidBlockHeight } = await conn.getLatestBlockhash();
  tx.feePayer = signers[0].publicKey;
  tx.recentBlockhash = blockhash;
  tx.sign(...signers);
  const sig = await conn.sendRawTransaction(tx.serialize());
  const r = await conn.confirmTransaction({ signature: sig, blockhash, lastValidBlockHeight }, "confirmed");
  if (r.value.err) throw new Error(`tx ${sig} failed: ${JSON.stringify(r.value.err)}`);
  return sig;
}
const disc = (n: string) => createHash("sha256").update(`global:${n}`).digest().subarray(0, 8);
let MINT: PublicKey;
async function splBalance(owner: PublicKey): Promise<bigint> {
  const a = await conn.getAccountInfo(ata(owner, MINT));
  return a ? tokenAccountAmount(a.data) : 0n;
}
async function bridgeNonce(): Promise<bigint> {
  return decodeBridgeConfig((await conn.getAccountInfo(bridgeConfigPda()))!.data).depositNonce;
}
async function lastIxName(addr: PublicKey, program: PublicKey, names: string[]): Promise<string | null> {
  const [sig] = await conn.getSignaturesForAddress(addr, { limit: 1 });
  if (!sig) return null;
  const tx = await conn.getTransaction(sig.signature, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
  if (!tx) return null;
  const keys = tx.transaction.message.staticAccountKeys;
  for (const ci of tx.transaction.message.compiledInstructions) {
    if (!keys[ci.programIdIndex].equals(program)) continue;
    const d = Buffer.from(ci.data).subarray(0, 8);
    for (const n of names) if (d.equals(disc(n))) return `${n} (${sig.signature.slice(0, 16)}...)`;
  }
  return null;
}

// ------------------------------------------------------------ chain helpers

async function chainBalance(k: PublicKey): Promise<bigint> { return (await node.getAccount(k.toBase58())).balance; }
async function sendChainTx(kp: Keypair, kind: number, body: Buffer, fee?: bigint): Promise<string> {
  const ci = await node.getChainInfo();
  const p = await nodeCall("getParams");
  const acct = await node.getAccount(kp.publicKey.toBase58());
  const tx = signTx(kp, { chainId: ci.chainId, from: kp.publicKey, nonce: acct.nonce, fee: fee ?? BigInt(p.min_fee), kind }, body);
  const h = await node.sendTx(tx);
  await waitFor(`chain tx ${h.slice(0, 12)} included`, async () => (await node.getTx(h))?.status === "included", 20 * MIN);
  return h;
}
async function depositFinal(nonce: bigint): Promise<any> {
  return waitFor(`deposit ${nonce} unlocked and final on the chain`, async () => {
    const d = await node.getDeposit(nonce);
    return d.processed && d.final ? d : null;
  }, 120 * MIN);
}
function orderTermsHash(poster: PublicKey, mm: number, n: number, k: number, maxBlocks: number, deadlineBlocks: bigint): Buffer {
  return Buffer.from(blake3(Buffer.concat([Buffer.from("pearl-sol:order-terms:v1"), poster.toBuffer(), u32le(mm), u32le(n), u32le(k),
    u32le(maxBlocks), u64le(deadlineBlocks)])));
}

// ------------------------------------------------------------ reporting

interface StepResult { step: string; ok: boolean; secs: number; detail: string; invariant: string }
const results: StepResult[] = [];
const t0 = Date.now();
const lagSamples: { t: string; node: number; lc_tip: number; lag: number }[] = [];
async function invariant(): Promise<string> {
  const r = new Relayer({ token: new RpcChain("token", SOL_RPC), node, signers: [], tokenPayer: ops, agentKey: ops,
    stateFile: resolve(OUT, "invariant-probe.json"), dryRun: true, log: () => {} });
  const t = await r.tick();
  const inv = t.invariant;
  if (!inv) return `n/a (${t.errors.join("; ")})`;
  return `${inv.ok ? "OK" : "BROKEN"} vault_spl=${inv.vaultSpl} 1e18-escrow=${inv.outsideEscrow} in_flight_deposits_spl=${inv.pendingDepositSpl} ` +
    `in_flight_withdrawals_spl=${inv.pendingWithdrawalSpl} delta=${inv.delta}`;
}
async function step(name: string, f: () => Promise<string>): Promise<void> {
  const s = Date.now();
  console.log(`\n== ${name}`);
  let ok = true, detail = "";
  try { detail = await f(); } catch (e) { ok = false; detail = String((e as Error).stack ?? e).slice(0, 3000); }
  let inv = "";
  try { inv = await invariant(); if (inv.startsWith("BROKEN")) ok = false; } catch (e) { inv = `n/a (${String((e as Error).message).slice(0, 200)})`; }
  const secs = (Date.now() - s) / 1000;
  results.push({ step: name, ok, secs, detail, invariant: inv });
  console.log(`   ${ok ? "PASS" : "FAIL"} ${name} (${(secs / 60).toFixed(1)} min): ${detail}`);
  console.log(`   invariant: ${inv}`);
  if (!ok) throw new Error(`step failed: ${name}`);
}
let lagTimer: NodeJS.Timeout | null = null;
function sampleLag() {
  lagTimer = setInterval(() => {
    try {
      const s = lcStatus();
      lagSamples.push({ t: new Date().toISOString(), node: s.node_height ?? -1, lc_tip: s.tip_height, lag: s.lag_blocks ?? -1 });
    } catch { /* box unreachable for a moment */ }
  }, 5 * MIN);
}

// ------------------------------------------------------------ steps

let params0: any;
async function preflight(): Promise<string> {
  const ci = await nodeCall("getChainInfo");
  params0 = await nodeCall("getParams");
  for (const [k, v] of Object.entries({ finality_k: 30, target_block_time_ms: 60000, emission_ppm: 7, alpha_ppm: 800000 })) {
    if (Number(params0[k]) !== v) throw new Error(`genesis param ${k} = ${params0[k]}, SPEC default ${v}`);
  }
  const h = await httpJson(RELAYER_HEALTH);
  if (h.body?.ok !== true) throw new Error(`relayer health ${JSON.stringify(h.body).slice(0, 300)}`);
  const sf = await httpJson(`${STOREFRONT}/health`);
  if (sf.body?.ok !== true) throw new Error(`storefront health ${JSON.stringify(sf.body).slice(0, 300)}`);
  const ex = await httpJson(`${EXPLORER}/api/summary`);
  const bc = decodeBridgeConfig((await conn.getAccountInfo(bridgeConfigPda()))!.data);
  MINT = bc.mint;
  const lc = lcStatus();
  if (!lc.initialized) throw new Error("light client not initialized");
  return `node height ${ci.height} final ${ci.finalized_height} peers ${ci.peers} chain_id ${ci.chain_id ?? ci.chainId}; params K ${params0.finality_k}, ` +
    `${params0.target_block_time_ms} ms, emission ${params0.emission_ppm} ppm; relayer ok; storefront ok; explorer ${ex.status}; mint ${MINT.toBase58()}; ` +
    `light client tip ${lc.tip_height} final ${lc.final_height} lag ${lc.lag_blocks}`;
}

let opsSol0 = 0;
async function fund(): Promise<string> {
  opsSol0 = await conn.getBalance(ops.publicKey);
  const want = [[user.kp.publicKey, BUY_SOL + 30_000_000n], [buyer.kp.publicKey, 500_000_000n]] as const;
  const ixs: TransactionInstruction[] = [];
  for (const [k, lamports] of want) {
    const have = BigInt(await conn.getBalance(k));
    if (have < lamports) ixs.push(SystemProgram.transfer({ fromPubkey: ops.publicKey, toPubkey: k, lamports: lamports - have }));
  }
  if (ixs.length) await send(ixs, [ops]);
  return `ops ${ops.publicKey.toBase58()} ${(opsSol0 / LAMPORTS_PER_SOL).toFixed(4)} SOL; user ${user.kp.publicKey.toBase58()}, buyer ${buyer.kp.publicKey.toBase58()} funded`;
}

let order: { nonce: bigint; jobId: string; postedHeight: number } | null = null;
let orderBody: unknown = null;
let bridgedIn = 0n;

async function payOrder(body: unknown): Promise<any> {
  // without a payment the storefront only quotes (402, no state), so this POST may retry: run 5
  // (2026-10-20 11:06 UTC) timed out here while Solana devnet's public RPC answered 429 to box 0 and the Mac
  const r402 = await httpJson(`${STOREFRONT}/jobs`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }, true);
  if (r402.status !== 402) throw new Error(`expected 402, got ${r402.status} ${JSON.stringify(r402.body)}`);
  const req = r402.body.accepts[0];
  const { blockhash } = await conn.getLatestBlockhash();
  const msg = new TransactionMessage({ payerKey: new PublicKey(req.extra.feePayer), recentBlockhash: blockhash, instructions: [
    ComputeBudgetProgram.setComputeUnitLimit({ units: 10_000 }),
    SystemProgram.transfer({ fromPubkey: buyer.kp.publicKey, toPubkey: new PublicKey(req.payTo), lamports: BigInt(req.maxAmountRequired) }),
  ] }).compileToV0Message();
  const vtx = new VersionedTransaction(msg);
  vtx.sign([buyer.kp]);
  const xPayment = Buffer.from(JSON.stringify({ x402Version: 1, scheme: "exact", network: req.network,
    payload: { transaction: Buffer.from(vtx.serialize()).toString("base64") } })).toString("base64");
  const r = await httpJson(`${STOREFRONT}/jobs`, { method: "POST", headers: { "content-type": "application/json", "x-payment": xPayment }, body: JSON.stringify(body) });
  if (r.status !== 201) throw new Error(`POST /jobs with payment: ${r.status} ${JSON.stringify(r.body)}`);
  info("order placed", { nonce: r.body.id, job_id: r.body.job_id, venue: r.body.venue, paid_lamports: req.maxAmountRequired });
  return r;
}

async function bridgeInAndOrder(): Promise<string> {
  // bridge in: the user buys on the pump.fun devnet curve and deposits half
  const b = trade(["buy", "--keypair", user.file, "--mint", MINT.toBase58(), "--sol", BUY_SOL.toString()]);
  const amount = BigInt(b.tokens) / 2n;
  const depNonce = await bridgeNonce();
  const tDep = Date.now();
  console.log(relayerCli(["deposit", amount.toString(), user.kp.publicKey.toBase58(), user.file]).trim());
  // the order: job data blob on the node, x402 purchase
  const jobDir = resolve(OUT, `job-${RUN}`);
  const g = lastJson(run(resolve(ROOT, "crates/target/release/pearl-job"), ["gen", "--m", String(JOB.m), "--n", String(JOB.n), "--k", String(JOB.k), "--out-dir", jobDir, "--seed", String(Date.now() % 100000)]));
  const put = await nodeCall("putBlob", { data: readFileSync(g.blob_file).toString("base64") });
  if (put.uri !== g.blob_uri) throw new Error(`putBlob uri ${put.uri} != ${g.blob_uri}`);
  const cfg = lastJson(run(resolve(ROOT, "crates/target/release/pearl-job"), ["config", "--k", String(JOB.k), "--rank", String(JOB.rank)])).config as string;
  const recipient = buyer.kp.publicKey;
  const terms = orderTermsHash(recipient, JOB.m, JOB.n, JOB.k, JOB.maxBlocks, BigInt(JOB.deadlineBlocks));
  const c = lastJson(run(resolve(ROOT, "crates/target/release/pearl-job"), ["commit", "--job", recipient.toBase58(), "--terms-hash", terms.toString("hex"), "--config", cfg, "--data", g.data_uri]));
  orderBody = { spec: { m: JOB.m, n: JOB.n, k: JOB.k, config: cfg, hash_a: c.hash_a, hash_b: c.hash_b, data_uri: put.uri, deadline_blocks: JOB.deadlineBlocks, max_blocks: JOB.maxBlocks } };
  const quote = await httpJson(`${STOREFRONT}/quote?m=${JOB.m}&n=${JOB.n}&k=${JOB.k}&max_blocks=${JOB.maxBlocks}`);
  info("quote", { price_lamports: quote.body.price_lamports, venue: quote.body.venue });
  const tOrd = Date.now();
  const r = await payOrder(orderBody);
  const nonce = BigInt(r.body.id);
  const [d, od] = await Promise.all([depositFinal(depNonce), depositFinal(nonce)]);
  const depMin = (Date.now() - tDep) / MIN;
  const job = await nodeCall("getJob", { job_id: r.body.job_id });
  if (job.poster !== recipient.toBase58() || job.hash_a !== c.hash_a || job.job_key !== c.job_key) throw new Error(`job mismatch ${JSON.stringify(job)}`);
  order = { nonce, jobId: job.job_id, postedHeight: job.posted_height };
  bridgedIn = await chainBalance(user.kp.publicKey);
  if (bridgedIn < amount * 1000n) throw new Error(`user chain balance ${bridgedIn} < ${amount * 1000n}`);
  return `user bought ${b.tokens} on ${b.venue === "pumpswap" ? "PumpSwap" : "the curve"}, deposited ${amount} SPL (nonce ${depNonce}) -> unlocked at ${d.height}, final after ${depMin.toFixed(1)} min; ` +
    `x402 order ${nonce} unlocked at ${od.height} (${((Date.now() - tOrd) / MIN).toFixed(1)} min): job ${job.job_id.slice(0, 16)} W=${job.worker_pay} P=${job.protocol_fee}`;
}

async function bridgeOutAndJob(): Promise<string> {
  if (!order) throw new Error("no order");
  const lockAmount = ((bridgedIn / 2n) / 1000n) * 1000n;
  const to = user.kp.publicKey;
  const splBefore = await splBalance(to);
  const tLock = Date.now();
  const out = relayerCli(["lock", lockAmount.toString(), to.toBase58(), user.file]).trim();
  console.log(out);
  const outP = (async () => {
    const got = await waitFor("SPL released on Solana", async () => { const b = await splBalance(to); return b > splBefore ? b : null; }, 150 * MIN);
    const ev = await nodeCall("getBridgeEvents", { from_nonce: 0, limit: 1000 });
    const wd = (ev.withdrawals as any[]).reverse().find((x) => x.solana_recipient === to.toBase58() && BigInt(x.amount_spl) === lockAmount / 1000n);
    if (!wd) throw new Error("withdrawal not in getBridgeEvents");
    const processed = PublicKey.findProgramAddressSync([Buffer.from("processed"), Buffer.from([1]), u64le(BigInt(wd.nonce))], BRIDGE_PROGRAM_ID)[0];
    const how = await lastIxName(processed, BRIDGE_PROGRAM_ID, ["release_proof", "release"]);
    if (got - splBefore !== lockAmount / 1000n) throw new Error(`released ${got - splBefore}`);
    return `bridge out ${lockAmount} units -> ${lockAmount / 1000n} SPL released by ${how} after ${((Date.now() - tLock) / MIN).toFixed(1)} min`;
  })();
  const jobP = (async () => {
    const tJob = Date.now();
    const job = await waitFor("job mined, delivered and completed", async () => {
      const j = await nodeCall("getJob", { job_id: order!.jobId });
      return j.status === "completed" ? j : null;
    }, 150 * MIN, 20_000);
    const mined = (Date.now() - tJob) / MIN;
    const pda_ = orderPda(order!.nonce);
    const acc = await waitFor("finalize_order on Solana", async () => {
      const a = await conn.getAccountInfo(pda_);
      return a && a.data[ORDER_LAYOUT.STATUS] === ORDER_STATUS_FINALIZED ? a : null;
    }, 150 * MIN);
    const how = await lastIxName(pda_, STOREFRONT_PROGRAM_ID, ["finalize_order_proof", "finalize_order"]);
    const rh = Buffer.from(acc.data.subarray(ORDER_LAYOUT.RESULT_HASH, ORDER_LAYOUT.RESULT_HASH + 32)).toString("hex");
    if (rh !== job.result_hash) throw new Error(`Order result_hash ${rh} != ${job.result_hash}`);
    return `job ${job.blocks_mined} block(s), subsidy ${job.subsidy_paid}, completed at ${job.closed_height} by ${job.deliverer} (${mined.toFixed(1)} min); ` +
      `Order finalized by ${how} after ${((Date.now() - tJob) / MIN).toFixed(1)} min, result_hash ${rh.slice(0, 16)}`;
  })();
  const [a, b] = await Promise.all([outP, jobP]);
  return `${a}; ${b}`;
}

async function buyback(): Promise<string> {
  if (!order) throw new Error("no order");
  const settled = JSON.parse(crankRetry(["sf-settle"]).trim().split("\n").pop()!);
  const s0 = JSON.parse(crankRetry(["sf-status"]));
  const reserveCreator = PublicKey.findProgramAddressSync([Buffer.from("creator")], RESERVE_ID)[0];
  const r0 = await splBalance(reserveCreator);
  // the box's crank service may already have bought (it runs every 600 s); then check its event
  let b: any = null;
  const avail = BigInt(s0.availableLamports);
  if (avail >= BigInt(s0.minBuy ?? 0) && avail > 0n) {
    const out = crank(["sf-buyback"]).trim().split("\n").pop()!;
    b = JSON.parse(out);
  }
  const s1 = JSON.parse(crankRetry(["sf-status"]));
  const r1 = await splBalance(reserveCreator);
  if (!(BigInt(s1.buys) > 0n)) throw new Error(`no buyback yet: ${JSON.stringify(s1)}`);
  return `settle ${JSON.stringify(settled.released ?? settled)}; vault available ${s0.availableLamports} (bps ${s0.buybackBps}, dest ${s0.destination}); ` +
    `${b ? `crank_buyback ${b.ok ? "ok" : "refused"} sol_in ${b.solIn} -> ${b.tokensOut} SPL (${b.sig?.slice(0, 16)}...)` : "buy left to the box's crank (interval)"}; ` +
    `totals: ${s1.buys} buys, to reserve ${s1.toReserve}; reserve ${r0} -> ${r1}`;
}

async function refund(): Promise<string> {
  const key = Number(params0.keys.min_protocol_fee);
  const adminConfig = (v: bigint) => Buffer.concat([Buffer.from([key]), u16le(8), u64le(v)]);
  const HIGH = 10n ** 16n;
  if ((await chainBalance(ops.publicKey)) < 1_000_000n) throw new Error("ops (chain admin) has no chain balance for the AdminConfig fee");
  const t = Date.now();
  await sendChainTx(ops, KIND.AdminConfig, adminConfig(HIGH));
  try {
    await waitFor("min_protocol_fee_effective raised", async () => (BigInt((await nodeCall("getParams")).min_protocol_fee_effective) === HIGH ? true : null), 60 * MIN);
    const r = await payOrder(orderBody);
    const nonce = BigInt(r.body.id);
    const o = decodeOrder((await conn.getAccountInfo(orderPda(nonce)))!.data);
    const buyerAta = ata(o.buyer, MINT);
    const before = (await conn.getAccountInfo(buyerAta)) ? tokenAccountAmount((await conn.getAccountInfo(buyerAta))!.data) : 0n;
    const held = await waitFor("relayer holds the order back", async () => {
      const h = (await httpJson(RELAYER_HEALTH)).body;
      return h?.needsRefund?.[nonce.toString()] ?? null;
    }, 60 * MIN);
    await waitFor("Order Refunded on Solana", async () => {
      const a = await conn.getAccountInfo(orderPda(nonce));
      return a && decodeOrder(a.data).status === ORDER_STATUS_REFUNDED ? true : null;
    }, 180 * MIN, 30_000);
    const how = await lastIxName(bridgeRefundPda(nonce), BRIDGE_PROGRAM_ID, ["refund_deposit"]);
    const after = tokenAccountAmount((await conn.getAccountInfo(buyerAta))!.data);
    if (after - before !== o.tokens) throw new Error(`buyer got ${after - before} back, want ${o.tokens}`);
    if (await conn.getAccountInfo(sharePda(nonce))) throw new Error("share record still open after the refund");
    return `floor raised to ${HIGH}; order ${nonce} (P ${o.body.protocolFee}) held back (${held.reason}), refunded by ${how}: ${o.tokens} SPL to ${o.buyer.toBase58()} ` +
      `+ share back, ${((Date.now() - t) / MIN).toFixed(1)} min`;
  } finally {
    await sendChainTx(ops, KIND.AdminConfig, adminConfig(BigInt(params0.min_protocol_fee)));
    info("floor restored", { min_protocol_fee: params0.min_protocol_fee });
  }
}

async function explorer(): Promise<string> {
  if (!order) throw new Error("no order");
  const j = await waitFor("explorer shows the job", async () => {
    const r = await httpJson(`${EXPLORER}/api/job/${order!.jobId}`);
    return r.status === 200 ? r.body : null;
  }, 20 * MIN);
  const s = await httpJson(`${EXPLORER}/api/summary`);
  const page = await fetch(`${EXPLORER}/`).then((r) => r.text());
  return `explorer job ${JSON.stringify(j).slice(0, 400)}; summary ${JSON.stringify(s.body).slice(0, 400)}; home page ${page.length} bytes`;
}

// ------------------------------------------------------------ main

let failed = false;
sampleLag();
try {
  await step("1. preflight: public endpoints, SPEC genesis params, light client", preflight);
  await step("2. fund throwaway e2e keys from ops", fund);
  await step("3. bridge in (pump.fun buy + deposit) and x402 storefront order, both unlocked K deep", bridgeInAndOrder);
  await step("4. bridge out (BridgeLock -> release_proof) and the paid job (mined, delivered, completed -> finalize_order_proof)", bridgeOutAndJob);
  await step("5. storefront buyback into the mining reserve", buyback);
  await step("6. refund: underpaid order held back and refunded on Solana", refund);
  await step("7. explorer shows the paid job", explorer);
} catch (e) {
  failed = true;
  console.log(`\n${String((e as Error).message)}`);
}
if (lagTimer) clearInterval(lagTimer);
const opsSol1 = await conn.getBalance(ops.publicKey);
const hours = (Date.now() - t0) / 3_600_000;
console.log("\n== summary");
for (const r of results) console.log(`  ${r.ok ? "PASS" : "FAIL"}  ${r.step}  (${(r.secs / 60).toFixed(1)} min)`);
console.log(`  total ${((Date.now() - t0) / MIN).toFixed(1)} min; ops SOL ${(opsSol0 / 1e9).toFixed(4)} -> ${(opsSol1 / 1e9).toFixed(4)}`);
console.log(`  relay lag samples: ${JSON.stringify(lagSamples.map((s) => s.lag))}`);
writeFileSync(resolve(OUT, `result-${RUN}.json`), JSON.stringify({ results, lagSamples, opsSol0, opsSol1, hours }, big, 2));
process.exit(failed || results.some((r) => !r.ok) ? 1 : 0);
