// Chain operations for the selected account: balances, sends, bridge in/out with
// pending tracking, agents, jobs, storefront orders, mining rewards.

import { b58, b64, fromB64, fromHex, hex, isAddress, key32 } from "../core/bytes";
import { pubkeyOf } from "../core/crypto";
import { commitJob, commitOrder, jobHeaderProblem } from "../chain/job";
import { orderJobId } from "../chain/messages";
import { ChainRpc, Job } from "../chain/rpc";
import { PostJobBody, TxBody, UnsignedTx, signTx } from "../chain/tx";
import { ContractContext, ContractFeeParams, contractFeeFor, contractFeeParams, isContractKind } from "../chain/contracts";
import {
  BridgeConfig, PumpAccounts, StoreJobSpec, ammGlobalBuyback, bondingCurvePda, bridgeConfigPda, bridgeDepositIx, buyJobAmmIx, buyJobIx, canonicalPool,
  computeUnitLimit, cpQuote, createAtaIdempotent, decodeBridgeConfig, decodeCurve, decodeOrder, decodePool, decodeStoreConfig, mintDecimals, openOrderIx,
  orderPda, processedPda, pumpGlobalBuyback, storeConfigPda, systemTransfer, tokenAccountAmount, transferChecked, PUMP, AMM, cancelDraftIx,
} from "../solana/programs";
import { SolanaRpc } from "../solana/rpc";
import { Instruction, ata, buildSigned, pda, pk } from "../solana/tx";
import { SPL_DECIMALS, SPL_TO_NATIVE } from "../shared/constants";
import { Network } from "../shared/networks";
import type { ActivityItem, Balances, PendingTransfer } from "../shared/types";
import { seedFor } from "./keyring";
import { activeNetwork, getLocal, setLocal } from "./store";
import { utf8 } from "../core/bytes";

const rid = () => hex(crypto.getRandomValues(new Uint8Array(8)));
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

export async function ctx() {
  const net = await activeNetwork();
  return { net, sol: new SolanaRpc(net.solanaRpc), chain: new ChainRpc(net.chainRpc), bridge: pk(net.programs.bridge), storefront: pk(net.programs.storefront) };
}
type Ctx = Awaited<ReturnType<typeof ctx>>;

// ---------------------------------------------------------------- activity

export async function recordActivity(a: Omit<ActivityItem, "id" | "at"> & { id?: string }) {
  const list = (await getLocal("activity")) ?? [];
  const item: ActivityItem = { id: a.id ?? rid(), at: Date.now(), ...a };
  list.unshift(item);
  await setLocal({ activity: list.slice(0, 300) });
  return item;
}
async function patchActivity(ref: string, patch: Partial<ActivityItem>) {
  const list = (await getLocal("activity")) ?? [];
  const it = list.find((x) => x.ref === ref);
  if (it) {
    Object.assign(it, patch);
    await setLocal({ activity: list });
  }
}

export async function activity(account: string) {
  const { net, sol } = await ctx();
  const local = ((await getLocal("activity")) ?? []).filter((a) => a.account === account && a.network === net.id);
  // reconcile pending local Solana items
  const pend = local.filter((a) => a.chain === "solana" && a.status === "pending");
  if (pend.length) {
    try {
      const st = await sol.statuses(pend.map((p) => p.ref));
      for (let i = 0; i < pend.length; i++) {
        const s = st[i];
        if (s?.err) await patchActivity(pend[i].ref, { status: "failed", error: JSON.stringify(s.err) });
        else if (s && s.confirmationStatus !== "processed") await patchActivity(pend[i].ref, { status: "confirmed" });
      }
    } catch {
      /* offline */
    }
  }
  const chainPend = local.filter((a) => a.chain === "pearl" && a.status === "pending");
  if (chainPend.length) {
    const { chain } = await ctx();
    for (const p of chainPend) {
      try {
        const t = await chain.tx(p.ref);
        if (t?.status === "included") await patchActivity(p.ref, { status: "confirmed" });
      } catch {
        /* offline */
      }
    }
  }
  let onchain: { signature: string; slot: number; err: unknown; blockTime: number | null }[] = [];
  let solanaError: string | undefined;
  try {
    onchain = await sol.signatures(account, 15);
  } catch (e) {
    solanaError = errMsg(e);
  }
  const fresh = ((await getLocal("activity")) ?? []).filter((a) => a.account === account && a.network === net.id);
  const known = new Set(fresh.map((a) => a.ref));
  const extra: ActivityItem[] = onchain.filter((s) => !known.has(s.signature)).map((s) => ({
    id: s.signature, account, network: net.id, chain: "solana", kind: "solana", title: "Solana transaction", ref: s.signature,
    status: s.err ? "failed" : "confirmed", at: (s.blockTime ?? 0) * 1000,
  }));
  return { items: [...fresh, ...extra].sort((a, b) => b.at - a.at).slice(0, 60), solanaError };
}

// ---------------------------------------------------------------- bridge config / balances

const bridgeCache = new Map<string, { at: number; cfg: BridgeConfig; decimals: number }>();

export async function bridgeConfig(c: Ctx, fresh = false): Promise<{ cfg: BridgeConfig; decimals: number }> {
  const key = `${c.net.id}:${c.net.solanaRpc}:${c.net.programs.bridge}`;
  const hit = bridgeCache.get(key);
  if (!fresh && hit && Date.now() - hit.at < 20_000) return hit;
  const acc = await c.sol.account(bridgeConfigPda(c.bridge));
  if (!acc) throw new Error(`bridge not initialized at ${c.net.programs.bridge} on ${c.net.solanaRpc}`);
  const cfg = decodeBridgeConfig(acc.data);
  if (c.net.mint && c.net.mint !== b58(cfg.mint)) throw new Error("the configured mint does not match the bridge's mint");
  const mintAcc = await c.sol.account(cfg.mint);
  const decimals = mintAcc ? mintDecimals(mintAcc.data) : SPL_DECIMALS;
  const v = { at: Date.now(), cfg, decimals };
  bridgeCache.set(key, v);
  return v;
}

export async function balances(address: string): Promise<Balances> {
  const c = await ctx();
  const out: Balances = { sol: null, token: null, tokenDecimals: SPL_DECIMALS, native: null, nonce: null, mint: null, errors: {}, chain: null, agent: null };
  const me = pk(address);
  await Promise.all([
    (async () => {
      try {
        out.sol = (await c.sol.balance(address)).toString();
      } catch (e) {
        out.errors.solana = errMsg(e);
        return;
      }
      try {
        const { cfg, decimals } = await bridgeConfig(c);
        out.mint = b58(cfg.mint);
        out.tokenDecimals = decimals;
        const t = await c.sol.account(ata(me, cfg.mint, cfg.tokenProgram));
        out.token = t ? tokenAccountAmount(t.data).toString() : "0";
      } catch (e) {
        out.errors.bridge = errMsg(e);
      }
    })(),
    (async () => {
      try {
        const [a, info] = await Promise.all([c.chain.account(address), c.chain.chainInfo()]);
        out.native = a.balance;
        out.nonce = a.nonce;
        out.chain = { height: info.height, finalizedHeight: info.finalized_height, finalityK: info.finality_k, chainId: info.chain_id };
        const ag = await c.chain.agent(address).catch(() => null);
        out.agent = ag ? { name: ag.name, uri: ag.uri, registered_height: ag.registered_height } : null;
      } catch (e) {
        out.errors.chain = errMsg(e);
      }
    })(),
  ]);
  return out;
}

// ---------------------------------------------------------------- agent-chain txs

const paramCache = new Map<string, { at: number; minFee: bigint; chainId: number; k: number; contractFee: ContractFeeParams | null }>();
async function chainParams(c: Ctx) {
  const hit = paramCache.get(c.net.chainRpc);
  if (hit && Date.now() - hit.at < 30_000) return hit;
  const [p, info] = await Promise.all([c.chain.params(), c.chain.chainInfo()]);
  const v = { at: Date.now(), minFee: BigInt(p.min_fee), chainId: info.chain_id, k: info.finality_k, contractFee: contractFeeParams(p) };
  paramCache.set(c.net.chainRpc, v);
  return v;
}

/** Next nonce: the node's account nonce, or one past what this wallet sent recently (mempool). */
async function nextNonce(c: Ctx, address: string): Promise<bigint> {
  const a = await c.chain.account(address);
  const cache = (await getLocal("chainNonces")) ?? {};
  const k = `${c.net.chainRpc}:${address}`;
  const local = cache[k];
  const n = local && Date.now() - local.at < 3 * 60_000 && local.nonce > a.nonce ? local.nonce : a.nonce;
  return BigInt(n);
}
async function bumpNonce(c: Ctx, address: string, used: bigint) {
  const cache = (await getLocal("chainNonces")) ?? {};
  cache[`${c.net.chainRpc}:${address}`] = { nonce: Number(used) + 1, at: Date.now() };
  await setLocal({ chainNonces: cache });
}

export async function prepareChainTx(address: string, body: TxBody, fee?: bigint): Promise<UnsignedTx> {
  const c = await ctx();
  const p = await chainParams(c);
  const t = { chainId: p.chainId, from: pk(address), nonce: await nextNonce(c, address), fee: fee ?? p.minFee, body };
  if (fee === undefined && isContractKind(body.kind)) {
    // contract kinds pay the gas fee rule (getParams); the unused gas part is refunded
    if (!p.contractFee) throw new Error("this node does not report the contract fee parameters (gas_price, contract_base_fee, contract_byte_fee)");
    t.fee = contractFeeFor(t, p.contractFee);
  }
  return t;
}

/** Fee rule and known contract labels for a contract tx's approval text. */
export async function contractSummaryContext(me: string, t: UnsignedTx): Promise<ContractContext> {
  const c = await ctx();
  const p = await chainParams(c).catch(() => null);
  const labels: Record<string, string> = {};
  const b = t.body;
  if (b.kind === "Call" || b.kind === "Migrate" || b.kind === "SetContractAdmin") {
    const info = await c.chain.contract(b58(b.contract)).catch(() => null);
    if (info?.label) labels[b58(b.contract)] = info.label;
  }
  return { me, fee: p?.contractFee ?? null, labels };
}

export async function signChainTx(address: string, t: UnsignedTx) {
  if (b58(t.from) !== address) throw new Error("the transaction's from is not this account");
  const seed = await seedFor(address);
  return signTx(seed, t);
}

export async function sendChainTx(address: string, t: UnsignedTx, title: string, kind: string, origin?: string, amount?: string) {
  const c = await ctx();
  const { bytes, hash } = await signChainTx(address, t);
  const r = await c.chain.sendTx(b64(bytes));
  if (r.tx_hash !== hex(hash)) throw new Error(`node reported tx hash ${r.tx_hash}, expected ${hex(hash)}`);
  await bumpNonce(c, address, t.nonce);
  await recordActivity({ account: address, network: c.net.id, chain: "pearl", kind, title, ref: r.tx_hash, status: "pending", origin, amount });
  return r.tx_hash;
}

export async function sendNative(address: string, to: string, amount: bigint) {
  if (!isAddress(to)) throw new Error("recipient is not a base58 address");
  if (amount <= 0n) throw new Error("amount must be positive");
  const t = await prepareChainTx(address, { kind: "Transfer", to: key32(to), amount });
  return sendChainTx(address, t, "Sent on agent chain", "transfer", undefined, amount.toString());
}

export async function registerAgent(address: string, name: string, uri: string) {
  if (!name.trim()) throw new Error("name is required");
  if (utf8(name).length > 255) throw new Error("name is over 255 bytes");
  const t = await prepareChainTx(address, { kind: "RegisterAgent", name: name.trim(), uri: uri.trim() });
  return sendChainTx(address, t, `Registered agent ${name.trim()}`, "register");
}

// ---------------------------------------------------------------- Solana sends

async function sendSolana(c: Ctx, address: string, ixs: Instruction[], title: string, kind: string, amount?: string, origin?: string) {
  const seed = await seedFor(address);
  const bh = await c.sol.blockhash();
  const { tx, signature } = buildSigned(ixs, seed, pk(address), bh);
  const sig = await c.sol.send(tx);
  await recordActivity({ account: address, network: c.net.id, chain: "solana", kind, title, ref: sig, status: "pending", amount, origin });
  if (sig !== signature) throw new Error("RPC returned a different signature");
  return sig;
}

export async function sendSol(address: string, to: string, lamports: bigint) {
  if (!isAddress(to)) throw new Error("recipient is not a base58 address");
  const c = await ctx();
  return sendSolana(c, address, [systemTransfer(pk(address), pk(to), lamports)], "Sent SOL", "send-sol", lamports.toString());
}

export async function sendToken(address: string, to: string, amount: bigint) {
  if (!isAddress(to)) throw new Error("recipient is not a base58 address");
  const c = await ctx();
  const { cfg, decimals } = await bridgeConfig(c);
  const me = pk(address);
  const dest = pk(to);
  return sendSolana(c, address, [
    createAtaIdempotent(me, dest, cfg.mint, cfg.tokenProgram),
    transferChecked(ata(me, cfg.mint, cfg.tokenProgram), cfg.mint, ata(dest, cfg.mint, cfg.tokenProgram), me, amount, decimals, cfg.tokenProgram),
  ], "Sent token on Solana", "send-token", amount.toString());
}

// ---------------------------------------------------------------- bridge

async function savePending(p: PendingTransfer) {
  const list = (await getLocal("pending")) ?? [];
  const i = list.findIndex((x) => x.id === p.id);
  if (i >= 0) list[i] = p;
  else list.unshift(p);
  await setLocal({ pending: list.slice(0, 100) });
}

export async function bridgeIn(address: string, amountSpl: bigint, recipient?: string) {
  if (amountSpl <= 0n) throw new Error("amount must be positive");
  const to = recipient?.trim() || address;
  if (!isAddress(to)) throw new Error("agent-chain recipient is not a base58 address");
  const c = await ctx();
  let last: unknown;
  for (let attempt = 0; attempt < 3; attempt++) {
    const { cfg } = await bridgeConfig(c, true);
    try {
      const sig = await sendSolana(c, address, [bridgeDepositIx(c.bridge, cfg, pk(address), amountSpl, pk(to))], "Bridge to agent chain", "bridge-in", amountSpl.toString());
      const p: PendingTransfer = {
        id: rid(), account: address, network: c.net.id, direction: "in", amountSpl: amountSpl.toString(), createdAt: Date.now(), updatedAt: Date.now(),
        stage: "submitted", sourceRef: sig, depositNonce: Number(cfg.depositNonce),
      };
      await savePending(p);
      return p;
    } catch (e) {
      last = e;
      // another deposit took this nonce (seeds constraint / already in use): re-read and retry
      if (!/already in use|ConstraintSeeds|0x7d6|seeds constraint/i.test(errMsg(e))) break;
    }
  }
  throw last;
}

export async function bridgeOut(address: string, amount: bigint, solanaRecipient?: string) {
  if (amount <= 0n || amount % SPL_TO_NATIVE !== 0n) throw new Error("amount must be a positive multiple of 1000 base units (0.000001 token)");
  const to = solanaRecipient?.trim() || address;
  if (!isAddress(to)) throw new Error("Solana recipient is not a base58 address");
  const c = await ctx();
  const t = await prepareChainTx(address, { kind: "BridgeLock", solanaRecipient: key32(to), amount });
  const hash = await sendChainTx(address, t, "Bridge to Solana", "bridge-out", undefined, amount.toString());
  const p: PendingTransfer = {
    id: rid(), account: address, network: c.net.id, direction: "out", amountSpl: (amount / SPL_TO_NATIVE).toString(), createdAt: Date.now(), updatedAt: Date.now(),
    stage: "submitted", sourceRef: hash,
  };
  await savePending(p);
  return p;
}

export async function refreshPending(address?: string): Promise<PendingTransfer[]> {
  const c = await ctx();
  const list = (await getLocal("pending")) ?? [];
  const mine = list.filter((p) => p.network === c.net.id && (!address || p.account === address));
  for (const p of mine) {
    if (p.stage === "done" || p.stage === "failed") continue;
    try {
      if (p.direction === "in") await stepIn(c, p);
      else await stepOut(c, p);
    } catch (e) {
      p.error = errMsg(e);
    }
    p.updatedAt = Date.now();
    await savePending(p);
  }
  return ((await getLocal("pending")) ?? []).filter((p) => p.network === c.net.id && (!address || p.account === address));
}

async function stepIn(c: Ctx, p: PendingTransfer) {
  if (p.stage === "submitted") {
    const [s] = await c.sol.statuses([p.sourceRef]);
    if (s?.err) {
      p.stage = "failed";
      p.error = `Solana deposit failed: ${JSON.stringify(s.err)}`;
      return;
    }
    if (!s || s.confirmationStatus === "processed") return;
    p.stage = "relaying";
    await patchActivity(p.sourceRef, { status: "confirmed" });
  }
  const d = await c.chain.deposit(p.depositNonce!);
  p.error = undefined;
  if (d.processed) {
    p.stage = "done";
    p.destRef = `unlock at height ${d.height}`;
    p.confirmations = d.confirmations;
  }
}

async function stepOut(c: Ctx, p: PendingTransfer) {
  const k = (await chainParams(c)).k;
  p.finalityK = k;
  if (p.withdrawalNonce === undefined) {
    const t = await c.chain.tx(p.sourceRef);
    if (!t) {
      if (Date.now() - p.createdAt > 15 * 60_000) {
        p.stage = "failed";
        p.error = "the node no longer knows this transaction (dropped from the mempool)";
      }
      return;
    }
    if (t.status !== "included" || t.height === undefined) return;
    p.sourceHeight = t.height;
    p.stage = "waiting-final";
    await patchActivity(p.sourceRef, { status: "confirmed" });
    // find the withdrawal this tx appended: same sender, same height, same amount
    const ev = await c.chain.bridgeEvents(0, 1);
    const total = ev.withdrawal_count;
    for (let from = Math.max(0, total - 200); from < total; from += 100) {
      const page = await c.chain.bridgeEvents(from, 100);
      const w = page.withdrawals.find((x) => x.from === p.account && x.height === t.height && x.amount_spl === p.amountSpl);
      if (w) {
        p.withdrawalNonce = w.nonce;
        break;
      }
    }
    if (p.withdrawalNonce === undefined) return;
  }
  const page = await c.chain.bridgeEvents(p.withdrawalNonce, 1);
  const w = page.withdrawals[0];
  if (w) {
    p.confirmations = w.confirmations;
    if (w.final && p.stage === "waiting-final") p.stage = "relaying";
  }
  if (p.stage === "relaying") {
    const acc = await c.sol.account(processedPda(c.bridge, BigInt(p.withdrawalNonce)));
    if (acc) {
      p.stage = "done";
      p.destRef = b58(processedPda(c.bridge, BigInt(p.withdrawalNonce)));
    }
  }
  p.error = undefined;
}

// ---------------------------------------------------------------- jobs

export async function listJobs(status: string | undefined, offset: number, limit: number) {
  const c = await ctx();
  return c.chain.listJobs({ status: status || undefined, offset, limit });
}
export async function getJob(jobId: string) {
  const c = await ctx();
  const j = await c.chain.job(jobId);
  if (!j) return j;
  // Recompute the Pearl header from header_id || terms_hash and check job_key locally.
  let headerCheck: string | null;
  try {
    headerCheck = jobHeaderProblem(j);
  } catch (e) {
    headerCheck = errMsg(e);
  }
  return { ...j, headerOk: headerCheck === null, headerCheck };
}

export interface PostJobInput {
  m: number;
  n: number;
  config: string;
  dataUri: string;
  workerPay: string;
  protocolFee: string;
  deadlineHeight: number;
  maxBlocks: number;
  evaluators: string[];
  threshold: number;
  a: string; // base64 int8
  b: string; // base64 int8
}

export async function preparePostJob(address: string, i: PostJobInput) {
  const t0 = await prepareChainTx(address, { kind: "Transfer", to: new Uint8Array(32), amount: 0n });
  const evaluators = i.evaluators.map((e) => key32(e));
  if (i.threshold < 1 || i.threshold > evaluators.length) throw new Error("threshold must be between 1 and the number of evaluators");
  const cm = commitJob({
    poster: pk(address), nonce: t0.nonce, m: i.m, n: i.n, config: fromHex(i.config), workerPay: BigInt(i.workerPay), protocolFee: BigInt(i.protocolFee),
    deadlineHeight: BigInt(i.deadlineHeight), maxBlocks: i.maxBlocks, a: fromB64(i.a), b: fromB64(i.b),
  });
  const body: PostJobBody = {
    m: i.m, n: i.n, k: cm.k, config: fromHex(i.config), hashA: cm.hashA, hashB: cm.hashB, dataUri: i.dataUri, workerPay: BigInt(i.workerPay),
    protocolFee: BigInt(i.protocolFee), deadlineHeight: BigInt(i.deadlineHeight), maxBlocks: i.maxBlocks, evaluators, threshold: i.threshold,
  };
  const tx: UnsignedTx = { ...t0, body: { kind: "PostJob", ...body } };
  return { tx, jobId: hex(cm.jobId), jobKey: hex(cm.jobKey), hashA: hex(cm.hashA), hashB: hex(cm.hashB), termsHash: hex(cm.termsHash) };
}

export async function postJob(address: string, i: PostJobInput) {
  const p = await preparePostJob(address, i);
  const hash = await sendChainTx(address, p.tx, "Posted a job", "post-job", undefined, i.workerPay);
  return { txHash: hash, jobId: p.jobId, jobKey: p.jobKey, hashA: p.hashA, hashB: p.hashB };
}

// ---------------------------------------------------------------- storefront

async function market(c: Ctx, orderSol: bigint) {
  const cfgAcc = await c.sol.account(storeConfigPda(c.storefront));
  if (!cfgAcc) throw new Error(`storefront not initialized at ${c.net.programs.storefront}`);
  const cfg = decodeStoreConfig(cfgAcc.data);
  // buy_job moves buyback_bps of sol_in into the buyback vault first and trades the rest
  const buyback = (orderSol * BigInt(cfg.buybackBps)) / 10_000n;
  const solIn = orderSol - buyback;
  const mint = cfg.mint;
  const [curveAcc, globalAcc, ammCfgAcc, poolAcc] = await c.sol.accounts([bondingCurvePda(mint), pda([utf8("global")], PUMP), pda([utf8("global_config")], AMM), canonicalPool(mint)]);
  if (!curveAcc || !globalAcc) throw new Error("pump coin or pump Global missing on this Solana RPC");
  const curve = decodeCurve(curveAcc.data);
  const p: PumpAccounts = { mint, curveBuyback: pumpGlobalBuyback(globalAcc.data), ammBuyback: ammCfgAcc ? ammGlobalBuyback(ammCfgAcc.data) : pumpGlobalBuyback(globalAcc.data) };
  let quote: bigint;
  let venue: "curve" | "pumpswap" = "curve";
  if (!curve.complete) quote = cpQuote(curve.virtualToken, curve.virtualQuote, solIn);
  else {
    if (!poolAcc) throw new Error("curve complete but the PumpSwap pool is missing");
    venue = "pumpswap";
    const pl = decodePool(poolAcc.data);
    p.poolBaseVault = pl.baseVault;
    p.poolQuoteVault = pl.quoteVault;
    const [bv, qv] = await c.sol.accounts([pl.baseVault, pl.quoteVault]);
    if (!bv || !qv) throw new Error("pool vaults missing");
    let q = tokenAccountAmount(qv.data) - pl.protocolFees - pl.creatorFees;
    if (q < 0n) q = 0n;
    quote = cpQuote(tokenAccountAmount(bv.data), q + pl.virtualQuote, solIn);
  }
  const minOut = (quote * BigInt(10_000 - cfg.maxSlippageBps)) / 10_000n;
  const p_ = (quote * BigInt(cfg.protocolFeeBps)) / 10_000n;
  return { cfg, venue, quote, minOut: minOut > 0n ? minOut : 1n, accounts: p, protocolFee: p_, workerPay: quote - p_, buyback, jobSol: solIn };
}

export async function storefrontQuote(solIn: bigint) {
  const c = await ctx();
  const m = await market(c, solIn);
  return {
    venue: m.venue, quoteSpl: m.quote.toString(), minOutSpl: m.minOut.toString(), workerPaySpl: m.workerPay.toString(), protocolFeeSpl: m.protocolFee.toString(),
    protocolFeeBps: m.cfg.protocolFeeBps, maxSlippageBps: m.cfg.maxSlippageBps,
    buybackBps: m.cfg.buybackBps, buybackLamports: m.buyback.toString(), jobLamports: m.jobSol.toString(), buybackDestination: m.cfg.buybackDest === 1 ? "burn" : "reserve", minSolIn: m.cfg.minSolIn.toString(), maxSolIn: m.cfg.maxSolIn.toString(),
    paused: m.cfg.paused, maxDim: m.cfg.maxDim, maxBlocksCap: m.cfg.maxBlocksCap, maxDeadlineBlocks: m.cfg.maxDeadlineBlocks.toString(),
  };
}

export interface StorefrontJobInput {
  m: number;
  n: number;
  config: string;
  a: string; // base64 int8, m x k
  b: string; // base64 int8, k x n
  dataUri: string;
  deadlineBlocks: string; // relative to the unlock height
  maxBlocks: number;
  evaluators: string[];
  threshold: number;
}

export interface StorefrontInput {
  solIn: string;
  recipient?: string;
  spec: StorefrontJobInput;
}

/**
 * The order's commitments, computed from the matrices: the job header is
 * `recipient || order_terms(recipient, m, n, k, max_blocks, deadline_blocks)`.
 */
export function prepareStorefrontJob(address: string, recipient: string | undefined, j: StorefrontJobInput) {
  const poster = pk(recipient?.trim() || address);
  if (!j.evaluators.length) throw new Error("add at least one evaluator address (the attester in M1)");
  if (j.threshold < 1 || j.threshold > j.evaluators.length) throw new Error("threshold must be between 1 and the number of evaluators");
  const deadlineBlocks = BigInt(j.deadlineBlocks);
  if (deadlineBlocks < 1n) throw new Error("deadline must be at least 1 block after the unlock");
  const config = fromHex(j.config);
  const cm = commitOrder({ poster, m: j.m, n: j.n, config, maxBlocks: j.maxBlocks, deadlineBlocks, a: fromB64(j.a), b: fromB64(j.b) });
  const spec: StoreJobSpec = {
    m: j.m, n: j.n, k: cm.k, config, hashA: cm.hashA, hashB: cm.hashB, dataUri: j.dataUri,
    deadlineBlocks, maxBlocks: j.maxBlocks, evaluators: j.evaluators.map((e) => pk(e)), threshold: j.threshold,
  };
  return {
    spec, recipient: poster,
    view: { headerId: hex(cm.headerId), recipient: b58(poster), termsHash: hex(cm.termsHash), header: hex(cm.header), jobKey: hex(cm.jobKey), hashA: hex(cm.hashA), hashB: hex(cm.hashB), k: cm.k },
  };
}

export async function storefrontBuy(address: string, i: StorefrontInput) {
  const c = await ctx();
  const solIn = BigInt(i.solIn);
  const { spec, recipient, view } = prepareStorefrontJob(address, i.recipient, i.spec);
  const me = pk(address);
  const draftId = new DataView(crypto.getRandomValues(new Uint8Array(8)).buffer).getBigUint64(0, true);
  const openSig = await sendSolana(c, address, [openOrderIx(c.storefront, me, draftId, recipient, spec)], "Opened storefront order", "storefront-open");
  await c.sol.confirm(openSig);
  let last: unknown;
  for (let attempt = 0; attempt < 3; attempt++) {
    const { cfg } = await bridgeConfig(c, true);
    const nonce = cfg.depositNonce;
    const m = await market(c, solIn);
    const ix = m.venue === "curve"
      ? buyJobIx(c.storefront, c.bridge, me, draftId, nonce, solIn, m.minOut, m.accounts)
      : buyJobAmmIx(c.storefront, c.bridge, me, draftId, nonce, solIn, m.minOut, m.accounts);
    try {
      const sig = await sendSolana(c, address, [computeUnitLimit(400_000), ix], "Bought an AI job", "storefront-buy", solIn.toString());
      return { openSig, buySig: sig, nonce: Number(nonce), order: b58(orderPda(c.storefront, nonce)), jobId: hex(orderJobId(nonce)), venue: m.venue, ...view };
    } catch (e) {
      last = e;
      if (!/WrongNonce|0x1779|already in use/.test(errMsg(e))) break;
    }
  }
  try {
    await sendSolana(c, address, [cancelDraftIx(c.storefront, me, draftId)], "Cancelled order draft", "storefront-cancel");
  } catch {
    /* draft rent stays */
  }
  throw last;
}

export async function storefrontOrder(nonce: number) {
  const c = await ctx();
  const acc = await c.sol.account(orderPda(c.storefront, BigInt(nonce)));
  if (!acc) return null;
  const o = decodeOrder(acc.data);
  return { status: o.status, venue: o.venue, jobId: hex(o.jobId), workerPay: o.workerPay.toString(), protocolFee: o.protocolFee.toString(), solSpent: o.solSpent.toString(), tokens: o.tokens.toString(), resultHash: o.resultHash ? hex(o.resultHash) : null };
}

// ---------------------------------------------------------------- mining rewards

export async function miningRewards(address: string, force = false) {
  const c = await ctx();
  const info = await c.chain.chainInfo();
  const k = info.finality_k;
  const depth = Math.max(10, c.net.rewardScanDepth);
  const cacheAll = (await getLocal("rewardCache")) ?? {};
  const key = `${c.net.id}:${address}`;
  let cache = cacheAll[key];
  const floor = Math.max(0, info.height - depth + 1);
  if (!cache || force || cache.scannedFrom > floor + depth) cache = { network: c.net.id, scannedTo: floor - 1, scannedFrom: floor, blocks: [] };
  // re-scan the unfinal window (reorgs) plus anything new
  const rescanFrom = Math.max(floor, Math.min(cache.scannedTo + 1, info.height - k));
  cache.blocks = cache.blocks.filter((b) => b.height < rescanFrom && b.height >= floor);
  const heights: number[] = [];
  for (let h = rescanFrom; h <= info.height; h++) heights.push(h);
  const batch = 16;
  for (let i = 0; i < heights.length; i += batch) {
    const got = await Promise.all(heights.slice(i, i + batch).map((h) => c.chain.block(h).catch(() => null)));
    for (const b of got) {
      if (!b || b.header_fields.miner !== address) continue;
      const fees = b.fees != null ? BigInt(b.fees) : b.txs.reduce((s, t) => s + BigInt(t.fee), 0n);
      const jobBlock = !/^0+$/.test(b.header_fields.job_id);
      const subsidy = b.subsidy != null ? b.subsidy : undefined;
      const reward = b.reward != null ? b.reward : subsidy !== undefined ? (BigInt(subsidy) + fees).toString() : undefined;
      cache.blocks.push({ height: b.header_fields.height, hash: b.hash, fees: fees.toString(), subsidy, reward, jobBlock });
    }
  }
  cache.scannedFrom = floor;
  cache.scannedTo = info.height;
  cacheAll[key] = cache;
  await setLocal({ rewardCache: cacheAll });
  const blocks = cache.blocks.slice().sort((a, b) => b.height - a.height).map((b) => ({ ...b, confirmations: info.height - b.height, matured: info.height - b.height >= k }));
  const sum = (xs: typeof blocks, f: (b: (typeof blocks)[0]) => string | undefined) => xs.reduce((s, b) => s + BigInt(f(b) ?? "0"), 0n).toString();
  const matured = blocks.filter((b) => b.matured);
  const immature = blocks.filter((b) => !b.matured);
  return {
    height: info.height, finalityK: k, scanned: { from: floor, to: info.height },
    rewardReported: blocks.some((b) => b.reward !== undefined),
    totals: {
      blocks: blocks.length, maturedBlocks: matured.length, immatureBlocks: immature.length, jobBlocks: blocks.filter((b) => b.jobBlock).length,
      maturedFees: sum(matured, (b) => b.fees), immatureFees: sum(immature, (b) => b.fees),
      maturedReward: sum(matured, (b) => b.reward), immatureReward: sum(immature, (b) => b.reward),
    },
    blocks: blocks.slice(0, 50),
  };
}

export async function jobsByPoster(address: string): Promise<Job[]> {
  const c = await ctx();
  const r = await c.chain.listJobs({ offset: 0, limit: 200 });
  return r.jobs.filter((j) => j.poster === address);
}

export const addressOfSeed = (seed: Uint8Array) => b58(pubkeyOf(seed));
export type { Network };
