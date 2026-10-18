// Buys N jobs through storefront-x402 from N fresh payer keys, so the explorer has paid
// work from distinct payers to show. LOCAL STACKS ONLY (the demo stack or the test VPS):
// payers are funded by the local test validator's airdrop; nothing touches a public chain.
// Run from the repo root on the machine that runs the stack:
//   node apps/explorer/tools/buy-jobs.mjs [count=3]
// Uses scripts/demo/node_modules (@solana/web3.js, @noble/hashes) and crates/target/release/pearl-job.
import { createRequire } from "node:module";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const ROOT = resolve(process.cwd());
const require = createRequire(join(ROOT, "scripts/demo/package.json"));
const { Connection, Keypair, PublicKey, SystemProgram, ComputeBudgetProgram, TransactionMessage, VersionedTransaction, LAMPORTS_PER_SOL } = require("@solana/web3.js");
const { blake3 } = require("@noble/hashes/blake3");

const TOKEN_RPC = "http://127.0.0.1:10600";
const DA_RPC = "http://127.0.0.1:10632";
const STOREFRONT = "http://127.0.0.1:10662";
const JOB_BIN = join(ROOT, "crates/target/release/pearl-job");
const JOB = { m: 64, n: 64, k: 2048, rank: 128, maxBlocks: 5, deadlineBlocks: 400 };
const count = Number(process.argv[2] ?? 3);
const conn = new Connection(TOKEN_RPC, "confirmed");

const u32 = (n) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const u64 = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const lastJson = (out) => JSON.parse(out.trim().split("\n").filter((l) => l.startsWith("{")).pop());
const job = (args) => lastJson(execFileSync(JOB_BIN, args, { encoding: "utf8" }));
const http = async (url, init) => { const r = await fetch(url, init); const t = await r.text(); let body; try { body = JSON.parse(t); } catch { body = t; } return { status: r.status, body }; };
const node = async (method, params, url = DA_RPC) => { const r = await http(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) }); if (r.body.error) throw new Error(`${method}: ${r.body.error.message}`); return r.body.result; };
const orderTerms = (poster) => Buffer.from(blake3(Buffer.concat([Buffer.from("pearl-sol:order-terms:v1"), poster.toBuffer(), u32(JOB.m), u32(JOB.n), u32(JOB.k), u32(JOB.maxBlocks), u64(JOB.deadlineBlocks)])));

const cfg = job(["config", "--k", String(JOB.k), "--rank", String(JOB.rank)]).config;
for (let i = 0; i < count; i++) {
  const payer = Keypair.generate();
  const sig = await conn.requestAirdrop(payer.publicKey, 2 * LAMPORTS_PER_SOL);
  await conn.confirmTransaction(sig, "confirmed");
  const dir = mkdtempSync(join(tmpdir(), "buy-jobs-"));
  const g = job(["gen", "--m", String(JOB.m), "--n", String(JOB.n), "--k", String(JOB.k), "--out-dir", dir, "--seed", String(1000 + Math.floor(Math.random() * 1e6))]);
  const put = await node("putBlob", { data: readFileSync(g.blob_file).toString("base64") });
  const c = job(["commit", "--job", payer.publicKey.toBase58(), "--terms-hash", orderTerms(payer.publicKey).toString("hex"), "--config", cfg, "--data", g.data_uri]);
  const body = { spec: { m: JOB.m, n: JOB.n, k: JOB.k, config: cfg, hash_a: c.hash_a, hash_b: c.hash_b, data_uri: put.uri, deadline_blocks: JOB.deadlineBlocks, max_blocks: JOB.maxBlocks } };
  const r402 = await http(`${STOREFRONT}/jobs`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  if (r402.status !== 402) throw new Error(`expected 402, got ${r402.status} ${JSON.stringify(r402.body)}`);
  const req = r402.body.accepts[0];
  const { blockhash } = await conn.getLatestBlockhash();
  const msg = new TransactionMessage({ payerKey: new PublicKey(req.extra.feePayer), recentBlockhash: blockhash, instructions: [
    ComputeBudgetProgram.setComputeUnitLimit({ units: 10_000 }),
    SystemProgram.transfer({ fromPubkey: payer.publicKey, toPubkey: new PublicKey(req.payTo), lamports: BigInt(req.maxAmountRequired) }),
  ] }).compileToV0Message();
  const vtx = new VersionedTransaction(msg);
  vtx.sign([payer]);
  const xPayment = Buffer.from(JSON.stringify({ x402Version: 1, scheme: "exact", network: req.network, payload: { transaction: Buffer.from(vtx.serialize()).toString("base64") } })).toString("base64");
  const r = await http(`${STOREFRONT}/jobs`, { method: "POST", headers: { "content-type": "application/json", "x-payment": xPayment }, body: JSON.stringify(body) });
  if (r.status !== 201) throw new Error(`POST /jobs with payment: ${r.status} ${JSON.stringify(r.body)}`);
  console.log(JSON.stringify({ payer: payer.publicKey.toBase58(), order: r.body.id, job_id: r.body.job_id, paid_lamports: req.maxAmountRequired, payment_tx: r.body.payment?.signature }));
}
