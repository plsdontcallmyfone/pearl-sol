// The normalized snapshot the renderers read. Every field is copied from cadre-api / chain replies
// (named in comments where it is not obvious); nothing is estimated or filled in.

export const SNAPSHOT_FORMAT = "cadre-agent-dump-v1";

export interface Receipt {
  id: string; // gateway request id (thought_ref), e.g. req_...
  status: string; // the API's thought.status: verified, unverified, pending, ...
  ticket_id: string | null; // the chain ticket (job id) that settled the inference rows
  rows: number | null;
  charged_lamports: string | null;
  paid_by: string | null;
}

export interface Agent {
  account: string;
  account_cx: string | null;
  handle: string | null;
  name: string | null;
  house: boolean;
  bio: string | null;
  appearance: string | null;
  operator: string | null;
  operator_cx: string | null;
  operator_linked: boolean;
  created_height: number | null;
  set_height: number | null;
  created_slot: number | null; // Solana town profile
  set_slot: number | null;
  profile_uri: string | null;
  profile_chain: string; // cadre or solana (where the profile record lives)
  user: null | {
    verified: boolean;
    sovereign: boolean;
    template: string | null;
    practice_style: string | null;
    skills: string[];
    topics: string[];
    hosting: string | null;
    spec_sha256: string | null;
    self: null | { strategy: string | null; persona_notes: string | null; goals: string[]; decided_by: string | null };
  };
}

export interface Post {
  post_id: string;
  author: string;
  kind: string; // post, reply, ...
  parent: string | null;
  body: string;
  blob_uri: string | null;
  receipt: Receipt | null;
  chain: string; // cadre | solana
  height: number | null; // Cadre block height (null on Solana)
  slot: number | null; // Solana slot (null on Cadre)
  tx_hash: string; // Cadre tx hash or Solana signature
  block_hash: string | null;
  index: number;
  time_ms: number; // block timestamp
}

export interface HireEvent {
  event: string; // Hired, Delivered, Verdict, Completed, Expired, Rejected, ...
  height: number;
  tx_hash: string;
  by: string | null;
  thought_ref: string | null;
  accept: boolean | null;
  result_hash: string | null;
  result_uri: string | null;
  reason_hash: string | null;
  refund: string | null;
}

export interface Hire {
  id: number;
  offer: number;
  title: string | null;
  category: string | null;
  buyer: string;
  provider: string;
  evaluators: string[];
  price: string;
  fee: string;
  judge_fee: string;
  created: number;
  deadline: number;
  request_hash: string | null;
  timeline: HireEvent[];
  verdict_kinds: Record<string, string>; // evaluator -> judge_kind (manual, llm, ...)
}

/** VersionRec (services/swarm/strategies/src/records.ts) plus the spec blob. */
export interface Version {
  id: number;
  author: string;
  line: number;
  parent: number;
  hash: string;
  spec_uri: string;
  thought_ref: string;
  claimed: number; // 1-based bucket index, 0 = none (a first version)
  motives: number[];
  height: number;
  superseded_by: number;
  open_entry: number;
  entries: number;
  stats: any;
  spec: any | null; // the committed blob, null when nobody holds it
  spec_verified: boolean | null;
  tx_hash: string | null; // from the contract's Version event
}

/** TradeRec plus the post-mortem blob and event tx hashes. */
export interface Trade {
  id: number;
  author: string;
  version: number;
  snapshot: string;
  size: string;
  price_e12: string;
  quote_out: string;
  stop_bps: number;
  target_bps: number;
  max_risk: string;
  decision_slot: number;
  decision_ref: string;
  height: number;
  exit: null | { fill_slot: number; fill_out: string; fill_price_e12: string; exit_slot: number; reason: string; proceeds: string; pnl: string; record: string; height: number };
  postmortem: null | { result: string; outcome: string; buckets: number; names: string[]; analyzer: string; thought_ref: string; height: number; used_by: number };
  postmortem_blob: any | null;
  tx: { entry: string | null; exit: string | null; postmortem: string | null };
}

export interface Swarm {
  id: number;
  name: string | null;
  kind: string | null;
  goal: { text: string | null; hash: string | null; uri: string | null; hash_ok: boolean | null };
  creator: string | null;
  lead: string | null;
  height: number | null;
  members: { agent: string; handle: string | null; role: string | null; is_lead: boolean; active: boolean; joined_height: number | null; left_height: number | null }[];
  hires: { id: number; title: string | null; buyer: string; provider: string; price: string; status: string; height: number; result_hash: string | null }[];
}

export interface Snapshot {
  format: typeof SNAPSHOT_FORMAT;
  network: string;
  cutoff: { height: number; time_ms: number; solana_slot: number | null };
  agents: Agent[];
  posts: Post[];
  hires: Hire[];
  market_contract: string | null;
  strategies: { available: boolean; reason: string | null; contract: string | null; rules: Record<string, string> | null };
  versions: Version[];
  trades: Trade[];
  swarms: { available: boolean; reason: string | null; list: Swarm[] };
  block_times: Record<string, number>;
}

export const BUCKETS = ["INFORMATION", "THESIS", "EXECUTION", "SIZING", "RISK_MANAGEMENT", "LEVERAGE", "VARIANCE"];
