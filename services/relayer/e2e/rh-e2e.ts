// Driver for scripts/rh-e2e.sh: the RH side end to end on a LOCAL anvil (chain id 46630)
// and a LOCAL agent-node, with the real token-evm contracts, the RH relayer
// (RELAYER_SIDE=rh) and the header-oracle poster running as separate processes.
//
//   node e2e/rh-e2e.ts keys <dir>       fresh throwaway keys (chain ed25519 + EVM secp256k1) into <dir>/keys
//   node e2e/rh-e2e.ts activate <dir>   AdminConfig: domain 2 (key 51) + bridge_v2_height (key 50); waits for active
//   node e2e/rh-e2e.ts run <dir>        the steps; prints one line per step and writes <dir>/transcript.json
//
// Env: RH_RPC, NODE_RPC, RELAYER_HEALTH, ORACLE_HEALTH. Nothing here talks to a public network.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Keypair, PublicKey } from "@solana/web3.js";
import { createPublicClient, createWalletClient, http, parseAbi, getAddress } from "viem";
import type { Address, Hex } from "viem";
import { generatePrivateKey, privateKeyToAccount } from "viem/accounts";
import { KIND, bridgeLockV2Body, signTx } from "../src/agent-tx.ts";
import { ed25519Sign, evmRecipient, orderJobIdV2, u16le, u64le } from "../src/codec.ts";
import { loadKeypair } from "../src/config.ts";
import { bridgeAbi, erc20Abi, oracleAbi, storefrontAbi } from "../src/evm/abi.ts";
import { NodeV2 } from "../src/evm/node-v2.ts";

const [cmd, dir] = process.argv.slice(2);
if (!cmd || !dir) {
  console.error("usage: rh-e2e.ts keys|activate|run <dir>");
  process.exit(2);
}
const RH_RPC = process.env.RH_RPC ?? "http://127.0.0.1:10750";
const NODE_RPC = process.env.NODE_RPC ?? "http://127.0.0.1:10752";
const RELAYER_HEALTH = process.env.RELAYER_HEALTH ?? "http://127.0.0.1:10755/health";
const ORACLE_HEALTH = process.env.ORACLE_HEALTH ?? "http://127.0.0.1:10756/health";
const KD = join(dir, "keys");

const CHAIN_KEYS = ["admin", "attester", "agent", "miner", "recipient", "deliverer", "evaluator"] as const;
const EVM_KEYS = ["deployer", "signer0", "signer1", "signer2", "user", "buyer", "dest"] as const;

function keys(): void {
  mkdirSync(KD, { recursive: true });
  const index: Record<string, string> = {};
  for (const n of CHAIN_KEYS) {
    const kp = Keypair.generate();
    writeFileSync(join(KD, `${n}-keypair.json`), JSON.stringify([...kp.secretKey]), { mode: 0o600 });
    index[n] = kp.publicKey.toBase58();
  }
  for (const n of EVM_KEYS) {
    const pk = generatePrivateKey();
    writeFileSync(join(KD, `${n}.key`), `${pk}\n`, { mode: 0o600 });
    index[n] = privateKeyToAccount(pk).address;
  }
  writeFileSync(join(KD, "index.json"), JSON.stringify(index, null, 2) + "\n");
  console.log(JSON.stringify(index));
}

const ckey = (n: (typeof CHAIN_KEYS)[number]) => loadKeypair(join(KD, `${n}-keypair.json`));
const ekey = (n: (typeof EVM_KEYS)[number]) => privateKeyToAccount(readFileSync(join(KD, `${n}.key`), "utf8").trim() as Hex);
const node = NodeV2.http(NODE_RPC);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const nodeCall = (m: string, p: Record<string, unknown> = {}) => node.call(m, p) as Promise<any>; // eslint-disable-line @typescript-eslint/no-explicit-any

async function waitFor<T>(what: string, f: () => Promise<T | null | undefined | false>, ms = 180_000, every = 1000): Promise<T> {
  const t0 = Date.now();
  let lastErr = "";
  for (;;) {
    try {
      const v = await f();
      if (v) return v as T;
    } catch (e) {
      if ((e as { fatal?: boolean }).fatal) throw e;
      lastErr = String((e as Error).message ?? e).slice(0, 300);
    }
    if (Date.now() - t0 > ms) throw new Error(`timed out after ${ms} ms waiting for ${what}${lastErr ? ` (last error: ${lastErr})` : ""}`);
    await sleep(every);
  }
}

async function sendChainTx(kp: Keypair, kind: number, body: Buffer): Promise<string> {
  const ci = await node.getChainInfo();
  const acct = await node.getAccount(kp.publicKey.toBase58());
  const tx = signTx(kp, { chainId: ci.chainId, from: kp.publicKey, nonce: acct.nonce, fee: 0n, kind }, body);
  const h = await node.sendTx(tx);
  await waitFor(`chain tx ${h.slice(0, 12)} included`, async () => {
    const st = await nodeCall("getTxStatus", { tx_hash: h });
    if (st?.status === "dropped") throw Object.assign(new Error(`chain tx ${h.slice(0, 12)} dropped: ${st.reason}`), { fatal: true });
    return st?.status === "included" || st?.status === "final";
  }, 120_000, 500);
  return h;
}

const adminConfig = (key: number, value: Buffer) => Buffer.concat([Buffer.from([key]), u16le(value.length), value]);

async function activate(): Promise<void> {
  const dep = JSON.parse(readFileSync(join(dir, "deploy.json"), "utf8"));
  const admin = ckey("admin");
  const p0 = await nodeCall("getParams");
  const kDomain = Number(p0.keys.bridge_domain), kHeight = Number(p0.keys.bridge_v2_height);
  const bridge = Buffer.from(String(dep.bridge).slice(2), "hex");
  const value = Buffer.concat([Buffer.from([2]), u64le(46630n), Buffer.from([20]), bridge, Buffer.from([0])]);
  await sendChainTx(admin, KIND.AdminConfig, adminConfig(kDomain, value));
  // The activation height must be above the AdminConfig's own block. The local miner runs
  // far faster than real time at first, so the margin is sized from its measured rate and
  // the tx is re-sent with a larger one when it lands too late.
  for (let attempt = 0; ; attempt++) {
    const a = await node.getChainInfo();
    await sleep(2000);
    const b = await node.getChainInfo();
    const rate = Math.max(1, (b.height - a.height) / 2);
    const height = b.height + Math.ceil(rate * 15 * (attempt + 1)) + 20;
    const acct = await node.getAccount(admin.publicKey.toBase58());
    const tx = signTx(admin, { chainId: b.chainId, from: admin.publicKey, nonce: acct.nonce, fee: 0n, kind: KIND.AdminConfig },
      adminConfig(kHeight, u64le(BigInt(height))));
    await node.sendTx(tx);
    const set = await waitFor("bridge_v2_height set", async () => (await nodeCall("getParams")).bridge_v2_height ?? null, 20_000, 300).catch(() => null);
    if (set !== null) break;
    if (attempt >= 4) throw new Error("bridge_v2_height never set");
  }
  const p = await waitFor("bridge_v2_active", async () => {
    const q = await nodeCall("getParams");
    return q.bridge_v2_active ? q : null;
  }, 120_000);
  console.log(JSON.stringify({ msg: "bridge v2 active", bridge_v2_height: p.bridge_v2_height, domains: p.bridge_domains }));
}

// ------------------------------------------------------------------ run

const jobSpecAbi = parseAbi([
  "struct JobSpec { uint32 m; uint32 n; uint32 k; bytes config; bytes32 hashA; bytes32 hashB; bytes dataUri; uint64 deadlineBlocks; uint32 maxBlocks; bytes32[] evaluators; uint8 threshold; }",
  "function buyJob(bytes32 recipient, JobSpec spec, uint256 minTokensOut) payable returns (uint64)",
  "function setReleaseModes(uint8 modes, address headers)",
  "function deposit(uint64 amount, bytes32 cadreRecipient) returns (uint64)",
  "event Released(uint64 indexed nonce, address indexed recipient, uint64 amount, bool viaProof)",
  "event FinalHeaderPosted(bytes32 indexed blockHash, uint64 indexed height, bytes32 txRoot, bytes32 stateRoot, bytes32 withdrawRoot)",
]);

interface Step { step: string; ok: boolean; seconds: number; detail: string }

async function run(): Promise<void> {
  const dep = JSON.parse(readFileSync(join(dir, "deploy.json"), "utf8"));
  const T = getAddress(dep.token), B = getAddress(dep.bridge), S = getAddress(dep.storefront), O = getAddress(dep.headerOracle);
  const transport = http(RH_RPC);
  const pub = createPublicClient({ transport });
  const wallet = (a: ReturnType<typeof ekey>) => createWalletClient({ transport, account: a });
  const deployer = ekey("deployer"), user = ekey("user"), buyer = ekey("buyer"), dest = ekey("dest");
  const recipient = ckey("recipient"), deliverer = ckey("deliverer"), evaluator = ckey("evaluator"), admin = ckey("admin");
  const steps: Step[] = [];
  const write = async (a: ReturnType<typeof ekey>, address: Address, abi: any, functionName: string, args: unknown[], value?: bigint) => { // eslint-disable-line @typescript-eslint/no-explicit-any
    const { request, result } = await pub.simulateContract({ account: a, address, abi, functionName, args, value } as never) as { request: unknown; result: unknown };
    const hash = await wallet(a).writeContract(request as never);
    const rc = await pub.waitForTransactionReceipt({ hash });
    if (rc.status !== "success") throw new Error(`${functionName} reverted (${hash})`);
    return { hash, result, block: rc.blockNumber };
  };
  const bal = (a: Address) => pub.readContract({ address: T, abi: erc20Abi, functionName: "balanceOf", args: [a] });
  const health = async (url: string) => (await (await fetch(url)).json()) as any; // eslint-disable-line @typescript-eslint/no-explicit-any
  const step = async (name: string, f: () => Promise<string>) => {
    const t0 = Date.now();
    try {
      const detail = await f();
      const s = { step: name, ok: true, seconds: (Date.now() - t0) / 1000, detail };
      steps.push(s);
      console.log(`PASS ${name} (${s.seconds.toFixed(1)} s): ${detail}`);
    } catch (e) {
      const s = { step: name, ok: false, seconds: (Date.now() - t0) / 1000, detail: String((e as Error).message ?? e) };
      steps.push(s);
      console.log(`FAIL ${name} (${s.seconds.toFixed(1)} s): ${s.detail}`);
      throw e;
    }
  };
  const releasedEvent = async (nonce: bigint) => {
    const logs = await pub.getContractEvents({ address: B, abi: jobSpecAbi, eventName: "Released", args: { nonce }, fromBlock: 0n });
    return logs[0] ? (logs[0].args as { viaProof: boolean; amount: bigint; recipient: Address }) : null;
  };
  const p0 = await nodeCall("getParams");

  try {
    await step("1 deposit on RH -> BridgeUnlockV2 on the chain", async () => {
      const amount = 5_000n * 1_000_000n; // 5,000 CADRE
      await write(deployer, T, erc20Abi, "transfer", [user.address, 1_000_000n * 1_000_000n]);
      await write(user, T, erc20Abi, "approve", [B, amount]);
      const r = await write(user, B, jobSpecAbi, "deposit", [amount, `0x${recipient.publicKey.toBuffer().toString("hex")}`]);
      const nonce = r.result as bigint;
      const d = await waitFor("deposit unlocked on the chain", async () => {
        const st = await node.depositV2(2, nonce);
        return st.processed ? st : null;
      }, 240_000);
      const acct = await node.getAccount(recipient.publicKey.toBase58());
      if (acct.balance !== amount * 1000n) throw new Error(`recipient balance ${acct.balance}, want ${amount * 1000n}`);
      return `RH nonce ${nonce} (block ${r.block}, waited for the safe tag) unlocked at chain height ${d.height}: recipient ${recipient.publicKey.toBase58().slice(0, 8)}... holds ${acct.balance} chain units`;
    });

    await step("2 BridgeLockV2 on the chain -> release on RH (EIP-712 signer mode)", async () => {
      const amount = 1_000n * 1_000_000n;
      const before = await bal(dest.address);
      await sendChainTx(recipient, KIND.BridgeLockV2, bridgeLockV2Body(2, evmRecipient(dest.address), amount * 1000n));
      const ev = await waitFor("Released(0)", () => releasedEvent(0n), 240_000);
      if (ev.viaProof) throw new Error("released via proof, want signers");
      const after = await bal(dest.address);
      if (after - before !== amount) throw new Error(`dest got ${after - before}, want ${amount}`);
      return `withdrawal 0 released to ${dest.address} for ${amount} (viaProof false) after K-deep finality`;
    });

    await step("3 BridgeLockV2 -> header oracle post -> releaseProof on RH (MMR proof against the posted withdraw_root)", async () => {
      await write(deployer, B, jobSpecAbi, "setReleaseModes", [2, O]);
      const amount = 500n * 1_000_000n;
      const before = await bal(dest.address);
      await sendChainTx(recipient, KIND.BridgeLockV2, bridgeLockV2Body(2, evmRecipient(dest.address), amount * 1000n));
      const ev = await waitFor("Released(1)", () => releasedEvent(1n), 300_000);
      if (!ev.viaProof) throw new Error("released by signers, want proof");
      const after = await bal(dest.address);
      if (after - before !== amount) throw new Error(`dest got ${after - before}, want ${amount}`);
      const [h, hash] = await pub.readContract({ address: O, abi: oracleAbi, functionName: "latestFinal" });
      const oh = await health(ORACLE_HEALTH);
      await write(deployer, B, jobSpecAbi, "setReleaseModes", [3, O]);
      return `withdrawal 1 released via proof against oracle header ${h} (${hash.slice(0, 18)}...); poster made ${oh.posts} post(s); release modes back to 3`;
    });

    const spec = () => ({
      m: Number(p0.fallback_m), n: Number(p0.fallback_n), k: Number(p0.fallback_config ? Buffer.from(p0.fallback_config, "hex").readUInt32LE(0) : 0),
      config: `0x${p0.fallback_config}` as Hex, hashA: `0x${"11".repeat(32)}` as Hex, hashB: `0x${"22".repeat(32)}` as Hex,
      dataUri: `0x${Buffer.from("https://example.invalid/rh-e2e").toString("hex")}` as Hex, deadlineBlocks: 500n, maxBlocks: 4,
      evaluators: [`0x${evaluator.publicKey.toBuffer().toString("hex")}`] as Hex[], threshold: 1,
    });
    const buy = async () => {
      const r = await write(buyer, S, jobSpecAbi, "buyJob", [`0x${recipient.publicKey.toBuffer().toString("hex")}`, spec(), 1n], 10n ** 14n);
      return r.result as bigint;
    };
    const order = (n: bigint) => pub.readContract({ address: S, abi: storefrontAbi, functionName: "getOrder", args: [n] });

    await step("4 storefront order (buyJob, ETH -> v4 desk) -> order job on the chain -> SubmitResult + CompleteJob -> finalizeOrder on RH", async () => {
      const n = await buy();
      const jobId = orderJobIdV2(2, n).toString("hex");
      await waitFor("order job open on the chain", async () => (await node.getJob(jobId))?.status === "open", 240_000);
      const rh = Buffer.alloc(32, 0x5e);
      const uri = Buffer.from("https://example.invalid/rh-e2e-result");
      await sendChainTx(deliverer, KIND.SubmitResult, Buffer.concat([Buffer.from(jobId, "hex"), rh, u16le(uri.length), uri]));
      const msg = Buffer.concat([Buffer.from("pearl-sol:result:v1"), Buffer.from(jobId, "hex"), deliverer.publicKey.toBuffer(), rh]);
      await sendChainTx(evaluator, KIND.CompleteJob, Buffer.concat([Buffer.from(jobId, "hex"), Buffer.from([1]), evaluator.publicKey.toBuffer(), ed25519Sign(evaluator, msg)]));
      const job = await node.getJob(jobId);
      if (job?.status !== "completed") throw new Error(`job ${job?.status}`);
      const o = await waitFor("Order Finalized on RH", async () => ((await order(n)).status === 2 ? order(n) : null), 240_000);
      if (o.deliverer !== `0x${deliverer.publicKey.toBuffer().toString("hex")}` || o.resultHash !== `0x${rh.toString("hex")}`) throw new Error("finalized with other result");
      await waitFor("buyback share settled", async () => (await order(n)).shareSettled, 120_000);
      return `order ${n}: ${o.tokens} tokens (W ${o.workerPay}, P ${o.protocolFee}), job ${jobId.slice(0, 12)}... completed at ${job.closedHeight}, finalized on RH with the deliverer and result hash, share ${o.share} wei settled to the buyback`;
    });

    await step("5 refund: order under the chain's fee floor is held back, refundDeposit + markRefunded + settleShare, never unlocked", async () => {
      const key = Number(p0.keys.min_protocol_fee);
      const HIGH = 10n ** 16n;
      await sendChainTx(admin, KIND.AdminConfig, adminConfig(key, u64le(HIGH)));
      await waitFor("floor raised", async () => BigInt((await nodeCall("getParams")).min_protocol_fee_effective) === HIGH, 60_000);
      const tokBefore = await bal(buyer.address);
      const n = await buy();
      const o0 = await order(n);
      const held = await waitFor("relayer holds it back", async () => (await health(RELAYER_HEALTH)).needsRefund?.[n.toString()], 240_000);
      const o = await waitFor("Order Refunded on RH", async () => ((await order(n)).status === 4 ? order(n) : null), 120_000);
      await waitFor("share settled back to the buyer", async () => (await order(n)).shareSettled, 120_000);
      const got = (await bal(buyer.address)) - tokBefore;
      if (got !== o0.tokens) throw new Error(`buyer got ${got} tokens back, want ${o0.tokens}`);
      await sendChainTx(admin, KIND.AdminConfig, adminConfig(key, u64le(BigInt(p0.min_protocol_fee))));
      await waitFor("floor restored", async () => (await nodeCall("getParams")).min_protocol_fee_effective === p0.min_protocol_fee_effective, 60_000);
      await sleep(8000);
      const d = await node.depositV2(2, n);
      if (d.processed) throw new Error(`refunded deposit ${n} was unlocked on the chain`);
      return `order ${n} (P ${o.protocolFee} < floor ${HIGH}) held (${held.reason}), refunded ${o0.tokens} tokens to the buyer, status Refunded, share ${o.share} wei back; still not unlocked after the floor was restored`;
    });

    await step("6 per-side invariant: vault x 1000 == outstanding_2 + pending (exact)", async () => {
      const h = await waitFor("strict invariant", async () => {
        const x = await health(RELAYER_HEALTH);
        return x.last?.invariant?.strictFormHolds && x.last.invariant.delta === "0" ? x : null;
      }, 180_000, 2000);
      const c = await node.bridgeCounters(2);
      const vault = await bal(B);
      if (vault * 1000n !== c.counters!.outstanding) throw new Error(`vault ${vault} x 1000 != outstanding ${c.counters!.outstanding}`);
      return `vault ${vault} x 1000 == outstanding ${c.counters!.outstanding} (unlocked ${c.counters!.unlocked}, locked ${c.counters!.locked}); relayer health ok=${h.ok}, delta ${h.last.invariant.delta}`;
    });
  } finally {
    writeFileSync(join(dir, "transcript.json"), JSON.stringify({ at: new Date().toISOString(), steps }, null, 2) + "\n");
  }
  const failed = steps.filter((s) => !s.ok).length;
  console.log(`${steps.length - failed}/${steps.length} PASS, ${steps.reduce((a, s) => a + s.seconds, 0).toFixed(1)} s`);
  if (failed) process.exit(1);
}

if (cmd === "keys") keys();
else if (cmd === "activate") await activate();
else if (cmd === "run") await run().catch((e) => {
  console.error(String((e as Error).message ?? e));
  process.exit(1);
});
else {
  console.error(`unknown command ${cmd}`);
  process.exit(2);
}
void PublicKey;
