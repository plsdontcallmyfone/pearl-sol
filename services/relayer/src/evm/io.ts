// What the RH relayer and the header-oracle poster read from and send to Robinhood Chain,
// behind one interface so tests can swap the transport. `ViemEvm` is the real one: every
// read takes a block number, so one tick reads the bridge, the vault and the storefront at
// the SAME block (the finality block for deposits and the invariant).

import { createPublicClient, createWalletClient, decodeEventLog, encodeFunctionData, http } from "viem";
import type { Abi, Address, Chain, Hex, PublicClient, Transport, WalletClient } from "viem";
import { bridgeAbi, erc20Abi, oracleAbi, storefrontAbi } from "./abi.ts";
import type { EvmKey } from "./sign.ts";

/** `safe` / `finalized` / `latest` block tag, or a number N = N blocks below latest. */
export type Finality = "safe" | "finalized" | "latest" | number;

export interface BridgeView {
  token: Address;
  domain: number;
  depositNonce: bigint;
  releaseModes: number;
  paused: boolean;
  headers: Address;
  threshold: number;
  signers: Address[];
  minWithdrawalNonce: bigint;
  /** token.balanceOf(bridge) */
  vault: bigint;
}

export interface EvmDeposit {
  nonce: bigint;
  sender: Address;
  amount: bigint;
  refundTo: Address;
  blockNumber: bigint;
  /** 32-byte chain recipient, 0x hex. */
  recipient: Hex;
  refunded: boolean;
}

export interface EvmOrder {
  buyer: Address;
  status: number;
  shareSettled: boolean;
  tokens: bigint;
  workerPay: bigint;
  protocolFee: bigint;
  share: bigint;
  recipient: Hex;
  jobId: Hex;
  bodyHash: Hex;
}

export interface SignerView {
  threshold: number;
  signers: Address[];
}

export interface OracleView extends SignerView {
  cadreChainId: bigint;
  latestHeight: bigint;
  latestHash: Hex;
}

export interface SendResult {
  hash: Hex;
  block: bigint;
}

export interface EvmIO {
  chainId(): Promise<bigint>;
  /** Block number at the configured finality, and latest. */
  blocks(): Promise<{ final: bigint; latest: bigint }>;
  bridge(at: bigint): Promise<BridgeView>;
  deposit(nonce: bigint, at: bigint | "latest"): Promise<EvmDeposit>;
  isProcessed(nonce: bigint, at: bigint | "latest"): Promise<boolean>;
  order(nonce: bigint, at: bigint | "latest"): Promise<EvmOrder>;
  /** The PostJob body the storefront emitted for order deposit `nonce` (OrderBody, in block `block`). */
  orderBody(nonce: bigint, block: bigint): Promise<Hex | null>;
  storefront(at: bigint | "latest"): Promise<SignerView & { owner: Address }>;
  oracle(at: bigint | "latest"): Promise<OracleView>;
  oracleHashAt(height: bigint): Promise<Hex>;
  oracleRoots(hash: Hex): Promise<{ height: bigint; withdrawRoot: Hex; final: boolean }>;
  /** Simulates (estimateGas), sends from the signer key, waits for the receipt; throws on a revert. */
  send(to: Address, data: Hex): Promise<SendResult>;
  readonly sender: Address;
}

export interface ViemEvmOptions {
  rpc?: string;
  /** Overrides the HTTP transport (tests: viem `custom`). */
  transport?: Transport;
  bridge: Address;
  storefront?: Address;
  oracle?: Address;
  signer: EvmKey;
  finality: Finality;
  /** Receipt wait, ms. */
  receiptTimeoutMs?: number;
}

const ZERO = "0x0000000000000000000000000000000000000000" as Address;

export class ViemEvm implements EvmIO {
  readonly pub: PublicClient;
  readonly wallet: WalletClient;
  readonly o: ViemEvmOptions;
  readonly sender: Address;
  private cid: bigint | null = null;

  constructor(o: ViemEvmOptions) {
    this.o = o;
    const transport = o.transport ?? http(o.rpc, { timeout: 20_000, retryCount: 2 });
    this.pub = createPublicClient({ transport }) as PublicClient;
    this.wallet = createWalletClient({ transport, account: o.signer.account });
    this.sender = o.signer.address;
  }

  private read<T>(address: Address, abi: Abi, functionName: string, args: unknown[], at: bigint | "latest"): Promise<T> {
    const p: Record<string, unknown> = { address, abi, functionName, args };
    if (at !== "latest") p.blockNumber = at;
    return this.pub.readContract(p as never) as Promise<T>;
  }

  async chainId(): Promise<bigint> {
    this.cid ??= BigInt(await this.pub.getChainId());
    return this.cid;
  }

  async blocks(): Promise<{ final: bigint; latest: bigint }> {
    const latest = await this.pub.getBlockNumber({ cacheTime: 0 });
    const f = this.o.finality;
    if (f === "latest") return { final: latest, latest };
    if (typeof f === "number") return { final: latest > BigInt(f) ? latest - BigInt(f) : 0n, latest };
    const b = await this.pub.getBlock({ blockTag: f });
    return { final: b.number ?? 0n, latest };
  }

  async bridge(at: bigint): Promise<BridgeView> {
    const b = this.o.bridge;
    const r = <T>(fn: string, args: unknown[] = []) => this.read<T>(b, bridgeAbi, fn, args, at);
    const [token, domain, depositNonce, releaseModes, paused, headers, threshold, signers, minWithdrawalNonce] = await Promise.all([
      r<Address>("token"), r<number>("domain"), r<bigint>("depositNonce"), r<number>("releaseModes"), r<boolean>("paused"),
      r<Address>("headers"), r<number>("threshold"), r<Address[]>("signers"), r<bigint>("minWithdrawalNonce"),
    ]);
    const vault = await this.read<bigint>(token, erc20Abi, "balanceOf", [b], at);
    return { token, domain: Number(domain), depositNonce, releaseModes: Number(releaseModes), paused, headers, threshold: Number(threshold),
      signers: [...signers], minWithdrawalNonce, vault };
  }

  async deposit(nonce: bigint, at: bigint | "latest"): Promise<EvmDeposit> {
    const d = await this.read<{ sender: Address; amount: bigint; refundTo: Address; blockNumber: bigint; recipient: Hex; refunded: boolean }>(
      this.o.bridge, bridgeAbi, "deposits", [nonce], at);
    return { nonce, sender: d.sender, amount: d.amount, refundTo: d.refundTo, blockNumber: d.blockNumber, recipient: d.recipient, refunded: d.refunded };
  }

  isProcessed(nonce: bigint, at: bigint | "latest"): Promise<boolean> {
    return this.read<boolean>(this.o.bridge, bridgeAbi, "isProcessed", [nonce], at);
  }

  private store(): Address {
    if (!this.o.storefront) throw new Error("RH_STOREFRONT not configured");
    return this.o.storefront;
  }

  async order(nonce: bigint, at: bigint | "latest"): Promise<EvmOrder> {
    const o = await this.read<EvmOrder & { ethSpent: bigint }>(this.store(), storefrontAbi, "getOrder", [nonce], at);
    return { buyer: o.buyer, status: Number(o.status), shareSettled: o.shareSettled, tokens: o.tokens, workerPay: o.workerPay,
      protocolFee: o.protocolFee, share: o.share, recipient: o.recipient, jobId: o.jobId, bodyHash: o.bodyHash };
  }

  async orderBody(nonce: bigint, block: bigint): Promise<Hex | null> {
    const logs = (await this.pub.getLogs({
      address: this.store(),
      event: storefrontAbi.find((x) => x.type === "event" && x.name === "OrderBody") as never,
      args: { nonce } as never,
      fromBlock: block,
      toBlock: block,
    })) as unknown as { data: Hex; topics: [Hex, ...Hex[]] }[];
    for (const l of logs) {
      const ev = decodeEventLog({ abi: storefrontAbi, data: l.data, topics: l.topics });
      if (ev.eventName === "OrderBody" && (ev.args as { nonce: bigint }).nonce === nonce) return (ev.args as { body: Hex }).body;
    }
    return null;
  }

  async storefront(at: bigint | "latest") {
    const s = this.store();
    const [threshold, signers, owner] = await Promise.all([
      this.read<number>(s, storefrontAbi, "threshold", [], at), this.read<Address[]>(s, storefrontAbi, "signers", [], at),
      this.read<Address>(s, storefrontAbi, "owner", [], at),
    ]);
    return { threshold: Number(threshold), signers: [...signers], owner };
  }

  private orc(): Address {
    if (!this.o.oracle) throw new Error("RH_HEADER_ORACLE not configured");
    return this.o.oracle;
  }

  async oracle(at: bigint | "latest"): Promise<OracleView> {
    const a = this.orc();
    const [cadreChainId, threshold, signers, latest] = await Promise.all([
      this.read<bigint>(a, oracleAbi, "cadreChainId", [], at), this.read<number>(a, oracleAbi, "threshold", [], at),
      this.read<Address[]>(a, oracleAbi, "signers", [], at), this.read<[bigint, Hex]>(a, oracleAbi, "latestFinal", [], at),
    ]);
    return { cadreChainId, threshold: Number(threshold), signers: [...signers], latestHeight: latest[0], latestHash: latest[1] };
  }

  oracleHashAt(height: bigint): Promise<Hex> {
    return this.read<Hex>(this.orc(), oracleAbi, "hashAtHeight", [height], "latest");
  }

  async oracleRoots(hash: Hex) {
    const [r, f] = await Promise.all([
      this.read<[bigint, Hex, Hex, Hex]>(this.orc(), oracleAbi, "roots", [hash], "latest"),
      this.read<boolean>(this.orc(), oracleAbi, "isFinal", [hash], "latest"),
    ]);
    return { height: r[0], withdrawRoot: r[3], final: f };
  }

  async send(to: Address, data: Hex): Promise<SendResult> {
    const account = this.o.signer.account;
    const gas = await this.pub.estimateGas({ account, to, data });
    const hash = await this.wallet.sendTransaction({ account, to, data, gas: (gas * 12n) / 10n, chain: null as unknown as Chain });
    const rc = await this.pub.waitForTransactionReceipt({ hash, timeout: this.o.receiptTimeoutMs ?? 120_000, pollingInterval: 500 });
    if (rc.status !== "success") throw new Error(`tx ${hash} reverted`);
    return { hash, block: rc.blockNumber };
  }
}

export { encodeFunctionData, ZERO };
