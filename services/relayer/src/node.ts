// Agent-chain node JSON-RPC client (docs/RPC.md). Only the methods the relayer
// uses. Amounts arrive as decimal strings and become bigint here.
//
// `transport` is swappable: HTTP by default, an in-process function in tests
// (test/fake-node.ts), so no test needs a port.

export type Transport = (method: string, params: Record<string, unknown>) => Promise<unknown>;

export class RpcError extends Error {
  readonly code: number;
  constructor(code: number, message: string) {
    super(message);
    this.code = code;
  }
}

export function httpTransport(url: string, timeoutMs = 10_000): Transport {
  let id = 0;
  return async (method, params) => {
    const res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }),
      signal: AbortSignal.timeout(timeoutMs),
    });
    if (!res.ok) throw new Error(`node rpc http ${res.status} on ${method}`);
    const j = (await res.json()) as { result?: unknown; error?: { code: number; message: string } };
    if (j.error) throw new RpcError(j.error.code, `node rpc ${method}: ${j.error.message}`);
    return j.result;
  };
}

export interface ChainInfo {
  chainId: number;
  height: number;
  tip: string;
  finalizedHeight: number;
  finalityK: number;
}

export interface NodeParams {
  attesters: string[];
  attesterThreshold: number;
  finalityK: number;
  bridgePaused: boolean;
}

export interface EscrowInfo {
  escrow: string;
  escrowBalance: bigint;
  totalSupply: bigint;
  height: number;
}

export interface Withdrawal {
  nonce: bigint;
  from: string;
  solanaRecipient: string;
  amountSpl: bigint;
  height: number;
  blockHash: string;
  confirmations: number;
  final: boolean;
}

export interface DepositStatus {
  processed: boolean;
  height?: number;
  recipient?: string;
  amountSpl?: bigint;
  confirmations?: number;
  final?: boolean;
}

export interface Job {
  jobId: string;
  poster: string;
  status: "open" | "delivered" | "completed" | "expired";
  deliverer: string | null;
  resultHash: string | null;
  resultUri: string;
  closedHeight: number;
}

export interface TxStatus {
  status: "pending" | "included";
  height?: number;
  confirmations?: number;
}

export interface NodeIO {
  getChainInfo(): Promise<ChainInfo>;
  getParams(): Promise<NodeParams>;
  getEscrow(): Promise<EscrowInfo>;
  getBridgeEvents(fromNonce: bigint, limit: number): Promise<{ withdrawalCount: bigint; depositCount: bigint; withdrawals: Withdrawal[] }>;
  getDeposit(solanaNonce: bigint): Promise<DepositStatus>;
  getAccount(account: string): Promise<{ balance: bigint; nonce: bigint }>;
  getJob(jobIdHex: string): Promise<Job | null>;
  getTx(txHash: string): Promise<TxStatus | null>;
  sendTx(tx: Buffer): Promise<string>;
}

type J = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

const big = (v: unknown): bigint => BigInt(v as string | number);

export class NodeRpc implements NodeIO {
  readonly call: Transport;
  constructor(call: Transport) {
    this.call = call;
  }

  static http(url: string): NodeRpc {
    return new NodeRpc(httpTransport(url));
  }

  private async req(method: string, params: Record<string, unknown> = {}): Promise<J> {
    return (await this.call(method, params)) as J;
  }

  async getChainInfo(): Promise<ChainInfo> {
    const r = await this.req("getChainInfo");
    return { chainId: r.chain_id, height: r.height, tip: r.tip, finalizedHeight: r.finalized_height, finalityK: r.finality_k };
  }

  async getParams(): Promise<NodeParams> {
    const r = await this.req("getParams");
    return { attesters: r.attesters ?? [], attesterThreshold: r.attester_threshold, finalityK: r.finality_k, bridgePaused: !!r.bridge_paused };
  }

  async getEscrow(): Promise<EscrowInfo> {
    const r = await this.req("getEscrow");
    return { escrow: r.escrow, escrowBalance: big(r.escrow_balance), totalSupply: big(r.total_supply), height: r.height };
  }

  async getBridgeEvents(fromNonce: bigint, limit: number) {
    const r = await this.req("getBridgeEvents", { from_nonce: Number(fromNonce), limit });
    return {
      withdrawalCount: big(r.withdrawal_count),
      depositCount: big(r.deposit_count),
      withdrawals: (r.withdrawals as J[]).map((w): Withdrawal => ({
        nonce: big(w.nonce),
        from: w.from,
        solanaRecipient: w.solana_recipient,
        amountSpl: big(w.amount_spl),
        height: w.height,
        blockHash: w.block_hash,
        confirmations: w.confirmations,
        final: w.final === true,
      })),
    };
  }

  async getDeposit(solanaNonce: bigint): Promise<DepositStatus> {
    const r = await this.req("getDeposit", { solana_nonce: Number(solanaNonce) });
    if (!r.processed) return { processed: false };
    return { processed: true, height: r.height, recipient: r.recipient, amountSpl: big(r.amount_spl), confirmations: r.confirmations, final: r.final === true };
  }

  async getAccount(account: string) {
    const r = await this.req("getAccount", { account });
    return { balance: big(r.balance), nonce: big(r.nonce) };
  }

  async getJob(jobIdHex: string): Promise<Job | null> {
    const r = (await this.call("getJob", { job_id: jobIdHex })) as J | null;
    if (!r) return null;
    return {
      jobId: r.job_id, poster: r.poster, status: r.status, deliverer: r.deliverer ?? null, resultHash: r.result_hash ?? null,
      resultUri: r.result_uri ?? "", closedHeight: r.closed_height ?? 0,
    };
  }

  async getTx(txHash: string): Promise<TxStatus | null> {
    const r = (await this.call("getTx", { tx_hash: txHash })) as J | null;
    return r ? { status: r.status, height: r.height, confirmations: r.confirmations } : null;
  }

  async sendTx(tx: Buffer): Promise<string> {
    const r = await this.req("sendTx", { tx: tx.toString("base64") });
    return r.tx_hash;
  }
}
