// Cadre chain node RPC for the bridge v2 side (docs/BRIDGE-V2.md 6, docs/RPC.md "Bridge v2").
// Separate from src/node.ts so the Solana relayer's client stays exactly as it is; it
// reuses that file's transports and the methods every side needs.

import { NodeRpc } from "../node.ts";
import type { Transport } from "../node.ts";
import { httpTransport } from "../node.ts";

type J = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
const big = (v: unknown): bigint => BigInt(v as string | number);

export interface DomainCfg {
  domain: number;
  extChainId: bigint;
  /** 0x + lowercase hex (20 bytes for an EVM side). */
  bridgeAddr: string;
  evm: boolean;
  paused: boolean;
}

export interface BridgeV2Params {
  active: boolean;
  activationHeight: number | null;
  domains: DomainCfg[];
}

export interface WithdrawalV2Ev {
  nonce: bigint;
  from: string;
  /** 32-byte recipient, hex. */
  recipient: string;
  /** 0x address when the recipient is a padded EVM address, else null. */
  recipientEvm: string | null;
  amountExt: bigint;
  height: number;
  blockHash: string | null;
  confirmations: number;
  /** The node holds an MMR location for it under a final root. */
  final: boolean;
  /** The 90-byte v2 log entry, hex. */
  entry: string;
}

export interface DepositV2Status {
  processed: boolean;
  height?: number;
  recipient?: string;
  amountExt?: bigint;
  confirmations?: number;
  final?: boolean;
}

export interface DomainCounters {
  domain: number;
  unlocked: bigint;
  locked: bigint;
  /** unlocked - locked, chain units: what the side's vault must back (x 1000). */
  outstanding: bigint;
  depositCount: number;
  withdrawalCount: number;
}

export interface WithdrawalV2Proof {
  final: boolean;
  entry?: string;
  location?: number;
  /** Codec-encoded MMR proof, base64. */
  proof?: string;
  withdrawRoot?: string;
  rootHeight?: number;
  rootBlockHash?: string;
}

export interface HeaderInfo {
  hash: string;
  /** 256 raw header bytes, hex. */
  header: string;
  fields: J;
  canonical: boolean;
  final: boolean;
}

export class NodeV2 extends NodeRpc {
  constructor(call: Transport) {
    super(call);
  }

  static override http(url: string): NodeV2 {
    return new NodeV2(httpTransport(url));
  }

  private async r(method: string, params: Record<string, unknown> = {}): Promise<J> {
    return (await this.call(method, params)) as J;
  }

  async bridgeV2Params(): Promise<BridgeV2Params> {
    const p = await this.r("getParams");
    return {
      active: p.bridge_v2_active === true,
      activationHeight: p.bridge_v2_height ?? null,
      domains: ((p.bridge_domains ?? []) as J[]).map((d) => ({
        domain: d.domain, extChainId: big(d.ext_chain_id), bridgeAddr: String(d.bridge_addr).toLowerCase(), evm: !!d.evm, paused: !!d.paused,
      })),
    };
  }

  async bridgeCounters(domain: number): Promise<{ height: number; escrow: bigint; counters: DomainCounters | null }> {
    const r = await this.r("getBridgeCounters");
    const d = ((r.domains ?? []) as J[]).find((x) => x.domain === domain);
    return {
      height: r.height,
      escrow: big(r.escrow_balance),
      counters: d ? {
        domain, unlocked: big(d.unlocked ?? 0), locked: big(d.locked ?? 0), outstanding: big(d.outstanding),
        depositCount: Number(d.deposit_count ?? 0), withdrawalCount: Number(d.withdrawal_count ?? 0),
      } : null,
    };
  }

  async depositV2(domain: number, nonce: bigint): Promise<DepositV2Status> {
    const r = await this.r("getDeposit", { domain, nonce: Number(nonce) });
    if (!r.processed) return { processed: false };
    return { processed: true, height: r.height, recipient: r.recipient, amountExt: big(r.amount_ext), confirmations: r.confirmations, final: r.final === true };
  }

  async bridgeEventsV2(domain: number, fromNonce: bigint, limit: number): Promise<{ withdrawalCount: bigint; withdrawals: WithdrawalV2Ev[] }> {
    const r = await this.r("getBridgeEvents", { domain, from_nonce: Number(fromNonce), limit });
    return {
      withdrawalCount: big(r.withdrawal_count),
      withdrawals: ((r.withdrawals ?? []) as J[]).map((w) => ({
        nonce: big(w.nonce), from: w.from, recipient: w.recipient, recipientEvm: w.recipient_evm ?? null, amountExt: big(w.amount_ext),
        height: w.height, blockHash: w.block_hash ?? null, confirmations: w.confirmations ?? 0, final: w.final === true, entry: w.entry,
      })),
    };
  }

  async withdrawalProofV2(domain: number, nonce: bigint, atHeight?: number): Promise<WithdrawalV2Proof | null> {
    const params: Record<string, unknown> = { domain, nonce: Number(nonce) };
    if (atHeight !== undefined) params.at_height = atHeight;
    const r = (await this.call("getWithdrawalProof", params)) as J | null;
    if (!r) return null;
    if (!r.final || !r.proof) return { final: false };
    return { final: true, entry: r.entry, location: r.location, proof: r.proof, withdrawRoot: r.withdraw_root, rootHeight: r.root_height, rootBlockHash: r.root_block_hash };
  }

  async header(height: number): Promise<HeaderInfo | null> {
    const r = (await this.call("getHeader", { height })) as J | null;
    if (!r) return null;
    return { hash: r.hash, header: r.header, fields: r.header_fields ?? {}, canonical: r.canonical !== false, final: r.final === true };
  }
}
