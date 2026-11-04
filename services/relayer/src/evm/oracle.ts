// Header-oracle poster (docs/RH-TESTNET-PLAN.md 4.3 "header oracle (phase 1)", decision E9):
// posts attester-signed FINAL Cadre headers to CadreHeaderOracle (token-evm/FORMATS.md 5)
// every `every` final blocks (default 10), and at once when a final domain withdrawal is
// waiting above the oracle's latest header (so proof-mode releases never wait a full period).
// One post covers every earlier withdrawal: the log is append-only and the proof is made
// against the posted header's withdraw_root (getWithdrawalProof at_height).
//
// Before signing it checks the node's header: canonical and final, blake3(header) == hash,
// the height and roots read at the SPEC 3.1 offsets match header_fields. Two different
// hashes at one height are refused by the contract; the poster never signs a height the
// oracle already holds.

import { blake3 } from "@noble/hashes/blake3";
import { encodeFunctionData } from "viem";
import type { Address, Hex } from "viem";
import { oracleAbi } from "./abi.ts";
import type { EvmIO } from "./io.ts";
import type { NodeV2 } from "./node-v2.ts";
import { finalHeaderDigest, pickSigners, signBundle } from "./sign.ts";
import type { EvmKey, FinalHeaderFields } from "./sign.ts";

export interface OraclePosterOptions {
  node: NodeV2;
  evm: EvmIO;
  oracle: Address;
  attesters: EvmKey[];
  /** Post every N final blocks (E9). */
  every?: number;
  /** Bridge domain whose waiting withdrawals trigger an early post; null = cadence only. */
  domain?: number | null;
  /** The bridge, to skip withdrawals already released. */
  isProcessed?: (nonce: bigint) => Promise<boolean>;
  log?: (msg: string, extra?: Record<string, unknown>) => void;
  now?: () => number;
}

export interface OracleTick {
  at: string;
  finalHeight: number | null;
  oracleHeight: string | null;
  posted: { height: number; hash: string; tx: string; reason: string } | null;
  waitingWithdrawals: string[];
  errors: string[];
}

const OFF_HEIGHT = 4;
const OFF_TX_ROOT = 120;
const OFF_STATE_ROOT = 152;
const OFF_WITHDRAW_ROOT = 184;

/** Height and roots at the oracle's offsets of a 256-byte header. */
export function parseHeader(header: Buffer): { height: bigint; txRoot: Hex; stateRoot: Hex; withdrawRoot: Hex } {
  if (header.length !== 256) throw new Error(`header is ${header.length} bytes, want 256`);
  const r = (o: number): Hex => `0x${header.subarray(o, o + 32).toString("hex")}`;
  return { height: header.readBigUInt64LE(OFF_HEIGHT), txRoot: r(OFF_TX_ROOT), stateRoot: r(OFF_STATE_ROOT), withdrawRoot: r(OFF_WITHDRAW_ROOT) };
}

export class OraclePoster {
  readonly o: Required<Omit<OraclePosterOptions, "isProcessed">> & { isProcessed: OraclePosterOptions["isProcessed"] };
  last: OracleTick | null = null;
  lastOkAt: number | null = null;
  ticks = 0;
  posts = 0;

  constructor(opts: OraclePosterOptions) {
    this.o = {
      every: 10,
      domain: 2,
      log: (m, e) => console.log(JSON.stringify({ t: new Date().toISOString(), svc: "rh-oracle", msg: m, ...e })),
      now: () => Date.now(),
      ...opts,
    } as OraclePoster["o"];
  }

  async tick(): Promise<OracleTick> {
    const res: OracleTick = { at: new Date(this.o.now()).toISOString(), finalHeight: null, oracleHeight: null, posted: null, waitingWithdrawals: [], errors: [] };
    this.ticks++;
    try {
      await this.run(res);
    } catch (e) {
      res.errors.push(String((e as { shortMessage?: string })?.shortMessage ?? (e as Error)?.message ?? e).slice(0, 500));
    }
    this.last = res;
    if (res.errors.length === 0) this.lastOkAt = this.o.now();
    return res;
  }

  private async run(res: OracleTick): Promise<void> {
    const { node, evm } = this.o;
    const info = await node.getChainInfo();
    const F = info.finalizedHeight;
    res.finalHeight = F;
    const orc = await evm.oracle("latest");
    res.oracleHeight = orc.latestHeight.toString();
    const empty = /^0x0+$/.test(orc.latestHash);
    if (F <= 0) return;
    if (!empty && BigInt(F) <= orc.latestHeight) return;

    let reason: string | null = null;
    if (empty) reason = "first header";
    else if (BigInt(F) >= orc.latestHeight + BigInt(this.o.every)) reason = `every ${this.o.every}`;
    if (this.o.domain !== null) {
      // withdrawals final on the chain but above the oracle's header: post now (E9 on demand)
      const from = await this.waitingFrom(orc.latestHeight, empty);
      for (const w of from) res.waitingWithdrawals.push(w);
      if (!reason && from.length) reason = `withdrawal ${from[0]} waiting`;
    }
    if (!reason) return;

    const keys = pickSigners(this.o.attesters, orc.signers, orc.threshold);
    if (!keys) throw new Error(`hold fewer than ${orc.threshold} header-oracle attester keys`);
    const h = await node.header(F);
    if (!h) throw new Error(`node has no header at final height ${F}`);
    if (!h.canonical || !h.final) throw new Error(`header at ${F} is not canonical and final on the node`);
    const raw = Buffer.from(h.header, "hex");
    const hash = Buffer.from(blake3(raw)).toString("hex");
    if (hash !== h.hash) throw new Error(`blake3(header ${F}) = ${hash}, node says ${h.hash}`);
    const p = parseHeader(raw);
    const f = h.fields;
    if (p.height !== BigInt(F) || (f.height !== undefined && BigInt(f.height) !== p.height)) throw new Error(`header height ${p.height} != ${F}`);
    for (const [name, v] of [["tx_root", p.txRoot], ["state_root", p.stateRoot], ["withdraw_root", p.withdrawRoot]] as const) {
      if (f[name] !== undefined && `0x${f[name]}` !== v) throw new Error(`header ${F} ${name} differs from header_fields`);
    }
    const existing = await evm.oracleHashAt(p.height);
    if (!/^0x0+$/.test(existing)) {
      if (existing.toLowerCase() !== `0x${hash}`) throw new Error(`oracle holds ${existing} at ${F}, node says ${hash}: refusing`);
      return;
    }
    const fields: FinalHeaderFields = { blockHash: `0x${hash}`, height: p.height, txRoot: p.txRoot, stateRoot: p.stateRoot, withdrawRoot: p.withdrawRoot };
    const sigs = await signBundle(keys, finalHeaderDigest(await evm.chainId(), this.o.oracle, orc.cadreChainId, fields));
    const data = encodeFunctionData({ abi: oracleAbi, functionName: "postFinalHeader", args: [`0x${h.header}`, fields.blockHash, sigs] });
    const tx = await evm.send(this.o.oracle, data);
    this.posts++;
    res.posted = { height: F, hash, tx: tx.hash, reason };
    this.o.log("final header posted", { height: F, hash, reason, tx: tx.hash });
  }

  /** Final, unreleased withdrawals of the domain whose block is above the oracle's header. */
  private async waitingFrom(oracleHeight: bigint, empty: boolean): Promise<string[]> {
    const out: string[] = [];
    const d = this.o.domain as number;
    const first = await this.o.node.bridgeEventsV2(d, 0n, 0);
    const count = first.withdrawalCount;
    // newest first, stop at the first one the oracle already covers
    for (let n = count - 1n; n >= 0n && out.length < 50; n--) {
      const ev = await this.o.node.bridgeEventsV2(d, n, 1);
      const w = ev.withdrawals[0];
      if (!w) continue;
      if (!empty && BigInt(w.height) <= oracleHeight) break;
      if (!w.final) continue;
      if (this.o.isProcessed && (await this.o.isProcessed(w.nonce))) continue;
      out.push(w.nonce.toString());
    }
    return out.reverse();
  }

  health(pollMs: number): Record<string, unknown> {
    const fresh = this.lastOkAt !== null && this.o.now() - this.lastOkAt < Math.max(3 * pollMs, 15_000);
    return { ok: fresh, svc: "rh-oracle", oracle: this.o.oracle, every: this.o.every, ticks: this.ticks, posts: this.posts,
      lastOkAt: this.lastOkAt ? new Date(this.lastOkAt).toISOString() : null, last: this.last };
  }
}
