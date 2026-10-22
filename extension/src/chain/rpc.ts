// Agent-chain node JSON-RPC client (docs/RPC.md). Named params, amounts as decimal strings.

export class RpcError extends Error {
  constructor(public code: number, message: string, public url: string) {
    super(message);
  }
}

let nextId = 1;

export async function jsonRpc<T>(url: string, method: string, params: unknown, timeoutMs = 10_000): Promise<T> {
  const ctl = new AbortController();
  const t = setTimeout(() => ctl.abort(), timeoutMs);
  let res: Response;
  try {
    res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: nextId++, method, params }),
      signal: ctl.signal,
    });
  } catch (e) {
    throw new RpcError(-1, `cannot reach ${url} (${(e as Error).name === "AbortError" ? "timeout" : "connection refused"})`, url);
  } finally {
    clearTimeout(t);
  }
  if (!res.ok) throw new RpcError(res.status, `${url} answered HTTP ${res.status}`, url);
  const j = await res.json();
  if (j.error) throw new RpcError(j.error.code ?? -32000, j.error.message ?? "rpc error", url);
  return j.result as T;
}

export interface ChainInfo {
  chain_id: number;
  genesis_hash: string;
  height: number;
  tip: string;
  finalized_height: number;
  finality_k: number;
  target_bits: number;
  mempool_size: number;
  peers: number;
  syncing: boolean;
}
export interface ChainParams {
  admin: string;
  min_fee: string;
  finality_k: number;
  emission_ppm: number;
  alpha_ppm: number;
  attesters: string[];
  attester_threshold: number;
  bridge_paused: boolean;
  max_job_blocks: number;
  min_protocol_fee: string;
  target_block_time_ms: number;
  [k: string]: unknown;
}
export interface ChainAccount {
  account: string;
  balance: string;
  nonce: number;
  /** The account's payout lien (SetRoute), or null (docs/RPC.md getRoute). */
  route?: ChainRoute | null;
}
export interface ChainRoute {
  account: string;
  solana_recipient: string;
  route_bps: number;
  until_height: number;
  cap: string;
  release_key: string | null;
  route_nonce: number;
  set_height: number;
  routed: string;
  pending: string;
  withdrawn: string;
  cleared: boolean;
  active: boolean;
  remaining_cap: string;
}
export interface Agent {
  account: string;
  name: string;
  uri: string;
  registered_height: number;
  updated_height: number;
}
export type JobStatus = "open" | "delivered" | "completed" | "expired";
export interface Job {
  job_id: string;
  poster: string;
  status: JobStatus;
  mineable: boolean;
  m: number;
  n: number;
  k: number;
  config: string;
  hash_a: string;
  hash_b: string;
  data_uri: string;
  worker_pay: string;
  protocol_fee: string;
  deadline_height: number;
  max_blocks: number;
  blocks_mined: number;
  cap_remaining: string;
  subsidy_paid: string;
  evaluators: string[];
  threshold: number;
  terms_hash: string;
  job_key: string;
  job_header: string;
  /** first field of the Pearl header: job_id for a PostJob, the poster for a storefront order */
  header_id: string;
  posted_height: number;
  deliverer: string | null;
  result_hash: string | null;
  result_uri: string;
  closed_height: number;
  index: number;
}
export interface TxInfo {
  tx_hash: string;
  /** "preconfirmed" and "final" from nodes with weak blocks (docs/SPEED.md 2.4). */
  status: "pending" | "preconfirmed" | "included" | "final";
  preconf?: { weak_count: number; weak_miners?: number; weak_share?: number; first_seen_ms?: number; latest_age_ms?: number; confidence?: "low" | "medium" | "high" };
  final?: boolean;
  block_hash?: string;
  height?: number;
  confirmations?: number;
  kind: string;
  from: string;
  nonce: number;
  fee: string;
}
export interface Withdrawal {
  nonce: number;
  from: string;
  solana_recipient: string;
  amount_spl: string;
  amount: string;
  height: number;
  block_hash: string;
  confirmations: number;
  final: boolean;
}
export interface BlockInfo {
  hash: string;
  header_fields: { height: number; miner: string; job_id: string; timestamp_ms: number; [k: string]: unknown };
  txs: { tx_hash: string; kind: string; from: string; nonce: number; fee: string }[];
  canonical: boolean;
  confirmations: number;
  final: boolean;
  /** docs/RPC.md getBlock (2026-10-17); null for blocks a node stored before it recorded them */
  reward?: string | null;
  subsidy?: string | null;
  fees?: string | null;
  matured?: string | null;
}

/** docs/RPC.md "Contracts": getContract (null when unknown). */
export interface ContractInfo {
  address: string;
  code_hash: string;
  admin: string | null;
  upgradable: boolean;
  creator: string;
  label: string;
  created_height: number;
  balance?: string;
  code_size?: number;
  [k: string]: unknown;
}
export interface CodeInfo {
  code_hash: string;
  deployer: string;
  height: number;
  size: number;
  deposit: string;
  instances: number;
  [k: string]: unknown;
}

export class ChainRpc {
  constructor(public url: string) {}
  call<T>(method: string, params: Record<string, unknown> = {}) {
    return jsonRpc<T>(this.url, method, params);
  }
  chainInfo = () => this.call<ChainInfo>("getChainInfo");
  params = () => this.call<ChainParams>("getParams");
  account = (account: string) => this.call<ChainAccount>("getAccount", { account });
  route = (account: string) => this.call<ChainRoute | null>("getRoute", { account });
  agent = (account: string) => this.call<Agent | null>("getAgent", { account });
  job = (job_id: string) => this.call<Job | null>("getJob", { job_id });
  listJobs = (p: { status?: string; offset?: number; limit?: number }) => this.call<{ total: number; jobs: Job[] }>("listJobs", p);
  sendTx = (txB64: string) => this.call<{ tx_hash: string }>("sendTx", { tx: txB64 });
  tx = (tx_hash: string) => this.call<TxInfo | null>("getTx", { tx_hash });
  block = (height: number) => this.call<BlockInfo | null>("getBlock", { height });
  bridgeEvents = (from_nonce = 0, limit = 100) => this.call<{ withdrawal_count: number; deposit_count: number; withdrawals: Withdrawal[] }>("getBridgeEvents", { from_nonce, limit });
  deposit = (solana_nonce: number) => this.call<{ processed: boolean; height?: number; confirmations?: number; final?: boolean; amount_spl?: string; recipient?: string }>("getDeposit", { solana_nonce });
  contract = (address: string) => this.call<ContractInfo | null>("getContract", { address });
  code = (code_hash: string) => this.call<CodeInfo | null>("getCode", { code_hash });
  contractReceipt = (tx_hash: string) => this.call<Record<string, unknown> | null>("getContractReceipt", { tx_hash });
  escrow = () => this.call<{ escrow: string; escrow_balance: string; pool: string; pool_balance: string; total_supply: string; height: number }>("getEscrow");
}
