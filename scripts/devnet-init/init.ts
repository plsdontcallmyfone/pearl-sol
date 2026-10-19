// One-time Solana devnet setup for the public Cadre devnet (docs/DEVNET.md), run from the Mac
// through scripts/devnet-solana.sh init (it exports the devnet program ids first). Idempotent:
// every step checks the on-chain state and skips what is already there.
//   1. reserve initialize (bridge program, the chain's mining pool), pump.fun launch of CADRE
//      (create_v2 on the real pump.fun devnet program, 1 SOL dev buy into the reserve)
//   2. bridge initialize (mint, attester 1 of 1)
//   3. storefront initialize (mint, bridge, attester 1 of 1) + the trader PDA's pump / PumpSwap
//      volume accumulators, buyback vault setup, buyback config = the owner defaults
//      (1000 bps to the mining reserve, cap 1 SOL per call, min buy 0.001 SOL, 600 s)
//   4. chain gas: ops buys a little CADRE and bridges it to the ops (chain admin) and miner0
//      (job deliverer) accounts
//   5. fund_mining: reserve tokens -> bridge -> the chain's mining pool
// The light client (header-relay init on box 0) and proof modes are set by devnet-solana.sh.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { ComputeBudgetProgram, Connection, Keypair, PublicKey, SystemProgram, Transaction, TransactionInstruction } from "@solana/web3.js";
import { BRIDGE_PROGRAM_ID, STOREFRONT_PROGRAM_ID, bridgeConfigPda, decodeBridgeConfig, storefrontConfigPda, traderPda, u32le } from "../../services/relayer/src/codec.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const env = process.env;
const SOL_RPC = env.SOLANA_RPC!;
const DASH = env.BOX0_IP!.replaceAll(".", "-");
const NODE_RPC = `https://rpc.${DASH}.sslip.io`;
const conn = new Connection(SOL_RPC, "confirmed");
const loadKp = (f: string) => Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(resolve(ROOT, f), "utf8"))));
const ops = loadKp("keys/devnet/ops-keypair.json");
const attester = loadKp("keys/devnet/attester-keypair.json");
const miner0 = loadKp("keys/devnet/miner0-keypair.json");
const PUMP = new PublicKey("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
const AMM = new PublicKey("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
const DEV_BUY = env.DEV_BUY_LAMPORTS ?? "1000000000";
const FUND_SPL = env.FUND_SPL ?? "10000000000000"; // 10M CADRE (6 dp) from the reserve to the mining pool
const GAS_BUY = env.GAS_BUY_LAMPORTS ?? "20000000"; // 0.02 SOL of CADRE for chain gas
const log = (m: string) => console.log(`[${new Date().toISOString().slice(11, 19)}] ${m}`);

function run(cmd: string, args: string[], cwd: string, e: Record<string, string> = {}): string {
  try {
    return execFileSync(cmd, args, { cwd, env: { ...process.env, ...e }, encoding: "utf8", timeout: 600_000, stdio: ["ignore", "pipe", "pipe"] });
  } catch (x) {
    const y = x as { stdout?: string; stderr?: string; message: string };
    throw new Error(`${cmd} ${args.join(" ")}: ${y.message}\n${y.stdout ?? ""}\n${y.stderr ?? ""}`.slice(0, 4000));
  }
}
const crankEnv = { RPC_URL: SOL_RPC, KEYPAIR: resolve(ROOT, "keys/devnet/ops-keypair.json"),
  RESERVE_IDL: resolve(ROOT, "deploy/devnet-idl/reserve.json"), STOREFRONT_IDL: resolve(ROOT, "deploy/devnet-idl/storefront.json") };
const crank = (args: string[]) => run("node", ["src/main.ts", ...args], resolve(ROOT, "services/crank"), crankEnv);
const trade = (args: string[]) => run("node", ["src/trade.ts", ...args], resolve(ROOT, "services/crank"), { RPC_URL: SOL_RPC });
const relayerCli = (args: string[]) => run("node", ["src/cli.ts", ...args], resolve(ROOT, "services/relayer"),
  { TOKEN_RPC: SOL_RPC, NODE_RPC, TOKEN_PAYER: crankEnv.KEYPAIR, AGENT_KEY: crankEnv.KEYPAIR, STATE_FILE: resolve(ROOT, ".devnet-e2e/init-relayer-cli.json") });
async function nodeCall(method: string, params: Record<string, unknown> = {}): Promise<any> {
  const r = await fetch(NODE_RPC, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
  const j = (await r.json()) as any;
  if (j.error) throw new Error(`${method}: ${j.error.message}`);
  return j.result;
}
async function send(ixs: TransactionInstruction[]): Promise<string> {
  const tx = new Transaction().add(ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), ...ixs);
  const { blockhash, lastValidBlockHeight } = await conn.getLatestBlockhash();
  tx.feePayer = ops.publicKey;
  tx.recentBlockhash = blockhash;
  tx.sign(ops);
  const sig = await conn.sendRawTransaction(tx.serialize());
  const r = await conn.confirmTransaction({ signature: sig, blockhash, lastValidBlockHeight }, "confirmed");
  if (r.value.err) throw new Error(`tx ${sig} failed: ${JSON.stringify(r.value.err)}`);
  return sig;
}
const disc = (n: string) => createHash("sha256").update(`global:${n}`).digest().subarray(0, 8);
const m = (pubkey: PublicKey, isWritable = false, isSigner = false) => ({ pubkey, isWritable, isSigner });
const pda = (seeds: Buffer[], prog: PublicKey) => PublicKey.findProgramAddressSync(seeds, prog)[0];
const some = (b: Buffer) => Buffer.concat([Buffer.from([1]), b]);
const NONE = Buffer.from([0]);

// 1. reserve + launch
const esc = await nodeCall("getEscrow");
let status = JSON.parse(crank(["status"]));
if (!status.initialized && !status.admin) {
  log(crank(["init", "--bridgeProgram", BRIDGE_PROGRAM_ID.toBase58(), "--miningPool", esc.pool]).trim());
  status = JSON.parse(crank(["status"]));
}
if (!status.mint || status.mint === PublicKey.default.toBase58()) {
  const mintFile = resolve(ROOT, "keys/devnet/mint-keypair.json");
  try { readFileSync(mintFile); } catch { execFileSync("solana-keygen", ["new", "--no-bip39-passphrase", "-s", "-o", mintFile]); }
  log(crank(["launch", "--name", "Cadre", "--symbol", "CADRE", "--uri", `https://explorer.${DASH}.sslip.io/cadre-token.json`,
    "--devBuy", DEV_BUY, "--mintKeypair", mintFile]).trim());
  status = JSON.parse(crank(["status"]));
}
const MINT = new PublicKey(status.mint);
log(`reserve: mint ${status.mint}, reserve tokens ${status.reserveTokens}, mining pool ${status.miningPool}`);

// 2. bridge
if (!(await conn.getAccountInfo(bridgeConfigPda()))) log(relayerCli(["init-bridge", MINT.toBase58(), "1", attester.publicKey.toBase58()]).trim());

// 3. storefront
const sfCfg = storefrontConfigPda();
if (!(await conn.getAccountInfo(sfCfg))) {
  const params = Buffer.concat([some(MINT.toBuffer()), some(BRIDGE_PROGRAM_ID.toBuffer()), NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE]);
  log(`storefront initialize ${await send([new TransactionInstruction({ programId: STOREFRONT_PROGRAM_ID,
    keys: [m(ops.publicKey, true, true), m(sfCfg, true), m(traderPda(), true), m(SystemProgram.programId)],
    data: Buffer.concat([disc("initialize"), params, u32le(1), attester.publicKey.toBuffer(), Buffer.from([1])]) })])}`);
}
for (const prog of [PUMP, AMM]) {
  const acc = pda([Buffer.from("user_volume_accumulator"), traderPda().toBuffer()], prog);
  if (await conn.getAccountInfo(acc)) continue;
  log(`trader volume accumulator ${prog.toBase58().slice(0, 4)} ${await send([new TransactionInstruction({ programId: prog,
    keys: [m(ops.publicKey, true, true), m(traderPda()), m(acc, true), m(SystemProgram.programId), m(pda([Buffer.from("__event_authority")], prog)), m(prog)],
    data: Buffer.from([94, 6, 202, 115, 255, 96, 232, 183]) })])}`);
}
log(crank(["sf-setup"]).trim());
log(crank(["sf-set", "--bps", "1000", "--dest", "reserve", "--maxPerCall", "1000000000", "--minBuy", "1000000", "--minIntervalSecs", "600"]).trim());

// 4. chain gas for ops (AdminConfig fees) and miner0 (SubmitResult / CompleteJob fees)
const bal = async (k: PublicKey) => BigInt((await nodeCall("getAccount", { account: k.toBase58() })).balance);
if ((await bal(ops.publicKey)) === 0n || (await bal(miner0.publicKey)) === 0n) {
  const b = JSON.parse(trade(["buy", "--keypair", crankEnv.KEYPAIR, "--mint", MINT.toBase58(), "--sol", GAS_BUY]).trim().split("\n").pop()!);
  const half = BigInt(b.tokens) / 2n;
  for (const k of [ops.publicKey, miner0.publicKey]) log(relayerCli(["deposit", half.toString(), k.toBase58(), crankEnv.KEYPAIR]).trim());
}

// 5. fund the chain's mining pool from the reserve (once)
if (BigInt(esc.pool_balance) === 0n) log(crank(["fund", "--amount", FUND_SPL]).trim());
log(`done: bridge ${decodeBridgeConfig((await conn.getAccountInfo(bridgeConfigPda()))!.data).depositNonce} deposits so far; ops SOL ${(await conn.getBalance(ops.publicKey)) / 1e9}`);

// 6. proof modes (once the light client is initialized by header-relay init on box 0): bridge
// releases and storefront finalize/expire only by light-client proof (MODE_PROOF = 2)
const LC = new PublicKey(env.LIGHTCLIENT_PROGRAM!);
if (await conn.getAccountInfo(pda([Buffer.from("config")], LC))) {
  for (const [prog, ix, cfg] of [[BRIDGE_PROGRAM_ID, "set_release_modes", bridgeConfigPda()], [STOREFRONT_PROGRAM_ID, "set_finalize_modes", storefrontConfigPda()]] as const) {
    log(`${ix} proof only: ${await send([new TransactionInstruction({ programId: prog, keys: [m(ops.publicKey, true, true), m(cfg, true)],
      data: Buffer.concat([disc(ix), Buffer.from([2]), LC.toBuffer()]) })])}`);
  }
} else log("light client not initialized yet: run header-relay init on box 0, then this again for the proof modes");
