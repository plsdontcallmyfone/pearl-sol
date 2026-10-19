// M1 end to end demo driver (SPEC 7 M1 item 6). Run through scripts/demo.sh, which
// builds the binaries, checks ports and links node_modules. Every step prints
// PASS/FAIL and the supply invariant; the first failure stops the run (exit 1).
// Every process this script starts is stopped by its PID at the end.

import { blake3 } from "@noble/hashes/blake3";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { resolve } from "node:path";
import {
  ComputeBudgetProgram, Keypair, LAMPORTS_PER_SOL, PublicKey, SystemProgram, Transaction, TransactionInstruction,
  TransactionMessage, VersionedTransaction,
} from "@solana/web3.js";
import {
  ATTESTER_URL, DEMO, KEYS, NODE_RPC, RELAYER_HEALTH, ROOT, STOREFRONT_URL, TOKEN_RPC, conn, demoKey, httpJson, lastJson,
  loadKp, nodeCall, portBusy, procs, run, sleep, start, waitFor,
} from "./lib.ts";
import {
  BRIDGE_PROGRAM_ID, ORDER_LAYOUT, ORDER_STATUS_FINALIZED, ORDER_STATUS_REFUNDED, STOREFRONT_PROGRAM_ID, ata, bridgeRefundPda, bridgeConfigPda, decodeBridgeConfig,
  decodeOrder, orderPda, sharePda, storefrontConfigPda, tokenAccountAmount, traderPda, u16le, u32le, u64le,
} from "../../services/relayer/src/codec.ts";
import { KIND, signTx, transferBody } from "../../services/relayer/src/agent-tx.ts";
import { NodeRpc } from "../../services/relayer/src/node.ts";
import { RpcChain } from "../../services/relayer/src/chain.ts";
import { Relayer } from "../../services/relayer/src/relayer.ts";

// ------------------------------------------------------------ demo parameters

// Agent-chain genesis overrides (docs/HANDOFF.md "Demo"): short K and fast blocks so the
// whole flow fits in minutes; emission high enough that one job block hits alpha x P.
const FINALITY_K = Number(process.env.DEMO_K ?? 6);
const GENESIS_PARAMS = {
  finality_k: FINALITY_K,
  target_block_time_ms: Number(process.env.DEMO_BLOCK_MS ?? 1500),
  wtema_half_life_ms: 15_000,
  emission_ppm: 10_000,
  min_fee: 1000,
};
const MINER_THREADS = process.env.DEMO_MINER_THREADS ?? "4";
// M2 light client: the token validator runs 8 ticks per slot (50 ms slots) so the
// header relay (about 37 Solana txs per agent-chain header) keeps pace with 1.5 s blocks.
const TICKS_PER_SLOT = process.env.DEMO_TICKS_PER_SLOT ?? "8";
const LIGHTCLIENT_ID = new PublicKey("BUDUW2bGexRGRHhEkBUj8dAvffRuXQQacredxKtRMHK5");
const MODE_PROOF = 2;
const DEV_BUY = 1_000_000_000n; // 1 SOL dev buy into the reserve
const FUND_SPL = 10_000_000_000_000n; // 10M tokens (6 dp) from the reserve to the mining pool
// k 2048, r 128: the localnet config after the rank penalty (r >= 128, k >= 16r; 2026-10-17)
const JOB = { m: 64, n: 64, k: 2048, rank: 128, maxBlocks: 5, deadlineBlocks: 400 };

const PUMP = new PublicKey("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
const AMM = new PublicKey("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
const T22 = new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

const payer = loadKp(KEYS.payer);
const attester = loadKp(KEYS.attester);
const miner = loadKp(KEYS.miner);
const trader = demoKey("trader");
const user = demoKey("user");
const buyer = demoKey("buyer");
const sfWallet = demoKey("storefront-wallet");
const mintKey = demoKey("mint");
const MINT = mintKey.kp.publicKey;
const RESERVE_ID = loadKp("keys/localnet/reserve-keypair.json").publicKey;
const node = NodeRpc.http(NODE_RPC);
/** Devnet node 1: takes the job data putBlob (the miner and attester use node 0). */
const DA_RPC = "http://127.0.0.1:10632";

const BIN = {
  node: resolve(ROOT, "chain/target/release/agent-node"),
  miner: resolve(ROOT, "crates/target/release/pearl-miner"),
  attester: resolve(ROOT, "crates/target/release/pearl-attester"),
  job: resolve(ROOT, "crates/target/release/pearl-job"),
  headerRelay: resolve(ROOT, "services/header-relay/target/release/header-relay"),
  gateway: resolve(ROOT, "crates/target/release/cadre-gateway"),
  pearl: resolve(ROOT, "crates/pearl-cli/target/release/pearl"),
};
// Cadre inference (SPEC 4.5 / 5.4): the quantized model (tools/cadre/quantize.py), the worker's python,
// the model store the miner, attester and gateway read registered matrices from
const CADRE_MODEL_DIR = process.env.CADRE_MODEL_DIR ?? resolve(homedir(), "cadre-models/llama-3.2-1b-w7a7-k2048");
const CADRE_PYTHON = process.env.CADRE_PYTHON ?? resolve(homedir(), "int7-exp/venv/bin/python");
const MODEL_STORE = resolve(DEMO, "models");
const GATEWAY_URL = "http://127.0.0.1:10664";
const WORKER_URL = "http://127.0.0.1:10665";
// node blob limits: room for a registered model (about 1.7 GB for Llama-3.2-1B) and a fast local upload
const DEVNET_BLOBS = { max_total_bytes: 8_000_000_000, put_rate_bytes_per_s: 2_000_000_000, put_burst_bytes: 256 << 20 };
const HR_ARGS = ["--node", NODE_RPC, "--solana", TOKEN_RPC, "--payer", resolve(ROOT, KEYS.payer)];
const CRANK = resolve(ROOT, "services/crank");
const RELAYER = resolve(ROOT, "services/relayer");
const DEVNET_ENV = { DEVNET_DIR: resolve(DEMO, "devnet"), DEVNET_PARAMS: JSON.stringify(GENESIS_PARAMS), DEVNET_BLOBS: JSON.stringify(DEVNET_BLOBS) };

const crank = (args: string[]) => run("node", ["src/main.ts", ...args], { cwd: CRANK, env: { RPC_URL: TOKEN_RPC, KEYPAIR: resolve(ROOT, KEYS.payer) } });
/** The crank CLI signed by another key (fund_mining is permissionless since 2026-10-19). */
const crankAs = (keyFile: string, args: string[]) => run("node", ["src/main.ts", ...args], { cwd: CRANK, env: { RPC_URL: TOKEN_RPC, KEYPAIR: keyFile } });
const trade = (args: string[]) => lastJson(run("node", ["src/trade.ts", ...args], { cwd: CRANK, env: { RPC_URL: TOKEN_RPC } }));
const relayerCli = (args: string[]) => run("node", ["src/cli.ts", ...args], { cwd: RELAYER, env: relayerEnv() });
function relayerEnv(): Record<string, string> {
  return {
    TOKEN_RPC, NODE_RPC, SIGNER_KEYS: resolve(ROOT, KEYS.attester), TOKEN_PAYER: resolve(ROOT, KEYS.payer),
    AGENT_KEY: resolve(ROOT, KEYS.payer), STATE_FILE: resolve(DEMO, "relayer-state.json"), POLL_MS: "1000", HEALTH_PORT: "10660",
    // held-back orders are refunded on Solana this long after they were first held back (step 8)
    REFUND_GRACE_MS: process.env.DEMO_REFUND_GRACE_MS ?? "5000",
  };
}

// ------------------------------------------------------------ reporting

interface StepResult { step: string; ok: boolean; secs: number; detail: string; invariant: string }
const results: StepResult[] = [];
const t0 = Date.now();
const info = (msg: string, extra?: unknown) => console.log(`    ${msg}${extra === undefined ? "" : " " + JSON.stringify(extra, big)}`);
function big(_k: string, v: unknown) { return typeof v === "bigint" ? v.toString() : v; }

/** Invariant snapshot through the relayer's own (dry-run) check: vault x 1000 vs 1e18 - escrow, in-flight adjusted. */
async function invariant(): Promise<string> {
  const [cfg] = await new RpcChain("token", TOKEN_RPC).getAccounts([bridgeConfigPda()]);
  if (!cfg) return "n/a (bridge not initialized yet)";
  const r = new Relayer({ token: new RpcChain("token", TOKEN_RPC), node, signers: [], tokenPayer: payer, agentKey: payer,
    stateFile: resolve(DEMO, "invariant-probe.json"), dryRun: true, log: () => {} });
  const t = await r.tick();
  const inv = t.invariant;
  if (!inv) return `n/a (${t.errors.join("; ")})`;
  return `${inv.ok ? "OK" : "BROKEN"} vault_spl=${inv.vaultSpl} vault_x1000=${BigInt(inv.vaultSpl) * 1000n} 1e18-escrow=${inv.outsideEscrow} ` +
    `in_flight_deposits_spl=${inv.pendingDepositSpl} in_flight_withdrawals_spl=${inv.pendingWithdrawalSpl} delta=${inv.delta} strict=${inv.strictFormHolds}`;
}

const ONLY = process.env.DEMO_STEPS ? new Set(process.env.DEMO_STEPS.split(",").map((x) => x.trim())) : null;

async function step(name: string, f: () => Promise<string>): Promise<void> {
  if (ONLY && !ONLY.has(name.split(".")[0])) {
    console.log(`\n== ${name}\n   SKIP (DEMO_STEPS)`);
    return;
  }
  const s = Date.now();
  console.log(`\n== ${name}`);
  let ok = true;
  let detail = "";
  try {
    detail = await f();
  } catch (e) {
    ok = false;
    detail = String((e as Error).stack ?? e).slice(0, 3000);
  }
  let inv = "";
  try {
    inv = await invariant();
    if (inv.startsWith("BROKEN")) ok = false;
  } catch (e) {
    inv = `n/a (${String((e as Error).message).slice(0, 200)})`;
  }
  const secs = (Date.now() - s) / 1000;
  results.push({ step: name, ok, secs, detail, invariant: inv });
  console.log(`   ${ok ? "PASS" : "FAIL"} ${name} (${secs.toFixed(1)} s): ${detail}`);
  console.log(`   invariant: ${inv}`);
  if (!ok) throw new Error(`step failed: ${name}`);
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

async function airdrop(to: PublicKey, sol: number) {
  const sig = await conn.requestAirdrop(to, sol * LAMPORTS_PER_SOL);
  await conn.confirmTransaction(sig, "confirmed");
}

async function splBalance(owner: PublicKey): Promise<bigint> {
  const a = await conn.getAccountInfo(ata(owner, MINT));
  return a ? tokenAccountAmount(a.data) : 0n;
}

const disc = (n: string) => createHash("sha256").update(`global:${n}`).digest().subarray(0, 8);
const some = (b: Buffer) => Buffer.concat([Buffer.from([1]), b]);
const NONE = Buffer.from([0]);
const m = (pubkey: PublicKey, isWritable = false, isSigner = false) => ({ pubkey, isWritable, isSigner });
const pda = (seeds: Buffer[], prog: PublicKey) => PublicKey.findProgramAddressSync(seeds, prog)[0];

// ------------------------------------------------------------ agent-chain helpers

async function chainBalance(k: PublicKey): Promise<bigint> {
  return (await node.getAccount(k.toBase58())).balance;
}

async function sendChainTx(kp: Keypair, kind: number, body: Buffer, fee?: bigint): Promise<string> {
  const ci = await node.getChainInfo();
  const p = await nodeCall("getParams");
  const acct = await node.getAccount(kp.publicKey.toBase58());
  const tx = signTx(kp, { chainId: ci.chainId, from: kp.publicKey, nonce: acct.nonce, fee: fee ?? BigInt(p.min_fee), kind }, body);
  const h = await node.sendTx(tx);
  await waitFor(`chain tx ${h.slice(0, 12)} included`, async () => (await node.getTx(h))?.status === "included", 120_000);
  return h;
}

const registerAgentBody = (name: string, uri: string) => {
  const n = Buffer.from(name), u = Buffer.from(uri);
  return Buffer.concat([Buffer.from([n.length]), n, u16le(u.length), u]);
};

/** schema::order_terms_hash (chain/state): what a storefront buyer commits against. */
function orderTermsHash(poster: PublicKey, mm: number, n: number, k: number, maxBlocks: number, deadlineBlocks: bigint): Buffer {
  return Buffer.from(blake3(Buffer.concat([Buffer.from("pearl-sol:order-terms:v1"), poster.toBuffer(), u32le(mm), u32le(n), u32le(k),
    u32le(maxBlocks), u64le(deadlineBlocks)])));
}

async function depositFinal(nonce: bigint): Promise<any> {
  return waitFor(`deposit ${nonce} unlocked and final on the chain`, async () => {
    const d = await node.getDeposit(nonce);
    return d.processed && d.final ? d : null;
  }, 240_000);
}

async function bridgeNonce(): Promise<bigint> {
  const a = await conn.getAccountInfo(bridgeConfigPda());
  return decodeBridgeConfig(a!.data).depositNonce;
}

/** Admin `set_release_modes` (bridge) / `set_finalize_modes` (storefront): proof path only. */
async function setProofMode(program: PublicKey, ix: string, config: PublicKey): Promise<void> {
  await send([new TransactionInstruction({ programId: program, keys: [m(payer.publicKey, true, true), m(config, true)],
    data: Buffer.concat([disc(ix), Buffer.from([MODE_PROOF]), LIGHTCLIENT_ID.toBuffer()]) })], [payer]);
}

/** The instruction (by Anchor name) of `program` in the newest tx touching `addr`. */
async function lastIxName(addr: PublicKey, program: PublicKey, names: string[]): Promise<string | null> {
  // newest first; later txs touching the account (e.g. a buyback crank's settle_share on an
  // Order) are skipped until one runs a named instruction
  for (const sig of await conn.getSignaturesForAddress(addr, { limit: 20 })) {
    const tx = await conn.getTransaction(sig.signature, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
    if (!tx) continue;
    const keys = tx.transaction.message.staticAccountKeys;
    for (const ci of tx.transaction.message.compiledInstructions) {
      if (!keys[ci.programIdIndex].equals(program)) continue;
      const d = Buffer.from(ci.data).subarray(0, 8);
      for (const n of names) if (d.equals(disc(n))) return `${n} (${sig.signature.slice(0, 16)}...)`;
    }
  }
  return null;
}

/** Light-client tip / final height from its Config account. */
async function lcHeights(): Promise<{ tip: number; final: number } | null> {
  const r = JSON.parse(run(BIN.headerRelay, [...HR_ARGS, "status"]).trim().split("\n").pop()!);
  return r.initialized ? { tip: r.tip_height, final: r.final_height } : null;
}

// ------------------------------------------------------------ steps

async function step1(): Promise<string> {
  // token chain
  console.log(run("bash", ["scripts/localnet.sh", "up", "token", "--ticks-per-slot", TICKS_PER_SLOT]).trim());
  for (const k of [payer.publicKey, trader.kp.publicKey, user.kp.publicKey, buyer.kp.publicKey, sfWallet.kp.publicKey]) await airdrop(k, 100);
  // agent chain: 3-node devnet from a fresh genesis
  console.log(run("bash", ["scripts/devnet.sh", "init"], { env: DEVNET_ENV }).trim());
  console.log(run("bash", ["scripts/devnet.sh", "start"], { env: DEVNET_ENV }).trim());
  const params = await waitFor("agent-chain node RPC", () => nodeCall("getParams"), 60_000);
  if (params.attesters.length !== 1 || params.attesters[0] !== attester.publicKey.toBase58() || params.attester_threshold !== 1) {
    throw new Error(`genesis attesters ${JSON.stringify(params.attesters)} are not the attester key 1-of-1`);
  }
  if (params.finality_k !== FINALITY_K) throw new Error(`finality_k ${params.finality_k}`);
  info("genesis params", { finality_k: params.finality_k, target_block_time_ms: params.target_block_time_ms, emission_ppm: params.emission_ppm,
    alpha_ppm: params.alpha_ppm, attesters: params.attesters });
  const esc = await nodeCall("getEscrow");
  if (esc.escrow_balance !== "1000000000000000000") throw new Error(`escrow at genesis ${esc.escrow_balance}`);
  const pool = new PublicKey(esc.pool);

  // services that do not need Solana state yet
  mkdirSync(MODEL_STORE, { recursive: true });
  start("attester", BIN.attester, ["--keypair", KEYS.attester, "--port", "10661", "--rpc", NODE_RPC, "--mode", "node", "--models", MODEL_STORE]);
  start("miner", BIN.miner, ["--rpc", NODE_RPC, "--payout", KEYS.miner, "--deliver", "--attester", ATTESTER_URL,
    "--results-dir", resolve(DEMO, "results"), "--threads", MINER_THREADS, "--stats-secs", "30", "--models", MODEL_STORE]);
  await waitFor("attester /health", async () => (await httpJson(`${ATTESTER_URL}/health`)).body?.ok === true, 30_000);
  await waitFor("first mined blocks", async () => (await node.getChainInfo()).height >= 3, 120_000);

  // reserve: init, then launch with the creator PDA + dev buy
  console.log(crank(["init", "--minIntervalSecs", "1", "--minBuy", "100000", "--bridgeProgram", BRIDGE_PROGRAM_ID.toBase58(),
    "--miningPool", pool.toBase58()]).trim());
  console.log(crank(["launch", "--name", "Pearl Demo", "--symbol", "PRLD", "--uri", "https://example.invalid/pearl.json",
    "--devBuy", DEV_BUY.toString(), "--mintKeypair", mintKey.file]).trim());
  const status = JSON.parse(crank(["status"]));
  if (status.mint !== MINT.toBase58() || BigInt(status.reserveTokens) === 0n) throw new Error(`reserve after launch: ${JSON.stringify(status)}`);
  info("reserve", { mint: status.mint, creatorPda: status.creatorPda, reserveTokens: status.reserveTokens, miningPool: status.miningPool });

  // bridge: mint + attester signer set, 1 of 1
  console.log(relayerCli(["init-bridge", MINT.toBase58(), "1", attester.publicKey.toBase58()]).trim());

  // storefront: config (mint, bridge, attester 1 of 1) + the trader PDA's volume accumulators
  const sfCfg = storefrontConfigPda();
  const trader_ = traderPda();
  const params_ = Buffer.concat([some(MINT.toBuffer()), some(BRIDGE_PROGRAM_ID.toBuffer()), NONE, NONE, NONE, NONE, NONE, NONE, NONE, NONE]);
  await send([new TransactionInstruction({
    programId: STOREFRONT_PROGRAM_ID,
    keys: [m(payer.publicKey, true, true), m(sfCfg, true), m(trader_, true), m(SystemProgram.programId)],
    data: Buffer.concat([disc("initialize"), params_, u32le(1), attester.publicKey.toBuffer(), Buffer.from([1])]),
  })], [payer]);
  for (const prog of [PUMP, AMM]) {
    await send([new TransactionInstruction({
      programId: prog,
      keys: [m(payer.publicKey, true, true), m(trader_), m(pda([Buffer.from("user_volume_accumulator"), trader_.toBuffer()], prog), true),
        m(SystemProgram.programId), m(pda([Buffer.from("__event_authority")], prog)), m(prog)],
      data: Buffer.from([94, 6, 202, 115, 255, 96, 232, 183]), // init_user_volume_accumulator
    })], [payer]);
  }

  // storefront buyback: the vault PDA's pump + PumpSwap volume accumulators (once), and
  // no minimum interval so the demo can show two buys back to back (default 600 s),
  console.log(crank(["sf-setup"]).trim());
  // and a lower min buy: the demo order's share is about 0.001 SOL and buy 1 spends half of it
  console.log(crank(["sf-set", "--minIntervalSecs", "0", "--minBuy", "100000"]).trim());

  // relayer + storefront service
  start("relayer", "node", ["src/main.ts"], { cwd: RELAYER, env: relayerEnv() });
  start("storefront-x402", "node", ["src/main.ts"], { cwd: resolve(ROOT, "services/storefront-x402"), env: {
    TOKEN_RPC, STOREFRONT_PORT: "10662", STOREFRONT_WALLET: sfWallet.file, STATE_FILE: resolve(DEMO, "storefront-state.json"), AGENT_RPC: NODE_RPC } });
  await waitFor("relayer /health ok", async () => (await httpJson(RELAYER_HEALTH)).body?.ok === true, 60_000);
  await waitFor("storefront /health", async () => (await httpJson(`${STOREFRONT_URL}/health`)).body?.ok === true, 30_000);
  const ci = await node.getChainInfo();
  return `token :10600, devnet 3 nodes (height ${ci.height}, peers ${(await nodeCall("getChainInfo")).peers}), mint ${MINT.toBase58()}, ` +
    `pool ${pool.toBase58()}, services up (relayer, attester, miner, storefront-x402)`;
}

async function step2(): Promise<string> {
  // curve volume from a funded trader: two round trips
  const mint = MINT.toBase58();
  const b1 = trade(["buy", "--keypair", trader.file, "--mint", mint, "--sol", String(3 * LAMPORTS_PER_SOL)]);
  const s1 = trade(["sell", "--keypair", trader.file, "--mint", mint, "--fraction", "0.5"]);
  const b2 = trade(["buy", "--keypair", trader.file, "--mint", mint, "--sol", String(2 * LAMPORTS_PER_SOL)]);
  const s2 = trade(["sell", "--keypair", trader.file, "--mint", mint, "--fraction", "0.5"]);
  info("trades", { buy1: b1.tokens, sell1: s1.tokens, buy2: b2.tokens, sell2: s2.tokens, curveCreatorFee: s2.creatorFee });
  if (BigInt(s2.creatorFee) === 0n) throw new Error("no creator fee accrued");
  const before = BigInt(JSON.parse(crank(["status"])).reserveTokens);
  await sleep(1500); // min_interval_secs = 1 since launch
  const once = crank(["once"]).trim();
  console.log(once);
  const after = BigInt(JSON.parse(crank(["status"])).reserveTokens);
  if (after <= before) throw new Error(`buyback did not add reserve tokens (${before} -> ${after}): ${once}`);
  // fund_mining into the bridge, recipient = the chain's mining pool. Permissionless and
  // rate limited (owner decision 2026-10-18): the trader key, not the admin, calls it.
  const nonce = await bridgeNonce();
  const poolBefore = BigInt((await nodeCall("getEscrow")).pool_balance);
  console.log(crankAs(trader.file, ["fund", "--amount", FUND_SPL.toString()]).trim());
  // a second call inside fund_min_interval_secs (default 3600 s): the CLI reads the on-chain limit and reports the wait
  const again = crankAs(trader.file, ["fund", "--amount", "1"]).trim();
  if (!/wait \d+s/.test(again)) throw new Error(`second fund_mining inside the interval: ${again}`);
  const d = await depositFinal(nonce);
  const esc = await nodeCall("getEscrow");
  const poolGain = BigInt(esc.pool_balance) - poolBefore;
  if (d.recipient !== esc.pool || poolGain < FUND_SPL * 1000n) throw new Error(`pool unlock: ${JSON.stringify(d)} gain ${poolGain}`);
  return `creator fee ${s2.creatorFee} lamports, buyback +${after - before} reserve tokens, fund_mining ${FUND_SPL} SPL ` +
    `(deposit ${nonce}) unlocked at height ${d.height} to pool ${esc.pool}, pool balance ${esc.pool_balance}`;
}

async function step3(): Promise<string> {
  const mint = MINT.toBase58();
  const b = trade(["buy", "--keypair", user.file, "--mint", mint, "--sol", String(LAMPORTS_PER_SOL / 2)]);
  const amount = BigInt(b.tokens) / 2n;
  const nonce = await bridgeNonce();
  console.log(relayerCli(["deposit", amount.toString(), user.kp.publicKey.toBase58(), user.file]).trim());
  const d = await depositFinal(nonce);
  const bal = await chainBalance(user.kp.publicKey);
  if (bal !== amount * 1000n) throw new Error(`user chain balance ${bal}, want ${amount * 1000n}`);
  const reg = await sendChainTx(user.kp, KIND.RegisterAgent, registerAgentBody("demo-agent", "https://example.invalid/agent.json"));
  const agent = await nodeCall("getAgent", { account: user.kp.publicKey.toBase58() });
  if (!agent || agent.name !== "demo-agent") throw new Error(`getAgent: ${JSON.stringify(agent)}`);
  // fund the miner's payout key so it can pay SubmitResult / CompleteJob fees before its rewards mature
  const fund = 5_000_000_000n;
  const minerBefore = await chainBalance(miner.publicKey);
  await sendChainTx(user.kp, KIND.Transfer, transferBody(miner.publicKey, fund));
  const minerAfter = await chainBalance(miner.publicKey);
  if (minerAfter < minerBefore + fund) throw new Error(`miner funding ${minerBefore} -> ${minerAfter}`);
  return `user bought ${b.tokens}, deposited ${amount} SPL (nonce ${nonce}) -> ${bal} chain units at height ${d.height}; ` +
    `RegisterAgent ${reg.slice(0, 16)} (agent "${agent.name}" at ${agent.registered_height}); miner payout funded with ${fund}`;
}

async function stepLightClient(): Promise<string> {
  // checkpoint = the node's current final header (before the order is posted, so its
  // job can be registered from its posting tx); then relay + settle by proof
  const out = run(BIN.headerRelay, [...HR_ARGS, "init"]).trim();
  console.log(out);
  // localnet only: the --ticks-per-slot validator's clock falls behind wall time under load and
  // the light client refuses headers more than max_future_ms (120 s, the production default)
  // ahead of it, which caps the relay at ~19 headers/min; allow an hour here
  console.log(run(BIN.headerRelay, [...HR_ARGS, "set-params", "--max-future-ms", "3600000"]).trim());
  start("header-relay", BIN.headerRelay, [...HR_ARGS, "run", "--settle", "--poll-ms", "1000", "--work", resolve(DEMO, "header-relay-work.json")]);
  await setProofMode(STOREFRONT_PROGRAM_ID, "set_finalize_modes", storefrontConfigPda());
  const lc0 = (await lcHeights())!;
  const h = await waitFor("light client follows the chain", async () => {
    const x = await lcHeights();
    return x && x.tip >= lc0.tip + 3 ? x : null;
  }, 120_000);
  return `${out}; header-relay running (--settle); storefront finalize mode = proof only; light client tip ${h.tip} final ${h.final}`;
}

let order: { nonce: bigint; jobId: string; P: bigint; W: bigint; postedHeight: number } | null = null;

async function step4(): Promise<string> {
  const g = lastJson(run(BIN.job, ["gen", "--m", String(JOB.m), "--n", String(JOB.n), "--k", String(JOB.k), "--out-dir", resolve(DEMO, "job"), "--seed", "7"]));
  // M2 data availability: the job data goes into the chain's blob store on devnet node 1
  // (10632); the miner and attester talk to node 0 and fetch it with getBlob (p2p push).
  const put = await nodeCall("putBlob", { data: readFileSync(g.blob_file).toString("base64") }, DA_RPC);
  if (put.uri !== g.blob_uri) throw new Error(`putBlob uri ${put.uri} != pearl-job's ${g.blob_uri}`);
  await waitFor("job blob on the miner's node", async () => ((await nodeCall("hasBlob", { blob_id: put.blob_id })).present ? true : null), 20_000);
  const dataUri: string = put.uri;
  info("job data blob", { uri: put.uri, size: put.size, put_on: DA_RPC });
  const cfg = lastJson(run(BIN.job, ["config", "--k", String(JOB.k), "--rank", String(JOB.rank)])).config as string;
  const recipient = buyer.kp.publicKey;
  const terms = orderTermsHash(recipient, JOB.m, JOB.n, JOB.k, JOB.maxBlocks, BigInt(JOB.deadlineBlocks));
  const c = lastJson(run(BIN.job, ["commit", "--job", recipient.toBase58(), "--terms-hash", terms.toString("hex"), "--config", cfg, "--data", g.data_uri]));
  const body = { spec: { m: JOB.m, n: JOB.n, k: JOB.k, config: cfg, hash_a: c.hash_a, hash_b: c.hash_b, data_uri: dataUri,
    deadline_blocks: JOB.deadlineBlocks, max_blocks: JOB.maxBlocks } };
  const quote = await httpJson(`${STOREFRONT_URL}/quote?m=${JOB.m}&n=${JOB.n}&k=${JOB.k}&max_blocks=${JOB.maxBlocks}`);
  info("quote", { price_lamports: quote.body.price_lamports, venue: quote.body.venue, tokens_spl: quote.body.estimate?.tokens_spl });
  lastOrderBody = body;
  const r = await payOrder(body);
  const nonce = BigInt(r.body.id);
  // relayer: BridgeUnlock with job_order; the job shows up on the chain
  const d = await depositFinal(nonce);
  const job = await waitFor("getJob for the order", () => nodeCall("getJob", { job_id: r.body.job_id }), 60_000);
  if (job.poster !== recipient.toBase58() || job.hash_a !== c.hash_a || job.hash_b !== c.hash_b) throw new Error(`job mismatch ${JSON.stringify(job)}`);
  if (job.job_key !== c.job_key) throw new Error(`node job_key ${job.job_key} != buyer's ${c.job_key}`);
  const o = decodeOrder((await conn.getAccountInfo(orderPda(nonce)))!.data);
  order = { nonce, jobId: job.job_id, P: BigInt(job.protocol_fee), W: BigInt(job.worker_pay), postedHeight: job.posted_height };
  // its buyback share is held for it (refunded if the order is), not crankable yet
  const sh = (await conn.getAccountInfo(sharePda(nonce)))!;
  const held = sh.data.readBigUInt64LE(48); // ShareRecord: disc, nonce, buyer [32], lamports
  const ss = JSON.parse(crank(["sf-status"]));
  if (held === 0n || BigInt(ss.heldLamports) < held || ss.availableLamports !== "0") throw new Error(`share not held: ${held} ${JSON.stringify(ss)}`);
  info("buyback share held", { nonce: nonce.toString(), lamports: held.toString(), crankable: ss.availableLamports });
  writeFileSync(resolve(DEMO, "order.json"), JSON.stringify({ ...order, nonce: nonce.toString(), P: order.P.toString(), W: order.W.toString() }));
  return `order ${nonce} (Solana Order ${orderPda(nonce).toBase58()}, ${o.tokens} SPL bought) unlocked at height ${d.height}: ` +
    `job ${job.job_id.slice(0, 16)} ${job.status} W=${job.worker_pay} P=${job.protocol_fee} cap=${job.cap_remaining} ` +
    `deadline ${job.deadline_height} data ${job.data_uri} (blob put on node 1)`;
}

let lastOrderBody: unknown = null;

/** x402 purchase of `body` through storefront-x402 (402 challenge, then the buyer-signed payment); returns the 201 response. */
async function payOrder(body: unknown): Promise<any> {
  const recipient = buyer.kp.publicKey;
  // x402: first call answers 402 with the payment requirements
  const r402 = await httpJson(`${STOREFRONT_URL}/jobs`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  if (r402.status !== 402) throw new Error(`expected 402, got ${r402.status} ${JSON.stringify(r402.body)}`);
  const req = r402.body.accepts[0];
  // exact native payment: service = fee payer, buyer signs the transfer
  const { blockhash } = await conn.getLatestBlockhash();
  const msg = new TransactionMessage({ payerKey: new PublicKey(req.extra.feePayer), recentBlockhash: blockhash, instructions: [
    ComputeBudgetProgram.setComputeUnitLimit({ units: 10_000 }),
    SystemProgram.transfer({ fromPubkey: recipient, toPubkey: new PublicKey(req.payTo), lamports: BigInt(req.maxAmountRequired) }),
  ] }).compileToV0Message();
  const vtx = new VersionedTransaction(msg);
  vtx.sign([buyer.kp]);
  const xPayment = Buffer.from(JSON.stringify({ x402Version: 1, scheme: "exact", network: req.network,
    payload: { transaction: Buffer.from(vtx.serialize()).toString("base64") } })).toString("base64");
  const buyerSolBefore = await conn.getBalance(recipient);
  const r = await httpJson(`${STOREFRONT_URL}/jobs`, { method: "POST", headers: { "content-type": "application/json", "x-payment": xPayment }, body: JSON.stringify(body) });
  if (r.status !== 201) throw new Error(`POST /jobs with payment: ${r.status} ${JSON.stringify(r.body)}`);
  info("order placed", { nonce: r.body.id, order: r.body.order, job_id: r.body.job_id, venue: r.body.venue, paid: req.maxAmountRequired,
    buyerSolSpent: buyerSolBefore - (await conn.getBalance(recipient)) });
  return r;
}

async function step5(): Promise<string> {
  if (!order) throw new Error("no order from step 4");
  const p = await nodeCall("getParams");
  const cap = (order.P * BigInt(p.alpha_ppm)) / 1_000_000n;
  const job = await waitFor("job mined, delivered and completed", async () => {
    const j = await nodeCall("getJob", { job_id: order!.jobId });
    return j.status === "completed" ? j : null;
  }, 600_000, 2000);
  // scan the chain for job blocks and fallback blocks since the job was posted
  const ci = await node.getChainInfo();
  const jobBlocks: number[] = [];
  let fallbackAfter = 0;
  let fallbackBefore = 0;
  for (let h = 1; h <= ci.height; h++) {
    const b = await nodeCall("getBlock", { height: h });
    const jid = b.header_fields.job_id as string;
    if (jid === order.jobId) jobBlocks.push(h);
    else if (/^0+$/.test(jid)) h > order.postedHeight ? fallbackAfter++ : fallbackBefore++;
  }
  const subsidy = BigInt(job.subsidy_paid);
  info("job", { blocks_mined: job.blocks_mined, job_blocks: jobBlocks, subsidy_paid: job.subsidy_paid, cap_alpha_x_P: cap.toString(),
    cap_remaining: job.cap_remaining, deliverer: job.deliverer, result_hash: job.result_hash, closed_height: job.closed_height,
    fallback_blocks_before_job: fallbackBefore, fallback_blocks_after_job: fallbackAfter });
  if (jobBlocks.length === 0 || jobBlocks.length !== job.blocks_mined) throw new Error(`job blocks ${jobBlocks.length} vs blocks_mined ${job.blocks_mined}`);
  if (subsidy === 0n || subsidy > cap || BigInt(job.cap_remaining) !== cap - subsidy) throw new Error(`subsidy ${subsidy} vs cap ${cap}`);
  const poolNow = BigInt((await nodeCall("getEscrow")).pool_balance);
  const uncapped = (poolNow * BigInt(p.emission_ppm)) / 1_000_000n;
  info("cap check", { pool_now: poolNow.toString(), pool_x_emission_per_block: uncapped.toString(), cap: cap.toString() });
  if (fallbackBefore === 0) throw new Error("no fallback blocks seen");
  if (job.deliverer !== miner.publicKey.toBase58()) throw new Error(`deliverer ${job.deliverer}`);
  // M2: the miner delivered C as a blob (putBlob) and getBlock reports each job block's subsidy
  if (!/^blob:[0-9a-f]{64}$/.test(job.result_uri)) throw new Error(`result_uri ${job.result_uri} is not a blob: URI`);
  const cBlob = await nodeCall("hasBlob", { blob_id: job.result_uri }, DA_RPC);
  if (!cBlob.present) throw new Error(`result blob not on node 1: ${JSON.stringify(cBlob)}`);
  let blockSubsidy = 0n;
  for (const h of jobBlocks) blockSubsidy += BigInt((await nodeCall("getBlock", { height: h })).subsidy);
  if (blockSubsidy !== subsidy) throw new Error(`getBlock subsidy sum ${blockSubsidy} != job subsidy_paid ${subsidy}`);
  info("data availability", { data_uri: job.data_uri, result_uri: job.result_uri, result_blob_size: cBlob.size, getBlock_subsidy_sum: blockSubsidy.toString() });
  // relayer finalizes the order on Solana once the completion is K deep
  const pda_ = orderPda(order.nonce);
  const acc = await waitFor("finalize_order on Solana", async () => {
    const a = await conn.getAccountInfo(pda_);
    return a && a.data[ORDER_LAYOUT.STATUS] === ORDER_STATUS_FINALIZED ? a : null;
  }, 300_000);
  const how = await lastIxName(pda_, STOREFRONT_PROGRAM_ID, ["finalize_order_proof", "finalize_order"]);
  if (!how?.startsWith("finalize_order_proof")) throw new Error(`Order finalized by ${how}, not by light-client proof`);
  const rh = Buffer.from(acc.data.subarray(ORDER_LAYOUT.RESULT_HASH, ORDER_LAYOUT.RESULT_HASH + 32)).toString("hex");
  const deliverer = new PublicKey(acc.data.subarray(ORDER_LAYOUT.DELIVERER, ORDER_LAYOUT.DELIVERER + 32)).toBase58();
  if (rh !== job.result_hash || deliverer !== job.deliverer) throw new Error(`Order on Solana: result_hash ${rh} deliverer ${deliverer}`);
  const svc = await httpJson(`${STOREFRONT_URL}/jobs/${order.nonce}`);
  const heightNow = (await node.getChainInfo()).height;
  return `${job.blocks_mined} job block(s) at ${jobBlocks.join(",")}, subsidy ${subsidy} = cap alpha x P ${cap} ` +
    `(pool x emission per block now ${uncapped}), ${fallbackBefore} fallback blocks before / ${fallbackAfter} after; completed at ${job.closed_height}, ` +
    `finalized on Solana by ${how} by chain height ${heightNow} (K=${FINALITY_K}); Order ${pda_.toBase58()} result_hash ${rh} (GET /jobs/${order.nonce}: ${svc.body?.status ?? svc.status})`;
}

async function step6(): Promise<string> {
  if (!order) throw new Error("no order");
  const job = await nodeCall("getJob", { job_id: order.jobId });
  const lastJobBlock = job.closed_height; // completion block; subsidies were paid in earlier job blocks
  await waitFor("miner rewards matured (K past the last job block)", async () => (await node.getChainInfo()).height >= lastJobBlock + FINALITY_K + 1, 120_000);
  const bal = await chainBalance(miner.publicKey);
  const lockAmount = ((bal / 2n) / 1000n) * 1000n;
  if (lockAmount === 0n) throw new Error(`miner balance ${bal}`);
  await setProofMode(BRIDGE_PROGRAM_ID, "set_release_modes", bridgeConfigPda());
  const to = miner.publicKey; // same ed25519 key on Solana
  const splBefore = await splBalance(to);
  const out = relayerCli(["lock", lockAmount.toString(), to.toBase58(), resolve(ROOT, KEYS.miner)]).trim();
  console.log(out);
  const hash = out.split(" ").pop()!;
  await waitFor("BridgeLock included", async () => (await node.getTx(hash))?.status === "included", 120_000);
  const lockHeight = (await node.getTx(hash))!.height!;
  const got = await waitFor("SPL released on Solana", async () => {
    const b = await splBalance(to);
    return b > splBefore ? b : null;
  }, 600_000);
  const heightAtRelease = (await node.getChainInfo()).height;
  const ev = await nodeCall("getBridgeEvents", { from_nonce: 0, limit: 100 });
  const wd = (ev.withdrawals as any[]).find((x) => x.solana_recipient === to.toBase58() && BigInt(x.amount_spl) === lockAmount / 1000n);
  if (!wd) throw new Error(`withdrawal for ${to.toBase58()} not in getBridgeEvents`);
  const wNonce = BigInt(wd.nonce);
  const processed = PublicKey.findProgramAddressSync([Buffer.from("processed"), Buffer.from([1]), u64le(wNonce)], BRIDGE_PROGRAM_ID)[0];
  const how = await lastIxName(processed, BRIDGE_PROGRAM_ID, ["release_proof", "release"]);
  if (!how?.startsWith("release_proof")) throw new Error(`withdrawal ${wNonce} released by ${how}, not by light-client proof`);
  const lc = await lcHeights();
  if (got - splBefore !== lockAmount / 1000n) throw new Error(`released ${got - splBefore}, want ${lockAmount / 1000n}`);
  return `miner balance ${bal}, BridgeLock ${lockAmount} at height ${lockHeight}, released ${lockAmount / 1000n} SPL to ${to.toBase58()} ` +
    `(ATA ${ata(to, MINT).toBase58()}) by ${how} (bridge release mode = proof only) by chain height ${heightAtRelease}; light client tip ${lc?.tip} final ${lc?.final}`;
}

/**
 * Storefront buyback (SYNTHESIS item 2, 2026-10-18): the x402 order of step 5 paid
 * buyback_bps of its sol_in into the storefront's buyback vault (an on-chain rule in
 * buy_job, not a promise). The permissionless crank buys the token with it through the
 * same pump path as the reserve's creator-fee buyback: first half the vault into the
 * mining reserve (per-call cap, the rest carries), then the rest burned. Each buy's
 * BuybackExecuted event (SOL in, tokens out, destination, order nonces covered) is read
 * back from the on-chain tx logs; totals from the config; the bridge vault is untouched.
 */
async function stepBuyback(): Promise<string> {
  if (!order) throw new Error("no order from step 5");
  const sfStatus = () => JSON.parse(crank(["sf-status"]));
  // settle_share: the order is finalized (step 7), so its share is released to the buyback
  const settled = JSON.parse(crank(["sf-settle"]).trim().split("\n").pop()!);
  if (!settled.released.includes(order.nonce.toString())) throw new Error(`settle: ${JSON.stringify(settled)}`);
  const s0 = sfStatus();
  info("buyback vault after the order", { vault: s0.vault, available: s0.availableLamports, bps: s0.buybackBps, destination: s0.destination,
    pendingOrders: s0.pendingOrders, pendingFirstNonce: s0.pendingFirstNonce, pendingLastNonce: s0.pendingLastNonce });
  const solSpent = (await conn.getAccountInfo(orderPda(order.nonce)))!.data.readBigUInt64LE(148); // Order.sol_spent (storefront::layout)
  const avail = BigInt(s0.availableLamports);
  if (s0.pendingOrders !== "1" || s0.pendingLastNonce !== order.nonce.toString()) throw new Error(`pending ${JSON.stringify(s0)}`);
  if (avail === 0n || BigInt(s0.accrued) !== avail) throw new Error(`vault available ${avail}, accrued ${s0.accrued}`);
  // the order's share = sol_in x bps / 10000, sol_in = the job's spend + the share (x402 grossed it up)
  if (avail * 10_000n < BigInt(s0.buybackBps) * (solSpent + avail) - 10_000n) throw new Error(`share ${avail} vs order sol_spent ${solSpent}`);
  const x = (await httpJson(`${STOREFRONT_URL}/buyback`)).body;
  if (x.available_lamports !== s0.availableLamports || x.pending.last_nonce !== order.nonce.toString()) throw new Error(`GET /buyback ${JSON.stringify(x)}`);
  const RESERVE_CREATOR = PublicKey.findProgramAddressSync([Buffer.from("creator")], RESERVE_ID)[0];
  const reserveAta = ata(RESERVE_CREATOR, MINT);
  if (s0.destinationAccount !== reserveAta.toBase58()) throw new Error(`destination ${s0.destinationAccount} != reserve ATA ${reserveAta.toBase58()}`);
  const bridgeVault = ata(bridgeConfigPda(), MINT);
  const vaultSpl0 = tokenAccountAmount((await conn.getAccountInfo(bridgeVault))!.data);
  const supply = async () => (await conn.getAccountInfo(MINT))!.data.readBigUInt64LE(36);
  const supply0 = await supply();
  const reserve0 = await splBalance(RESERVE_CREATOR);

  // buy 1: half the vault (per-call cap) into the mining reserve
  const half = avail / 2n;
  console.log(crank(["sf-set", "--maxPerCall", half.toString()]).trim());
  const b1 = JSON.parse(crank(["sf-buyback"]).trim().split("\n").pop()!);
  if (!b1.ok || b1.destination !== "reserve" || b1.venue !== "curve") throw new Error(`buy 1: ${JSON.stringify(b1)}`);
  if (BigInt(b1.solIn) > half || BigInt(b1.carry) !== avail - BigInt(b1.solIn) || b1.orders !== "1" || b1.lastNonce !== order.nonce.toString()) throw new Error(`buy 1 event ${JSON.stringify(b1)}`);
  const reserve1 = await splBalance(RESERVE_CREATOR);
  if (reserve1 - reserve0 !== BigInt(b1.routed) || BigInt(b1.tokensOut) === 0n) throw new Error(`reserve +${reserve1 - reserve0}, event routed ${b1.routed}`);
  // the event comes from the on-chain tx: a storefront crank_buyback instruction with its log
  const tx1 = await conn.getTransaction(b1.sig, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
  const k1 = tx1!.transaction.message.staticAccountKeys;
  const isCrank = tx1!.transaction.message.compiledInstructions.some((ci) => k1[ci.programIdIndex].equals(STOREFRONT_PROGRAM_ID) && Buffer.from(ci.data).subarray(0, 8).equals(disc("crank_buyback")));
  if (!isCrank || !tx1!.meta!.logMessages!.some((l) => l.startsWith("Program data: "))) throw new Error(`tx ${b1.sig} is not a crank_buyback with an event`);
  info("buy 1 (reserve)", b1);

  // buy 2: the carry, under the reserve cap rule (owner decision 2026-10-18): the cap is set
  // 1000 tokens above the reserve's balance, so 1000 tokens go to the reserve and the rest is
  // burned (SPL burn) in the same instruction
  const ROOM = 1_000_000_000n;
  console.log(crank(["sf-set", "--maxPerCall", "0", "--reserveCap", (reserve1 + ROOM).toString()]).trim());
  const b2 = JSON.parse(crank(["sf-buyback"]).trim().split("\n").pop()!);
  if (!b2.ok || b2.orders !== "0" || BigInt(b2.availableBefore) !== BigInt(b1.carry)) throw new Error(`buy 2: ${JSON.stringify(b2)}`);
  if (BigInt(b2.toReserve) !== ROOM || BigInt(b2.toReserve) + BigInt(b2.burned) !== BigInt(b2.routed) || b2.burnReason !== "above cap") throw new Error(`buy 2 split: ${JSON.stringify(b2)}`);
  const supply2 = await supply();
  if (supply0 - supply2 !== BigInt(b2.burned) || BigInt(b2.burned) === 0n || BigInt(b2.supplyAfter) !== supply2) throw new Error(`supply ${supply0} -> ${supply2}, burned ${b2.burned}`);
  if ((await splBalance(RESERVE_CREATOR)) !== reserve1 + ROOM) throw new Error("reserve not exactly at the cap");
  info("buy 2 (reserve to the cap, rest burned)", b2);
  // back to the provisional defaults: 1 SOL per call, cap 100M tokens (interval and min
  // buy stay at the demo's values)
  console.log(crank(["sf-set", "--maxPerCall", "1000000000", "--reserveCap", "100000000000000"]).trim());

  const s2 = sfStatus();
  if (s2.buys !== "2" || BigInt(s2.toReserve) !== BigInt(b1.routed) + ROOM || BigInt(s2.burned) !== BigInt(b2.burned) || s2.pendingOrders !== "0") throw new Error(`totals ${JSON.stringify(s2)}`);
  if (BigInt(s2.spent) !== BigInt(b1.solIn) + BigInt(b2.solIn)) throw new Error(`spent ${s2.spent}`);
  // bridge vault untouched; after a burn the chain-side supply outside the escrow is still
  // backed: 1e18 - escrow <= 1000 x mint supply (burned tokens can never be bridged in)
  const vaultSpl2 = tokenAccountAmount((await conn.getAccountInfo(bridgeVault))!.data);
  if (vaultSpl2 !== vaultSpl0) throw new Error(`bridge vault moved ${vaultSpl0} -> ${vaultSpl2}`);
  const esc = await nodeCall("getEscrow");
  const outside = BigInt(esc.total_supply) - BigInt(esc.escrow_balance);
  if (outside > supply2 * 1000n) throw new Error(`outside escrow ${outside} > 1000 x supply ${supply2}`);
  return `order ${order.nonce} paid ${avail} lamports (${s0.buybackBps} bps of sol_in) into vault ${s0.vault}; ` +
    `buy 1 ${b1.solIn} lamports -> ${b1.tokensOut} SPL to the mining reserve ${reserveAta.toBase58()} (orders ${b1.firstNonce}..${b1.lastNonce}, carry ${b1.carry}, ${b1.sig.slice(0, 16)}...); ` +
    `buy 2 ${b2.solIn} lamports -> ${b2.tokensOut} SPL: ${b2.toReserve} to the reserve up to its cap, ${b2.burned} burned, mint supply ${supply0} -> ${supply2} (${b2.sig.slice(0, 16)}...); ` +
    `config totals: ${s2.buys} buys, spent ${s2.spent}, to reserve ${s2.toReserve}, burned ${s2.burned}; GET /buyback agrees; ` +
    `bridge vault unchanged (${vaultSpl2} SPL), 1e18 - escrow ${outside} <= 1000 x supply`;
}

/**
 * Refund path (owner decision 2026-10-17): the chain's min protocol fee is raised by
 * AdminConfig above any order's P (an order paid on Solana is now underpaid for the
 * chain), a new x402 order is held back by the relayer (never signed), and once
 * REFUND_GRACE_MS passes the relayer refunds it on Solana: bridge refund_deposit back
 * to the Order's buyer + storefront mark_refunded. The deposit is never unlocked, even
 * after the floor is restored, and the invariant is exact.
 */
async function stepRefund(): Promise<string> {
  if (!lastOrderBody) throw new Error("no order body from step 5");
  const p0 = await nodeCall("getParams");
  const key = Number(p0.keys.min_protocol_fee);
  const admin = payer; // devnet admin = keys/localnet/payer
  if ((await chainBalance(admin.publicKey)) < 10_000n) await sendChainTx(user.kp, KIND.Transfer, transferBody(admin.publicKey, 1_000_000n));
  const adminConfig = (v: bigint) => Buffer.concat([Buffer.from([key]), u16le(8), u64le(v)]);
  const HIGH = 10n ** 16n;
  await sendChainTx(admin, KIND.AdminConfig, adminConfig(HIGH));
  await waitFor("min_protocol_fee_effective raised", async () => (BigInt((await nodeCall("getParams")).min_protocol_fee_effective) === HIGH ? true : null), 60_000);
  const r = await payOrder(lastOrderBody);
  const nonce = BigInt(r.body.id);
  const o = decodeOrder((await conn.getAccountInfo(orderPda(nonce)))!.data);
  if (o.body.protocolFee >= HIGH) throw new Error(`order P ${o.body.protocolFee} not below the raised floor`);
  const buyerAta = ata(o.buyer, MINT);
  const share = (await conn.getAccountInfo(sharePda(nonce)))!.data.readBigUInt64LE(48);
  const refunded0 = BigInt(JSON.parse(crank(["sf-status"])).refunded);
  const before = (await conn.getAccountInfo(buyerAta)) ? tokenAccountAmount((await conn.getAccountInfo(buyerAta))!.data) : 0n;
  const held = await waitFor("relayer holds the order back (needsRefund)", async () => {
    const h = (await httpJson(RELAYER_HEALTH)).body;
    return h?.needsRefund?.[nonce.toString()] ? h.needsRefund[nonce.toString()] : null;
  }, 60_000);
  info("held back", { nonce: nonce.toString(), reason: held.reason, P: held.protocolFee, floor: held.minProtocolFeeEffective });
  const st = await waitFor("Order Refunded on Solana", async () => {
    const a = await conn.getAccountInfo(orderPda(nonce));
    return a && decodeOrder(a.data).status === ORDER_STATUS_REFUNDED ? true : null;
  }, 120_000);
  const rec = await conn.getAccountInfo(bridgeRefundPda(nonce));
  if (!st || !rec) throw new Error("no refund record");
  const how = await lastIxName(bridgeRefundPda(nonce), BRIDGE_PROGRAM_ID, ["refund_deposit"]);
  if (!how?.startsWith("refund_deposit")) throw new Error(`refund record written by ${how}`);
  const after = tokenAccountAmount((await conn.getAccountInfo(buyerAta))!.data);
  if (after - before !== o.tokens) throw new Error(`buyer got ${after - before} SPL back, want ${o.tokens}`);
  // the relayer's refund tx also settled the buyback share: back to the buyer, record closed
  if (await conn.getAccountInfo(sharePda(nonce))) throw new Error("share record still open after the refund");
  const refunded1 = BigInt(JSON.parse(crank(["sf-status"])).refunded);
  if (refunded1 - refunded0 !== share) throw new Error(`buyback refunded ${refunded1 - refunded0}, want the order's share ${share}`);
  // restore the floor: the refunded deposit must still never be unlocked
  await sendChainTx(admin, KIND.AdminConfig, adminConfig(BigInt(p0.min_protocol_fee)));
  await waitFor("floor restored", async () => (BigInt((await nodeCall("getParams")).min_protocol_fee_effective) === BigInt(p0.min_protocol_fee_effective) ? true : null), 60_000);
  await sleep(5000);
  const dep = await node.getDeposit(nonce);
  if (dep.processed) throw new Error(`refunded deposit ${nonce} was unlocked on the chain`);
  const inv = await invariant();
  if (!/ in_flight_deposits_spl=0 .*delta=0 strict=true/.test(inv)) throw new Error(`invariant not exact after the refund: ${inv}`);
  return `chain min_protocol_fee raised to ${HIGH} by AdminConfig; x402 order ${nonce} (P=${o.body.protocolFee}) held back ` +
    `(${held.reason}), refunded after REFUND_GRACE_MS by ${how}: ${o.tokens} SPL and its ${share}-lamport buyback share back to the Order's buyer ${o.buyer.toBase58()} ` +
    `(storefront wallet), Order status Refunded; floor restored, deposit still not unlocked; invariant exact`;
}

/**
 * Agent Credit R1 + R2 + R5 (2026-10-17): the miner's payout key puts a payout lien
 * (SetRoute, 50%) on its income to a lender's Solana key; the chain itself splits each
 * matured coinbase into withdrawal entries from the miner to the lender, which
 * header-relay --settle releases on Solana by light-client proof (bridge proof mode
 * since step 7). The lender's release key then clears it (ClearRoute). R1: a block's
 * coinbase receipt is proven against its header's tx_root; R5: the light client's
 * final-header accumulator proves an old final header on chain (prove_header).
 */
async function stepLien(): Promise<string> {
  const lender = demoKey("lender").kp.publicKey;
  const release = demoKey("lien-release").kp;
  const tip0 = (await node.getChainInfo()).height;
  const BPS = 5000;
  const body = Buffer.concat([lender.toBuffer(), u16le(BPS), u64le(BigInt(tip0 + 10_000)), u64le(10n ** 15n), release.publicKey.toBuffer()]);
  const setHash = await sendChainTx(miner, 9, body);
  const setHeight = (await node.getTx(setHash))!.height!;
  const route0 = await nodeCall("getRoute", { account: miner.publicKey.toBase58() });
  if (!route0?.active || route0.route_bps !== BPS) throw new Error(`route not active: ${JSON.stringify(route0)}`);
  // income for the miner: a user tx paying a 2-token fee; its block's coinbase matures K
  // blocks later and the chain splits it (50% = 1e9 base units = 1,000,000 SPL units out)
  await sendChainTx(user.kp, KIND.Transfer, transferBody(user.kp.publicKey, 1n), 2_000_000_000n);
  const lenderBefore = await splBalance(lender);
  const routed = await waitFor("lien entries in the withdrawal log", async () => {
    const r = await nodeCall("getRoute", { account: miner.publicKey.toBase58() });
    return BigInt(r.withdrawn) >= 1_000_000_000n ? r : null;
  }, 180_000);
  const ev = await nodeCall("getBridgeEvents", { from_nonce: 0, limit: 1000 });
  const entries = (ev.withdrawals as any[]).filter((w) => w.solana_recipient === lender.toBase58());
  if (!entries.length || entries.some((w) => w.from !== miner.publicKey.toBase58())) throw new Error(`lien entries ${JSON.stringify(entries)}`);
  const first = entries[0];
  const got = await waitFor("lien income released to the lender on Solana", async () => {
    const b = await splBalance(lender);
    return b >= lenderBefore + BigInt(first.amount_spl) ? b : null;
  }, 600_000);
  const processed = PublicKey.findProgramAddressSync([Buffer.from("processed"), Buffer.from([1]), u64le(BigInt(first.nonce))], BRIDGE_PROGRAM_ID)[0];
  const how = await lastIxName(processed, BRIDGE_PROGRAM_ID, ["release_proof", "release"]);
  if (!how?.startsWith("release_proof")) throw new Error(`lien withdrawal ${first.nonce} released by ${how}`);
  // R1: a block the miner mined after SetRoute: receipt fields, tx_root path, reward split
  let proved = "";
  for (let h = setHeight; h < setHeight + 20 && !proved; h++) {
    const b = await nodeCall("getBlock", { height: h });
    const hd = await nodeCall("getHeader", { height: h });
    if (!b?.coinbase || b.coinbase.miner !== miner.publicKey.toBase58()) continue;
    const rec = Buffer.from(b.coinbase.record, "hex");
    if (rec.length !== 97 || rec[0] !== 0 || rec.readBigUInt64LE(1) !== BigInt(h)) throw new Error(`receipt at ${h}`);
    let acc = Buffer.from(blake3(Buffer.concat([Buffer.from([0]), Buffer.from(blake3(rec))])));
    const path = Buffer.from(b.coinbase.path, "hex");
    for (let o = 0; o < path.length; o += 33) {
      const sib = path.subarray(o, o + 32);
      acc = Buffer.from(blake3(Buffer.concat([Buffer.from([1]), ...(path[o + 32] === 1 ? [sib, acc] : [acc, sib])])));
    }
    if (acc.toString("hex") !== hd.header_fields.tx_root) throw new Error(`receipt path at ${h} does not reach tx_root`);
    proved = `block ${h}: fees ${b.coinbase.fees} subsidy ${b.coinbase.subsidy} matures at ${b.coinbase.mature_height}`;
  }
  if (!proved) throw new Error("no block by the miner after SetRoute");
  // clear by the release key (raw ed25519 over route-clear:v1 || chain_id || account || route_nonce)
  const ci = await node.getChainInfo();
  const msg = Buffer.concat([Buffer.from("pearl-sol:route-clear:v1"), u32le(ci.chainId), miner.publicKey.toBuffer(), u64le(BigInt(route0.route_nonce))]);
  const { ed25519 } = await import("@noble/curves/ed25519");
  const sig = Buffer.from(ed25519.sign(msg, release.secretKey.subarray(0, 32)));
  await sendChainTx(miner, 10, Buffer.concat([miner.publicKey.toBuffer(), u64le(BigInt(route0.route_nonce)), sig]));
  const rEnd = await nodeCall("getRoute", { account: miner.publicKey.toBase58() });
  if (rEnd.active || !rEnd.cleared) throw new Error(`route not cleared: ${JSON.stringify(rEnd)}`);
  // R5: accumulator on the light client; an early final header proven on chain
  const st = JSON.parse(run(BIN.headerRelay, [...HR_ARGS, "status"]).trim().split("\n").pop()!);
  const acc = st.accumulator;
  if (!acc || acc.count < 10) throw new Error(`accumulator ${JSON.stringify(acc)}`);
  const accPda = PublicKey.findProgramAddressSync([Buffer.from("acc")], LIGHTCLIENT_ID)[0];
  // acc-proof gives the short form (siblings to the current peaks), valid only while the
  // accumulator's count is unchanged; the parallel relay can accumulate a header between the
  // proof and the tx (AccProof 0x177a), so take a fresh proof and retry a few times
  let ap: any = null;
  let sim = "";
  for (let attempt = 0; ; attempt++) {
    ap = JSON.parse(run(BIN.headerRelay, [...HR_ARGS, "acc-proof", String(acc.start_height + 1)]).trim().split("\n").pop()!);
    const proof = Buffer.from(ap.proof, "hex");
    const data = Buffer.concat([disc("prove_header"), Buffer.from(ap.header, "hex"), u64le(BigInt(ap.count)), u32le(proof.length), proof]);
    try {
      sim = await send([new TransactionInstruction({ programId: LIGHTCLIENT_ID, keys: [m(accPda)], data })], [payer]);
      break;
    } catch (e) {
      if (attempt >= 5 || !/"Custom":6026/.test(String(e))) throw e;
    }
  }
  return `miner key SetRoute ${BPS} bps to lender ${lender.toBase58()} at height ${setHeight}; ${entries.length} lien withdrawal entries ` +
    `(routed ${routed.routed}, withdrawn ${routed.withdrawn} base units) from the miner; lender got ${got - lenderBefore} SPL on Solana by ${how}; ` +
    `ClearRoute by the release key (route_nonce ${route0.route_nonce}) ended it; R1 receipt ${proved} proven to tx_root; ` +
    `R5 accumulator ${acc.count} final headers from ${acc.start_height}, prove_header of height ${ap.height} on chain (${sim.slice(0, 16)}...)`;
}

/**
 * Cadre inference (SYNTHESIS item 3, SPEC 4.5 / 5.4): the quantized Llama-3.2-1B is registered
 * (RegisterModel + its blobs), the buyer pays for a completion over x402 at the inference
 * gateway, the int7 worker answers with tokens, and the request's 96 weight GEMMs become
 * standing jobs (PostSegments) that the miner mines from its model store: blocks tied to that
 * payment. The ticket completes (attester recomputes every segment's C = A B), W goes to the
 * server, and the Order is finalized on Solana by the light client.
 */
async function stepInference(): Promise<string> {
  if (!existsSync(resolve(CADRE_MODEL_DIR, "manifest.json"))) throw new Error(`no quantized model at ${CADRE_MODEL_DIR} (python tools/cadre/quantize.py --model HF_DIR --out ${CADRE_MODEL_DIR})`);
  // a hardlinked copy: registration writes its own manifest.json (new inode), the weights are shared
  const modelDir = resolve(DEMO, "model");
  run("cp", ["-al", CADRE_MODEL_DIR, modelDir]);
  const registrant = payer; // localnet payer: a stable model id across demo runs
  const gwKey = demoKey("gateway");
  // fees: the registrant, the gateway (server) key
  await sendChainTx(user.kp, KIND.Transfer, transferBody(registrant.publicKey, 1_000_000_000n));
  await sendChainTx(user.kp, KIND.Transfer, transferBody(gwKey.kp.publicKey, 1_000_000_000n));
  const pearl = (args: string[], key: string, timeoutMs = 300_000) => lastJson(run(BIN.pearl, ["--network", "localnet", "--rpc", NODE_RPC, "--key", key, "--json", "--wait", ...args], { timeoutMs }));
  const reg = pearl(["model", "register", modelDir, "--store", MODEL_STORE], resolve(ROOT, KEYS.payer), 3_600_000);
  info("model registered", { model_id: reg.model_id, matrices: reg.matrices, macs_per_row: reg.macs_per_row, chunks: reg.chunks, bytes: reg.bytes, upload_s: reg.upload_s, height: reg.included_height });
  const model = await nodeCall("getModel", { model_id: reg.model_id });
  if (!model || model.matrix_count !== reg.matrices) throw new Error(`getModel ${JSON.stringify(model)?.slice(0, 300)}`);
  // worker + gateway
  start("cadre-worker", CADRE_PYTHON, [resolve(ROOT, "tools/cadre/worker.py"), "--model", modelDir, "--port", "10665"], { env: { HF_HUB_OFFLINE: "1", TRANSFORMERS_OFFLINE: "1" } });
  await waitFor("worker /health", async () => (await httpJson(`${WORKER_URL}/health`).catch(() => ({ body: null }))).body?.ok === true, 180_000);
  start("cadre-gateway", BIN.gateway, ["--key", gwKey.file, "--model-id", reg.model_id, "--rpc", NODE_RPC, "--storefront", STOREFRONT_URL, "--worker", WORKER_URL,
    "--attester", ATTESTER_URL, "--work-dir", resolve(DEMO, "gateway"), "--deadline-blocks", "400", "--round-secs", "5", "--balance-deadline-blocks", "2000"]);
  await waitFor("gateway /health", async () => (await httpJson(`${GATEWAY_URL}/health`).catch(() => ({ body: null }))).body?.ok === true, 60_000);
  // prepaid balance (owner decision 2026-10-18): one x402 top-up, then wallet-signed requests
  const rowsBudget = Number(process.env.CADRE_BALANCE_ROWS ?? 256);
  const topBody = JSON.stringify({ rows: rowsBudget });
  const r402 = await httpJson(`${GATEWAY_URL}/v1/balance/topup`, { method: "POST", headers: { "content-type": "application/json" }, body: topBody });
  if (r402.status !== 402) throw new Error(`expected 402, got ${r402.status} ${JSON.stringify(r402.body)}`);
  const req = r402.body.accepts[0];
  info("top-up quote", { price_lamports: req.maxAmountRequired, rows: rowsBudget });
  const { blockhash } = await conn.getLatestBlockhash();
  const msg = new TransactionMessage({ payerKey: new PublicKey(req.extra.feePayer), recentBlockhash: blockhash, instructions: [
    ComputeBudgetProgram.setComputeUnitLimit({ units: 10_000 }),
    SystemProgram.transfer({ fromPubkey: buyer.kp.publicKey, toPubkey: new PublicKey(req.payTo), lamports: BigInt(req.maxAmountRequired) }),
  ] }).compileToV0Message();
  const vtx = new VersionedTransaction(msg);
  vtx.sign([buyer.kp]);
  const xPayment = Buffer.from(JSON.stringify({ x402Version: 1, scheme: "exact", network: req.network,
    payload: { transaction: Buffer.from(vtx.serialize()).toString("base64") } })).toString("base64");
  const tPay = Date.now();
  const top = await httpJson(`${GATEWAY_URL}/v1/balance/topup`, { method: "POST", headers: { "content-type": "application/json", "x-payment": xPayment }, body: topBody });
  if (top.status !== 201) throw new Error(`top-up: ${top.status} ${JSON.stringify(top.body).slice(0, 800)}`);
  const ticket: string = top.body.ticket_id;
  const nonce = BigInt(top.body.order);
  info("balance", { ticket, order: top.body.order, rows: top.body.rows, wallet: top.body.wallet });
  // wallet-signed OpenAI requests (cadre-auth-v1)
  const { ed25519 } = await import("@noble/curves/ed25519");
  const signedPost = async (path: string, b: string) => {
    const ts = Date.now();
    const m = `cadre-auth-v1\nPOST\n${path}\n${ts}\n${createHash("sha256").update(b).digest("hex")}`;
    const sig = Buffer.from(ed25519.sign(Buffer.from(m), buyer.kp.secretKey.subarray(0, 32))).toString("hex");
    const res = await fetch(`${GATEWAY_URL}${path}`, { method: "POST", body: b, headers: { "content-type": "application/json",
      "x-cadre-wallet": buyer.kp.publicKey.toBase58(), "x-cadre-timestamp": String(ts), "x-cadre-signature": sig } });
    return res;
  };
  const prompt = process.env.CADRE_PROMPT ?? "What is the capital of France? Answer in one word.";
  const maxTokens = Number(process.env.CADRE_MAX_TOKENS ?? 12);
  const chatBody = JSON.stringify({ model: model.name, messages: [{ role: "user", content: prompt }], max_tokens: maxTokens });
  const t1 = Date.now();
  const r1 = await signedPost("/v1/chat/completions", chatBody);
  const j1: any = await r1.json();
  if (r1.status !== 200) throw new Error(`chat completion: ${r1.status} ${JSON.stringify(j1).slice(0, 800)}`);
  const answerS = (Date.now() - t1) / 1000;
  if (j1.cadre?.ticket_id !== ticket || r1.headers.get("x-cadre-ticket") !== ticket || !r1.headers.get("x-cadre-explorer")) throw new Error(`receipt ${JSON.stringify(j1.cadre)}`);
  info("chat completion", { text: j1.choices[0].message.content, usage: j1.usage, receipt: j1.cadre, wall_s: answerS });
  // streaming text completion: SSE until [DONE]
  const t2 = Date.now();
  const r2 = await signedPost("/v1/completions", JSON.stringify({ model: model.name, prompt: "The capital of France is", max_tokens: maxTokens, stream: true }));
  const sse = await r2.text();
  const events = sse.split("\n\n").filter((e) => e.startsWith("data: ")).map((e) => e.slice(6));
  if (events[events.length - 1] !== "[DONE]") throw new Error(`stream did not end in [DONE]: ${sse.slice(-300)}`);
  const lastEv = JSON.parse(events[events.length - 2]);
  const streamed = events.slice(0, -1).map((e) => JSON.parse(e).choices?.[0]?.text ?? "").join("");
  const firstChunkMs = Number(lastEv.cadre?.measured?.ttft_s ?? 0) * 1000;
  info("streamed completion", { text: streamed, chunks: events.length - 1, usage: lastEv.usage, ttft_s: lastEv.cadre?.measured?.ttft_s, decode_tokens_per_s: lastEv.cadre?.measured?.decode_tokens_per_s, wall_s: (Date.now() - t2) / 1000 });
  void firstChunkMs;
  const models = (await httpJson(`${GATEWAY_URL}/v1/models`)).body;
  const stats = (await httpJson(`${GATEWAY_URL}/v1/stats`)).body;
  info("models / stats", { price: models.data[0].cadre.pricing, stats });
  // close the balance: the gateway posts the round, delivers, gets it attested, completes
  const rc = await signedPost("/v1/balance/close", "{}");
  if (rc.status !== 202) throw new Error(`close: ${rc.status}`);
  const r = { body: { text: j1.choices[0].message.content, output_ids: { length: j1.usage.completion_tokens }, rows: j1.cadre.rows } };
  const done0 = await waitFor("balance ticket completed", async () => {
    const t = await nodeCall("getTicket", { ticket_id: ticket }).catch(() => null);
    return t && t.status === "completed" ? t : null;
  }, 3_600_000, 5000);
  const respBlob = await nodeCall("getBlob", { uri: done0.result_uri });
  if (!respBlob) throw new Error(`response blob ${done0.result_uri} not on the node`);
  const tk = await waitFor("every segment of every round mined", async () => {
    const t = await nodeCall("getTicket", { ticket_id: ticket }).catch(() => null);
    if (!t) return null;
    const segs = t.segment_list as any[];
    return t.status === "completed" && segs.length === model.matrix_count * t.standing.rounds && segs.every((g) => g.blocks_mined >= 1) ? t : null;
  }, 3_600_000, 5000);
  const settleS = (Date.now() - tPay) / 1000;
  const p = await nodeCall("getParams");
  const capTotal = BigInt(tk.standing.cap_total);
  if (capTotal !== (BigInt(tk.protocol_fee) * BigInt(p.alpha_ppm)) / 1_000_000n) throw new Error(`cap_total ${capTotal} vs alpha x P`);
  // every block mined on the ticket's segments: tied to this payment
  const segIds = new Set((tk.segment_list as any[]).map((g) => g.segment_id));
  const ci = await node.getChainInfo();
  let blocks = 0, sub = 0n;
  const heights: number[] = [];
  const posted = tk.posted_height as number;
  for (let h = posted; h <= ci.height; h++) {
    const b = await nodeCall("getBlock", { height: h });
    if (segIds.has(b.header_fields.job_id)) {
      blocks++;
      sub += BigInt(b.subsidy);
      heights.push(h);
    }
  }
  if (blocks !== tk.standing.blocks_mined || sub !== BigInt(tk.standing.subsidy_paid)) throw new Error(`blocks ${blocks}/${tk.standing.blocks_mined} subsidy ${sub}/${tk.standing.subsidy_paid}`);
  if (sub === 0n || sub > capTotal) throw new Error(`ticket subsidy ${sub} vs alpha x P ${capTotal}`);
  if (tk.deliverer !== gwKey.kp.publicKey.toBase58()) throw new Error(`deliverer ${tk.deliverer}`);
  // the response blob is the ticket's result
  const rj = JSON.parse(Buffer.from(respBlob.data, "base64").toString());
  const receipts = (rj.rounds as any[]).flatMap((x) => x.requests);
  if (rj.ticket_id !== ticket || receipts.length !== 2 || !receipts.some((q: any) => q.request_id === j1.cadre.request_id)) throw new Error(`response blob ${JSON.stringify(rj).slice(0, 300)}`);
  // pay for rows used: W x rows_used / R to the server, the rest of W + P's share back to the buyer
  if (BigInt(tk.standing.rows_used) > BigInt(tk.standing.rows)) throw new Error(`rows_used ${tk.standing.rows_used}`);
  // Solana: the Order is finalized by proof with the response's hash
  const pda_ = orderPda(nonce);
  const acc = await waitFor("ticket Order finalized on Solana", async () => {
    const a = await conn.getAccountInfo(pda_);
    return a && a.data[ORDER_LAYOUT.STATUS] === ORDER_STATUS_FINALIZED ? a : null;
  }, 900_000, 2000);
  const how = await lastIxName(pda_, STOREFRONT_PROGRAM_ID, ["finalize_order_proof", "finalize_order"]);
  if (!how?.startsWith("finalize_order_proof")) throw new Error(`ticket Order finalized by ${how}`);
  const rh = Buffer.from(acc.data.subarray(ORDER_LAYOUT.RESULT_HASH, ORDER_LAYOUT.RESULT_HASH + 32)).toString("hex");
  if (rh !== tk.result_hash) throw new Error(`Order result_hash ${rh} != ticket ${tk.result_hash}`);
  // CLI reads of the same state
  const cliTicket = pearl(["ticket", ticket], resolve(ROOT, KEYS.payer));
  const cliModel = pearl(["model", "show", reg.model_id], resolve(ROOT, KEYS.payer));
  if (cliTicket.standing.blocks_mined !== tk.standing.blocks_mined || cliModel.tickets < 1) throw new Error("pearl ticket / model show disagree with the node");
  const mined = BigInt(model.macs_per_row) * BigInt(tk.standing.rows_used);
  writeFileSync(resolve(DEMO, "inference.json"), JSON.stringify({ model_id: reg.model_id, ticket, order: nonce.toString(), text: r.body.text, output_ids: r.body.output_ids,
    rows: r.body.rows, chat: j1, streamed: { text: streamed, last: lastEv }, models, stats, answer_s: answerS, settle_s: settleS, blocks, heights,
    subsidy: sub.toString(), cap_total: capTotal.toString(), rows_used: tk.standing.rows_used, rows_budget: tk.standing.rows, price_lamports: req.maxAmountRequired, registration: reg }, null, 1));
  return `model ${reg.model_id.slice(0, 12)} registered (${reg.matrices} matrices, ${reg.chunks} blobs in ${Number(reg.upload_s).toFixed(1)} s); balance of ${rowsBudget} rows ` +
    `topped up for ${req.maxAmountRequired} lamports over x402; signed chat completion -> "${r.body.text}" (${r.body.output_ids.length} tokens in ${answerS.toFixed(1)} s), ` +
    `streamed completion "${streamed}"; ticket ${ticket.slice(0, 12)}: ${tk.standing.rounds} round(s), ${segIds.size} standing jobs ` +
    `(${tk.standing.rows_used} of ${tk.standing.rows} rows used, ${mined} mined MACs), ${blocks} blocks at ${heights[0]}..${heights[heights.length - 1]}, subsidy ${sub} <= alpha x P ${capTotal}; ` +
    `completed (W to the gateway), Order ${pda_.toBase58()} finalized by ${how}; settled ${settleS.toFixed(0)} s after payment`;
}

// ------------------------------------------------------------ teardown

function stopPid(name: string, pid: number) {
  try {
    process.kill(pid, 0);
  } catch {
    return;
  }
  try {
    process.kill(pid, "SIGTERM");
  } catch {
    /* gone */
  }
}

async function teardown(): Promise<string> {
  for (const p of [...procs].reverse()) stopPid(p.name, p.pid);
  await sleep(1500);
  for (const p of procs) {
    try {
      process.kill(p.pid, 0);
      process.kill(p.pid, "SIGKILL");
    } catch {
      /* stopped */
    }
  }
  try {
    console.log(run("bash", ["scripts/devnet.sh", "stop"], { env: DEVNET_ENV }).trim());
  } catch (e) {
    console.log(String(e).slice(0, 300));
  }
  try {
    console.log(run("bash", ["scripts/localnet.sh", "down", "token"]).trim());
  } catch (e) {
    console.log(String(e).slice(0, 300));
  }
  const ports = [10600, 10601, 10602, 10603, 10630, 10631, 10632, 10633, 10634, 10635, 10640, 10641, 10642, 10660, 10661, 10662, 10664, 10665];
  const busy = await waitFor("ports free", async () => {
    const b = ports.filter((p) => portBusy(p));
    return b.length === 0 ? [] : null;
  }, 30_000).catch(() => ports.filter((p) => portBusy(p)));
  if (busy.length) throw new Error(`ports still bound: ${busy.join(",")}`);
  return `stopped ${procs.map((p) => `${p.name}(${p.pid})`).join(", ")}, devnet and token validator by PID; ports ${ports[0]}-${ports[ports.length - 1]} free`;
}

// ------------------------------------------------------------ main

let failed = false;
try {
  await step("1. bring up, init reserve + launch, bridge, storefront, genesis signers", step1);
  await step("2. trades, buyback crank, fund_mining -> pool unlock", step2);
  await step("3. plain bridge-in + RegisterAgent", step3);
  await step("4. light client: init from the devnet checkpoint, header-relay, storefront finalize -> proof mode", stepLightClient);
  await step("5. storefront order over x402 -> job on the chain", step4);
  await step("6. job + fallback mining, delivery, CompleteJob, finalize_order_proof on Solana", step5);
  await step("7. storefront buyback: the finalized order's share released (settle_share) and bought back, half to the mining reserve, the rest burned", stepBuyback);
  await step("8. rewards mature, bridge release -> proof mode, BridgeLock -> release_proof on Solana", step6);
  await step("9. underpaid order (chain floor raised) held back, refunded on Solana: refund_deposit + mark_refunded", stepRefund);
  await step("10. payout lien (R2) on the miner's income -> lender on Solana by proof; coinbase receipt (R1); accumulator (R5)", stepLien);
  await step("11. paid inference (Cadre): RegisterModel Llama-3.2-1B int7, prepaid balance over x402, signed OpenAI chat + streamed completions, their weight GEMMs mined as standing jobs, ticket completed (pay for rows used), Order finalized by proof", stepInference);
} catch (e) {
  failed = true;
  console.log(`\n${String((e as Error).message)}`);
}
const keepUp = process.env.DEMO_KEEP_UP === "1";
if (!keepUp) {
  const s = Date.now();
  try {
    const d = await teardown();
    results.push({ step: "12. teardown", ok: true, secs: (Date.now() - s) / 1000, detail: d, invariant: "" });
    console.log(`\n== 12. teardown\n   PASS (${((Date.now() - s) / 1000).toFixed(1)} s): ${d}`);
  } catch (e) {
    failed = true;
    results.push({ step: "12. teardown", ok: false, secs: (Date.now() - s) / 1000, detail: String((e as Error).message), invariant: "" });
    console.log(`\n== 12. teardown\n   FAIL: ${(e as Error).message}`);
  }
} else {
  console.log(`\nDEMO_KEEP_UP=1: leaving everything running (PIDs in ${resolve(DEMO, "pids")}); stop with scripts/demo.sh down`);
}
console.log("\n== summary");
for (const r of results) console.log(`  ${r.ok ? "PASS" : "FAIL"}  ${r.step}  (${r.secs.toFixed(1)} s)`);
console.log(`  total ${((Date.now() - t0) / 1000).toFixed(1)} s`);
writeFileSync(resolve(DEMO, "result.json"), JSON.stringify(results, null, 2));
process.exit(failed || results.some((r) => !r.ok) ? 1 : 0);
