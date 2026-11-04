// A fake Robinhood Chain behind a viem `custom` transport, so the RH relayer runs its real
// ViemEvm client (eth_call decoding, local signing, raw tx submit, receipts, getLogs)
// against in-memory CadreToken / CadreBridge / CadreStorefront / CadreHeaderOracle.
// Every tx mines one block and the state is kept per block, so reads at the finality block
// see old state. EIP-712 bundles are recovered and checked (threshold, ascending order,
// membership) like token-evm/src/lib/SignerSet.sol.

import { createHash } from "node:crypto";
import {
  custom, decodeFunctionData, encodeAbiParameters, encodeEventTopics, encodeFunctionResult, keccak256, parseTransaction,
  recoverAddress, recoverTransactionAddress, getAddress,
} from "viem";
import type { Address, Hex, Transport } from "viem";
import { bridgeAbi, erc20Abi, oracleAbi, storefrontAbi } from "../src/evm/abi.ts";
import { expiredDigest, finalHeaderDigest, refundDigest, releaseDigest, resultDigest } from "../src/evm/sign.ts";
import { orderJobIdV2 } from "../src/codec.ts";

const Z = "0x0000000000000000000000000000000000000000" as Address;
const lc = (a: string) => a.toLowerCase();

interface Dep { sender: Address; amount: bigint; refundTo: Address; blockNumber: bigint; recipient: Hex; refunded: boolean }
interface Ord {
  buyer: Address; status: number; shareSettled: boolean; tokens: bigint; workerPay: bigint; protocolFee: bigint; ethSpent: bigint;
  share: bigint; recipient: Hex; jobId: Hex; bodyHash: Hex; deliverer: Hex; resultHash: Hex;
}
interface Rec { height: bigint; txRoot: Hex; stateRoot: Hex; withdrawRoot: Hex }

export interface RhState {
  balances: Record<string, bigint>;
  eth: Record<string, bigint>;
  depositNonce: bigint;
  deposits: Record<string, Dep>;
  processed: Record<string, boolean>;
  releaseModes: number;
  bridgePaused: boolean;
  bridgeSigners: Address[];
  bridgeThreshold: number;
  orders: Record<string, Ord>;
  storeSigners: Address[];
  storeThreshold: number;
  oracleSigners: Address[];
  oracleThreshold: number;
  records: Record<string, Rec>;
  hashAtHeight: Record<string, Hex>;
  latestHeight: bigint;
  latestHash: Hex;
}

export interface Log { address: Address; topics: Hex[]; data: Hex; blockNumber: bigint }

export class FakeRh {
  readonly chainId: bigint;
  readonly token = "0x7000000000000000000000000000000000000001" as Address;
  readonly bridge = "0x7000000000000000000000000000000000000002" as Address;
  readonly store = "0x7000000000000000000000000000000000000003" as Address;
  readonly oracle = "0x7000000000000000000000000000000000000004" as Address;
  readonly owner = "0x70000000000000000000000000000000000000aa" as Address;
  readonly domain: number;
  readonly cadreChainId: bigint;
  history: RhState[] = [];
  s: RhState;
  logs: Log[] = [];
  receipts = new Map<string, { block: bigint; ok: boolean; from: Address; to: Address }>();
  nonces = new Map<string, number>();
  sent: { fn: string; args: readonly unknown[]; from: Address }[] = [];
  reverts: string[] = [];

  constructor(o: { chainId?: bigint; domain?: number; cadreChainId?: bigint; bridgeSigners: Address[]; bridgeThreshold: number;
    storeSigners: Address[]; storeThreshold: number; oracleSigners: Address[]; oracleThreshold: number; modes?: number }) {
    this.chainId = o.chainId ?? 46630n;
    this.domain = o.domain ?? 2;
    this.cadreChainId = o.cadreChainId ?? 1128350803n;
    this.s = {
      balances: {}, eth: {}, depositNonce: 0n, deposits: {}, processed: {}, releaseModes: o.modes ?? 3, bridgePaused: false,
      bridgeSigners: o.bridgeSigners, bridgeThreshold: o.bridgeThreshold, orders: {}, storeSigners: o.storeSigners, storeThreshold: o.storeThreshold,
      oracleSigners: o.oracleSigners, oracleThreshold: o.oracleThreshold, records: {}, hashAtHeight: {}, latestHeight: 0n,
      latestHash: `0x${"0".repeat(64)}`,
    };
    this.commit();
  }

  get block(): bigint {
    return BigInt(this.history.length - 1);
  }

  /** Seals the current state as a new block. */
  commit(): void {
    this.history.push(structuredClone(this.s));
  }

  mine(n: number): void {
    for (let i = 0; i < n; i++) this.commit();
  }

  bal(a: Address): bigint {
    return this.s.balances[lc(a)] ?? 0n;
  }

  mint(to: Address, amount: bigint): void {
    this.s.balances[lc(to)] = this.bal(to) + amount;
  }

  private move(from: Address, to: Address, amount: bigint): void {
    if (this.bal(from) < amount) throw new Error("ERC20: insufficient balance");
    this.s.balances[lc(from)] = this.bal(from) - amount;
    this.s.balances[lc(to)] = this.bal(to) + amount;
  }

  /** CadreBridge.deposit / depositFor, in a new block. */
  deposit(sender: Address, amount: bigint, recipient: Hex, refundTo: Address = Z): bigint {
    this.move(sender, this.bridge, amount);
    const n = this.s.depositNonce++;
    this.s.deposits[n.toString()] = { sender, amount, refundTo, blockNumber: this.block + 1n, recipient, refunded: false };
    this.commit();
    return n;
  }

  /** CadreStorefront.buyJob, simplified: the tokens already bought; emits OrderBody in the same block. */
  buyJob(buyer: Address, tokens: bigint, recipient: Hex, body: Buffer, workerPay: bigint, protocolFee: bigint, share: bigint, opts: { badHash?: boolean } = {}): bigint {
    this.mint(this.store, tokens);
    this.s.eth[lc(this.store)] = (this.s.eth[lc(this.store)] ?? 0n) + share;
    this.move(this.store, this.bridge, tokens);
    const n = this.s.depositNonce++;
    const blk = this.block + 1n;
    this.s.deposits[n.toString()] = { sender: this.store, amount: tokens, refundTo: buyer, blockNumber: blk, recipient, refunded: false };
    const bodyHash = (`0x${createHash("sha256").update(opts.badHash ? Buffer.from("x") : body).digest("hex")}`) as Hex;
    this.s.orders[n.toString()] = {
      buyer, status: 1, shareSettled: false, tokens, workerPay, protocolFee, ethSpent: 0n, share, recipient,
      jobId: `0x${orderJobIdV2(this.domain, n).toString("hex")}`, bodyHash, deliverer: `0x${"0".repeat(64)}`, resultHash: `0x${"0".repeat(64)}`,
    };
    const topics = encodeEventTopics({ abi: storefrontAbi, eventName: "OrderBody", args: { nonce: n } }) as Hex[];
    this.logs.push({ address: this.store, topics, data: encodeAbiParameters([{ type: "bytes" }], [`0x${body.toString("hex")}`]), blockNumber: blk });
    this.commit();
    return n;
  }

  private async sigsOk(digest: Hex, sigs: readonly Hex[], set: Address[], threshold: number): Promise<void> {
    if (sigs.length < threshold) throw new Error("NotEnoughSignatures");
    let last = 0n;
    for (const sig of sigs) {
      const a = await recoverAddress({ hash: digest, signature: sig });
      if (BigInt(a) <= last || !set.some((x) => lc(x) === lc(a))) throw new Error("BadSignature");
      last = BigInt(a);
    }
  }

  private st(at: string | undefined): RhState {
    if (!at || at === "latest" || at === "pending") return this.s;
    const n = Number(BigInt(at));
    if (n >= this.history.length) throw new Error("block not found");
    return this.history[n];
  }

  private view(to: Address, data: Hex, s: RhState): Hex {
    if (lc(to) === lc(this.token)) {
      const c = decodeFunctionData({ abi: erc20Abi, data });
      if (c.functionName === "balanceOf") return encodeFunctionResult({ abi: erc20Abi, functionName: "balanceOf", result: s.balances[lc(c.args[0])] ?? 0n });
    }
    if (lc(to) === lc(this.bridge)) {
      const c = decodeFunctionData({ abi: bridgeAbi, data });
      const r = (functionName: string, result: unknown) => encodeFunctionResult({ abi: bridgeAbi, functionName: functionName as never, result: result as never });
      switch (c.functionName) {
        case "token": return r("token", this.token);
        case "domain": return r("domain", this.domain);
        case "depositNonce": return r("depositNonce", s.depositNonce);
        case "releaseModes": return r("releaseModes", s.releaseModes);
        case "paused": return r("paused", s.bridgePaused);
        case "headers": return r("headers", this.oracle);
        case "threshold": return r("threshold", s.bridgeThreshold);
        case "signers": return r("signers", s.bridgeSigners);
        case "minWithdrawalNonce": return r("minWithdrawalNonce", 0n);
        case "deposits": {
          const d = s.deposits[String(c.args[0])] ?? { sender: Z, amount: 0n, refundTo: Z, blockNumber: 0n, recipient: `0x${"0".repeat(64)}`, refunded: false };
          return r("deposits", d);
        }
        case "isProcessed": return r("isProcessed", !!s.processed[String(c.args[0])]);
      }
    }
    if (lc(to) === lc(this.store)) {
      const c = decodeFunctionData({ abi: storefrontAbi, data });
      const r = (functionName: string, result: unknown) => encodeFunctionResult({ abi: storefrontAbi, functionName: functionName as never, result: result as never });
      switch (c.functionName) {
        case "threshold": return r("threshold", s.storeThreshold);
        case "signers": return r("signers", s.storeSigners);
        case "owner": return r("owner", this.owner);
        case "getOrder": {
          const z = `0x${"0".repeat(64)}` as Hex;
          return r("getOrder", s.orders[String(c.args[0])] ?? { buyer: Z, status: 0, shareSettled: false, tokens: 0n, workerPay: 0n, protocolFee: 0n,
            ethSpent: 0n, share: 0n, recipient: z, jobId: z, bodyHash: z, deliverer: z, resultHash: z });
        }
      }
    }
    if (lc(to) === lc(this.oracle)) {
      const c = decodeFunctionData({ abi: oracleAbi, data });
      const r = (functionName: string, result: unknown) => encodeFunctionResult({ abi: oracleAbi, functionName: functionName as never, result: result as never });
      const z = `0x${"0".repeat(64)}` as Hex;
      switch (c.functionName) {
        case "cadreChainId": return r("cadreChainId", this.cadreChainId);
        case "threshold": return r("threshold", s.oracleThreshold);
        case "signers": return r("signers", s.oracleSigners);
        case "latestFinal": return r("latestFinal", [s.latestHeight, s.latestHash]);
        case "hashAtHeight": return r("hashAtHeight", s.hashAtHeight[String(c.args[0])] ?? z);
        case "isFinal": return r("isFinal", !!s.records[lc(c.args[0] as string)]);
        case "roots": {
          const x = s.records[lc(c.args[0] as string)];
          return r("roots", x ? [x.height, x.txRoot, x.stateRoot, x.withdrawRoot] : [0n, z, z, z]);
        }
      }
    }
    throw new Error(`fake rh: no view for ${to} ${data.slice(0, 10)}`);
  }

  /** Applies a state-changing call to this.s (throws = revert). */
  private async exec(from: Address, to: Address, data: Hex): Promise<void> {
    const s = this.s;
    if (lc(to) === lc(this.bridge)) {
      const c = decodeFunctionData({ abi: bridgeAbi, data });
      this.sent.push({ fn: c.functionName, args: c.args ?? [], from });
      if (c.functionName === "release") {
        if ((s.releaseModes & 1) === 0) throw new Error("ModeDisabled");
        const [nonce, recipient, amount, sigs] = c.args as [bigint, Address, bigint, Hex[]];
        await this.sigsOk(releaseDigest(this.chainId, this.bridge, nonce, recipient, amount), sigs, s.bridgeSigners, s.bridgeThreshold);
        return this.pay(nonce, recipient, amount);
      }
      if (c.functionName === "releaseProof") {
        if ((s.releaseModes & 2) === 0) throw new Error("ModeDisabled");
        const [entry, , proof, headerHash] = c.args as [Hex, bigint, Hex, Hex];
        const rec = s.records[lc(headerHash)];
        if (!rec) throw new Error("NotFinal");
        const e = Buffer.from(entry.slice(2), "hex");
        if (e.length !== 90 || e[0] !== 2) throw new Error("BadEntry");
        if (e[1] !== this.domain) throw new Error("WrongDomain");
        const nonce = e.readBigUInt64LE(2);
        if (Buffer.from(proof.slice(2), "hex").toString() !== `proof:${e[1]}:${nonce}@${rec.height}`) throw new Error("BadProof");
        return this.pay(nonce, getAddress(`0x${e.subarray(54, 74).toString("hex")}`), e.readBigUInt64LE(74));
      }
      if (c.functionName === "refundDeposit") {
        const [nonce, sigs] = c.args as [bigint, Hex[]];
        const d = s.deposits[nonce.toString()];
        if (!d || d.refundTo === Z) throw new Error("NotAnOrder");
        if (d.refunded) throw new Error("AlreadyRefunded");
        const o = s.orders[nonce.toString()];
        if (!o || (o.status !== 1 && o.status !== 3)) throw new Error("NotRefundable");
        await this.sigsOk(refundDigest(this.chainId, this.bridge, nonce, d.refundTo, d.amount), sigs, s.bridgeSigners, s.bridgeThreshold);
        d.refunded = true;
        return this.move(this.bridge, d.refundTo, d.amount);
      }
    }
    if (lc(to) === lc(this.store)) {
      const c = decodeFunctionData({ abi: storefrontAbi, data });
      this.sent.push({ fn: c.functionName, args: c.args ?? [], from });
      const o = s.orders[String((c.args as readonly unknown[])[0])];
      if (!o) throw new Error("no order");
      const caller = () => {
        if (lc(from) !== lc(o.buyer) && lc(from) !== lc(this.owner) && !s.storeSigners.some((a) => lc(a) === lc(from))) throw new Error("Unauthorized");
      };
      if (c.functionName === "finalizeOrder") {
        const [, deliverer, resultHash, , sigs] = c.args as [bigint, Hex, Hex, string, Hex[]];
        if (o.status !== 1) throw new Error("NotPending");
        caller();
        await this.sigsOk(resultDigest(this.chainId, this.store, o.jobId, deliverer, resultHash), sigs, s.storeSigners, s.storeThreshold);
        Object.assign(o, { status: 2, deliverer, resultHash });
        return;
      }
      if (c.functionName === "expireOrder") {
        const [, sigs] = c.args as [bigint, Hex[]];
        if (o.status !== 1) throw new Error("NotPending");
        caller();
        await this.sigsOk(expiredDigest(this.chainId, this.store, o.jobId), sigs, s.storeSigners, s.storeThreshold);
        o.status = 3;
        return;
      }
      if (c.functionName === "markRefunded") {
        const n = String(c.args[0]);
        if (o.status !== 1 && o.status !== 3) throw new Error("NotPending");
        if (!s.deposits[n]?.refunded) throw new Error("NotRefunded");
        o.status = 4;
        return;
      }
      if (c.functionName === "settleShare") {
        if (o.shareSettled) throw new Error("AlreadySettled");
        if (o.status === 1) throw new Error("NotPending");
        o.shareSettled = true;
        return;
      }
    }
    if (lc(to) === lc(this.oracle)) {
      const c = decodeFunctionData({ abi: oracleAbi, data });
      this.sent.push({ fn: c.functionName, args: c.args ?? [], from });
      if (c.functionName === "postFinalHeader") {
        const [header, blockHash, sigs] = c.args as [Hex, Hex, Hex[]];
        const h = Buffer.from(header.slice(2), "hex");
        if (h.length !== 256) throw new Error("BadHeader");
        const height = h.readBigUInt64LE(4);
        const r = (o: number) => `0x${h.subarray(o, o + 32).toString("hex")}` as Hex;
        if (s.records[lc(blockHash)]) throw new Error("AlreadyPosted");
        if (s.hashAtHeight[height.toString()]) throw new Error("ConflictingHeader");
        const rec = { height, txRoot: r(120), stateRoot: r(152), withdrawRoot: r(184) };
        await this.sigsOk(finalHeaderDigest(this.chainId, this.oracle, this.cadreChainId, { blockHash, ...rec }), sigs, s.oracleSigners, s.oracleThreshold);
        s.records[lc(blockHash)] = rec;
        s.hashAtHeight[height.toString()] = blockHash;
        if (height > s.latestHeight || /^0x0+$/.test(s.latestHash)) {
          s.latestHeight = height;
          s.latestHash = blockHash;
        }
        return;
      }
    }
    throw new Error(`fake rh: no method for ${to} ${data.slice(0, 10)}`);
  }

  private pay(nonce: bigint, recipient: Address, amount: bigint): void {
    if (this.s.bridgePaused) throw new Error("Paused");
    if (this.s.processed[nonce.toString()]) throw new Error("AlreadyProcessed");
    this.s.processed[nonce.toString()] = true;
    this.move(this.bridge, recipient, amount);
  }

  /** Runs `exec` on a throwaway copy (estimateGas). */
  private async dry(from: Address, to: Address, data: Hex): Promise<void> {
    const saved = this.s;
    this.s = structuredClone(saved);
    const sent = this.sent.length;
    try {
      await this.exec(from, to, data);
    } finally {
      this.s = saved;
      this.sent.length = sent;
    }
  }

  transport(): Transport {
    const hexn = (n: bigint | number) => `0x${BigInt(n).toString(16)}`;
    return custom({
      request: async ({ method, params }: { method: string; params?: any }) => { // eslint-disable-line @typescript-eslint/no-explicit-any
        const p = (params ?? []) as any[]; // eslint-disable-line @typescript-eslint/no-explicit-any
        switch (method) {
          case "eth_chainId": return hexn(this.chainId);
          case "eth_blockNumber": return hexn(this.block);
          case "eth_getBlockByNumber": {
            const tag = p[0] as string;
            let n = this.block;
            if (tag === "safe") n = this.block > 4n ? this.block - 4n : 0n;
            else if (tag === "finalized") n = this.block > 8n ? this.block - 8n : 0n;
            else if (tag !== "latest" && tag !== "pending") n = BigInt(tag);
            return { number: hexn(n), hash: keccak256(`0x${n.toString(16).padStart(64, "0")}`), parentHash: `0x${"0".repeat(64)}`, timestamp: hexn(1_760_000_000n + n),
              baseFeePerGas: "0x1", gasLimit: "0x1c9c380", gasUsed: "0x0", transactions: [], logsBloom: `0x${"0".repeat(512)}`, miner: Z,
              difficulty: "0x0", extraData: "0x", nonce: "0x0000000000000000", sha3Uncles: `0x${"0".repeat(64)}`, size: "0x0", stateRoot: `0x${"0".repeat(64)}`,
              receiptsRoot: `0x${"0".repeat(64)}`, transactionsRoot: `0x${"0".repeat(64)}`, uncles: [], mixHash: `0x${"0".repeat(64)}`, totalDifficulty: "0x0" };
          }
          case "eth_call": return this.view(p[0].to, p[0].data ?? p[0].input, this.st(p[1]));
          case "eth_estimateGas":
            try {
              await this.dry(p[0].from, p[0].to, p[0].data ?? p[0].input);
            } catch (e) {
              this.reverts.push(String((e as Error).message));
              throw Object.assign(new Error(`execution reverted: ${(e as Error).message}`), { code: 3 });
            }
            return "0x30000";
          case "eth_maxPriorityFeePerGas": return "0x1";
          case "eth_gasPrice": return "0x2";
          case "eth_getTransactionCount": return hexn(this.nonces.get(lc(p[0])) ?? 0);
          case "eth_sendRawTransaction": {
            const raw = p[0] as Hex;
            const tx = parseTransaction(raw);
            const from = await recoverTransactionAddress({ serializedTransaction: raw as never });
            const hash = keccak256(raw);
            this.nonces.set(lc(from), (this.nonces.get(lc(from)) ?? 0) + 1);
            let ok = true;
            try {
              await this.exec(from, tx.to as Address, tx.data as Hex);
            } catch (e) {
              ok = false;
              this.reverts.push(String((e as Error).message));
            }
            this.commit();
            this.receipts.set(hash, { block: this.block, ok, from, to: tx.to as Address });
            return hash;
          }
          case "eth_getTransactionReceipt": {
            const r = this.receipts.get(p[0]);
            if (!r) return null;
            return { transactionHash: p[0], blockNumber: hexn(r.block), blockHash: keccak256(`0x${r.block.toString(16).padStart(64, "0")}`), status: r.ok ? "0x1" : "0x0",
              from: r.from, to: r.to, logs: [], cumulativeGasUsed: "0x1", gasUsed: "0x1", effectiveGasPrice: "0x1", type: "0x2", contractAddress: null,
              transactionIndex: "0x0", logsBloom: `0x${"0".repeat(512)}` };
          }
          case "eth_getTransactionByHash": return null;
          case "eth_getLogs": {
            const f = p[0];
            const from = BigInt(f.fromBlock), to = BigInt(f.toBlock);
            const topics = (f.topics ?? []) as (Hex | null)[];
            return this.logs.filter((l) => lc(l.address) === lc(f.address) && l.blockNumber >= from && l.blockNumber <= to &&
              topics.every((t, i) => t === null || lc(t) === lc(l.topics[i] ?? ""))).map((l, i) => ({
              address: l.address, topics: l.topics, data: l.data, blockNumber: hexn(l.blockNumber), transactionHash: keccak256(`0x${i.toString(16).padStart(64, "0")}`),
              transactionIndex: "0x0", blockHash: keccak256(`0x${l.blockNumber.toString(16).padStart(64, "0")}`), logIndex: hexn(i), removed: false,
            }));
          }
          default:
            throw new Error(`fake rh: ${method} not implemented`);
        }
      },
    });
  }
}
