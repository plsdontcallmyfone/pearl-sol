// Cadre chain client for trading: contract ABI (docs/CONTRACTS.md 4), contract tx kinds 30-32
// (DeployCode, Instantiate, Call) on the SPEC 2 envelope, the contract fee floor (CONTRACTS 5),
// simulate-then-send with gas + 25%, nonce tracking for back-to-back txs, inclusion waits, and
// event reads. Social txs (receipt posts) go through the swarm's own encoder (../../src/chaintx.ts).
import { b58decode, b58encode } from "../../src/b58.ts";
import { blake3 } from "../../src/blake3.ts";
import { feeFor, frame, NS_TX, signTx, type Body } from "../../src/chaintx.ts";
import { NodeRpc } from "../../src/clients.ts";
import type { Wallet } from "../../src/wallet.ts";

// ------------------------------------------------------------------ ABI

export class Abi {
  parts: number[] = [];
  static method(name: string) { return new Abi().str8(name); }
  private str8(s: string) { const b = Buffer.from(s, "utf8"); this.parts.push(b.length, ...b); return this; }
  u8(v: number) { this.parts.push(v & 0xff); return this; }
  u16(v: number) { this.parts.push(v & 0xff, (v >> 8) & 0xff); return this; }
  u32(v: number) { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); this.parts.push(...b); return this; }
  u64(v: bigint | number) { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(v)); this.parts.push(...b); return this; }
  bool(v: boolean) { return this.u8(v ? 1 : 0); }
  addr(a: Uint8Array | string) { const k = typeof a === "string" ? b58decode(a) : a; if (k.length !== 32) throw new Error("address must be 32 bytes"); this.parts.push(...k); return this; }
  hash(h: Uint8Array | string) { const k = typeof h === "string" ? Buffer.from(h, "hex") : h; if (k.length !== 32) throw new Error("hash must be 32 bytes"); this.parts.push(...k); return this; }
  bytes(b: Uint8Array) { this.u32(b.length); this.parts.push(...b); return this; }
  str(s: string) { return this.bytes(Buffer.from(s, "utf8")); }
  out() { return Uint8Array.from(this.parts); }
  hex() { return Buffer.from(this.out()).toString("hex"); }
}

export class Rd {
  o = 0;
  b: Buffer;
  constructor(b: Uint8Array | string) { this.b = typeof b === "string" ? Buffer.from(b, "hex") : Buffer.from(b); }
  u8() { return this.b[this.o++]; }
  u16() { const v = this.b.readUInt16LE(this.o); this.o += 2; return v; }
  u64() { const v = this.b.readBigUInt64LE(this.o); this.o += 8; return v; }
  i64() { const v = this.b.readBigInt64LE(this.o); this.o += 8; return v; }
  bool() { return this.u8() === 1; }
  addr() { const v = b58encode(this.b.subarray(this.o, this.o + 32)); this.o += 32; return v; }
  hash() { const v = this.b.subarray(this.o, this.o + 32).toString("hex"); this.o += 32; return v; }
}

export const eventTopic = (name: string) => Buffer.from(blake3(Buffer.from("pearl-sol:event:v1"), Buffer.from(name))).toString("hex");
export const codeHash = (code: Uint8Array) => Buffer.from(blake3(code)).toString("hex");
export function contractAddress(creator: Uint8Array, nonce: bigint): string {
  const n = Buffer.alloc(8);
  n.writeBigUInt64LE(nonce);
  return b58encode(blake3(Buffer.from("pearl-sol:contract:v1"), creator, n));
}

// ------------------------------------------------------------------ contract tx kinds

export type ContractBody =
  | { kind: "DeployCode"; gasLimit: bigint; code: Uint8Array }
  | { kind: "Instantiate"; codeHash: string; admin: Uint8Array; value: bigint; gasLimit: bigint; label: string; args: Uint8Array }
  | { kind: "Call"; contract: string; value: bigint; gasLimit: bigint; args: Uint8Array };

const CKIND = { DeployCode: 30, Instantiate: 31, Call: 32 } as const;

export function encodeContractBody(b: ContractBody): { kind: number; bytes: Uint8Array } {
  const w = new Abi();
  switch (b.kind) {
    case "DeployCode": w.u64(b.gasLimit).bytes(b.code); break;
    case "Instantiate": {
      const label = Buffer.from(b.label, "utf8");
      if (label.length > 64) throw new Error("label over 64 bytes");
      w.hash(b.codeHash).addr(b.admin).u64(b.value).u64(b.gasLimit).u8(label.length);
      w.parts.push(...label);
      w.u16(b.args.length); w.parts.push(...b.args);
      break;
    }
    case "Call": w.addr(b.contract).u64(b.value).u64(b.gasLimit).u16(b.args.length); w.parts.push(...b.args); break;
  }
  return { kind: CKIND[b.kind], bytes: w.out() };
}

export function contractSigningBytes(chainId: number, from: Uint8Array, nonce: bigint, fee: bigint, b: ContractBody): Uint8Array {
  const { kind, bytes } = encodeContractBody(b);
  return Uint8Array.from([...new Abi().u8(1).u32(chainId).addr(from).u64(nonce).u64(fee).u8(kind).out(), ...bytes]);
}

export function signContractTx(w: Wallet, chainId: number, nonce: bigint, fee: bigint, b: ContractBody): Uint8Array {
  const sb = contractSigningBytes(chainId, w.publicKey, nonce, fee, b);
  return Uint8Array.from([...sb, ...w.sign(frame(NS_TX, sb))]);
}

export interface Params { min_fee: string; social_min_fee?: string; social_byte_fee?: string; gas_price: string; gas_price_unit?: string; contract_base_fee: string; contract_byte_fee: string; tx_gas_limit: string; [k: string]: any }

/** CONTRACTS 5: max(min_fee, base + byte_fee x len + ceil(gas_limit x gas_price / 1e6)). */
export function contractFee(p: Params, len: number, gasLimit: bigint): bigint {
  const unit = BigInt(p.gas_price_unit ?? 1_000_000);
  const gas = (gasLimit * BigInt(p.gas_price) + unit - 1n) / unit;
  const f = BigInt(p.contract_base_fee) + BigInt(p.contract_byte_fee) * BigInt(len) + gas;
  const min = BigInt(p.min_fee);
  return f > min ? f : min;
}

// ------------------------------------------------------------------ client

export interface Sent { txHash: string; nonce: bigint; fee: bigint; sentAt: number; bytes: number }

export class Cadre {
  rpc: NodeRpc;
  chainId = 0;
  params: Params | null = null;
  nonces = new Map<string, bigint>();
  constructor(url: string) { this.rpc = new NodeRpc(url); }

  async init() {
    const info = await this.rpc.call("getChainInfo");
    this.chainId = info.chain_id;
    this.params = await this.rpc.call("getParams");
    return info;
  }
  async height(): Promise<number> { return (await this.rpc.call("getChainInfo")).height; }

  /** Next nonce: chain nonce, or one past what this process last sent if that is higher (back-to-back txs). */
  async nonceOf(w: Wallet): Promise<bigint> {
    const a = await this.rpc.call("getAccount", { account: w.address });
    const chain = BigInt(a?.nonce ?? 0);
    const mine = this.nonces.get(w.address);
    return mine !== undefined && mine > chain ? mine : chain;
  }
  async balance(addr: string): Promise<bigint> {
    const a = await this.rpc.call("getAccount", { account: addr });
    return BigInt(a?.balance ?? 0);
  }

  private async push(w: Wallet, tx: Uint8Array, nonce: bigint, fee: bigint): Promise<Sent> {
    const sentAt = Date.now();
    const r = await this.rpc.sendTx(tx);
    this.nonces.set(w.address, nonce + 1n);
    return { txHash: r.tx_hash, nonce, fee, sentAt, bytes: tx.length };
  }

  /** A contract tx: simulate a Call first for gas (+25%), then sign and send. */
  async sendContract(w: Wallet, b: ContractBody): Promise<Sent & { simulated?: any }> {
    if (!this.params) await this.init();
    let simulated: any;
    if (b.kind === "Call" && b.gasLimit === 0n) {
      simulated = await this.rpc.call("simulateCall", { from: w.address, address: b.contract, args: Buffer.from(b.args).toString("hex"), value: Number(b.value) });
      if (!simulated.ok) throw new Error(`simulation failed: ${simulated.error ? Buffer.from(simulated.error, "hex").toString("utf8") : "unknown"}`);
      b = { ...b, gasLimit: BigInt(Math.ceil(simulated.gas_used * 1.25)) + 20_000n };
    }
    const nonce = await this.nonceOf(w);
    const len = contractSigningBytes(this.chainId, w.publicKey, nonce, 0n, b).length + 64;
    const fee = contractFee(this.params!, len, b.gasLimit);
    return { ...(await this.push(w, signContractTx(w, this.chainId, nonce, fee, b), nonce, fee)), simulated };
  }

  /** A native or social tx through the swarm's encoder (Transfer, Post, ...). */
  async sendNative(w: Wallet, body: Body): Promise<Sent> {
    if (!this.params) await this.init();
    const nonce = await this.nonceOf(w);
    const fee = feeFor(body, this.params!);
    return this.push(w, signTx(w, this.chainId, nonce, fee, body), nonce, fee);
  }

  async query(contract: string, args: Uint8Array): Promise<Rd> {
    const r = await this.rpc.call("queryContract", { address: contract, args: Buffer.from(args).toString("hex") });
    if (!r.ok) throw new Error(`query failed: ${r.error ? Buffer.from(r.error, "hex").toString("utf8") : "?"}`);
    return new Rd(r.return);
  }

  /** Wait until a tx is in a block; returns its height and (contract kinds) its receipt. */
  async waitIncluded(txHash: string, timeoutMs = 600_000, pollMs = 1_000): Promise<{ height: number; receipt: any; at: number }> {
    const end = Date.now() + timeoutMs;
    for (;;) {
      const t = await this.rpc.call("getTx", { tx_hash: txHash }).catch(() => null);
      if (t?.status === "included") return { height: t.height, receipt: t.receipt ?? null, at: Date.now() };
      if (t === null && Date.now() > end - timeoutMs + 120_000) throw new Error(`tx ${txHash} is gone (dropped from the mempool)`);
      if (Date.now() > end) throw new Error(`tx ${txHash} not included after ${timeoutMs} ms`);
      await new Promise((r) => setTimeout(r, pollMs));
    }
  }

  async events(contract: string, topicName?: string, limit = 50, offset = 0) {
    return this.rpc.call("getContractEvents", { address: contract, limit, offset, ...(topicName ? { topic: eventTopic(topicName) } : {}) });
  }
}

export const errText = (hex: string | null | undefined) => (hex ? Buffer.from(hex, "hex").toString("utf8") : "");
