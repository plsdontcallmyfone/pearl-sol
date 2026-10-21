// Budget controls (owner decision D1, ECOSYSTEM 2.6): a daily row budget split over the agents
// by weight, a hard per-agent daily row cap, a daily top-up SOL cap and transfer caps. A thought
// is admitted only when its upper-bound row estimate fits the day's cap and the agent's booked rows
// are still under its share (the upper bound runs about 7x the real rows on devnet, so checking it
// against the smaller per-agent share refused almost every thought); the receipt's actual rows
// are booked (an upper bound by construction, see prompt.ts rowsUpperBound). Days are UTC.

export interface BudgetConfig {
  rowsPerDay: number; // SWARM_ROWS_PER_DAY
  agentRowsCap: number; // SWARM_AGENT_ROWS_PER_DAY (hard ceiling per agent)
  topupLamportsPerDay: bigint; // SWARM_TOPUP_LAMPORTS_PER_DAY
  transferUnitsPerDay: bigint; // SWARM_TRANSFER_PER_DAY (chain base units, all agent fundings)
  agentFloat: bigint; // SWARM_AGENT_FLOAT: target fee balance per agent (chain base units)
}

export interface BudgetState {
  day: string;
  rows: number;
  reserved: number;
  agentRows: Record<string, number>;
  topupLamports: string;
  transferUnits: string;
  thoughts: number;
  refused: Record<string, number>;
}

export const dayOf = (ms: number) => new Date(ms).toISOString().slice(0, 10);

export function freshState(now: number): BudgetState {
  return { day: dayOf(now), rows: 0, reserved: 0, agentRows: {}, topupLamports: "0", transferUnits: "0", thoughts: 0, refused: {} };
}

/** Each agent's share of the day's rows: floor(budget x w_i / sum w), at least `floor`. */
export function split(budget: number, weights: Record<string, number>, floor = 0): Record<string, number> {
  const sum = Object.values(weights).reduce((a, b) => a + b, 0);
  const out: Record<string, number> = {};
  for (const [k, w] of Object.entries(weights)) out[k] = Math.max(floor, sum > 0 ? Math.floor((budget * w) / sum) : 0);
  return out;
}

export class Budget {
  cfg: BudgetConfig;
  st: BudgetState;
  shares: Record<string, number>;
  constructor(cfg: BudgetConfig, weights: Record<string, number>, st: BudgetState | undefined, now: number) {
    this.cfg = cfg;
    this.st = st && st.day === dayOf(now) ? st : freshState(now);
    this.st.reserved = 0;
    // an agent may spend up to three times its weighted share (busy agents borrow from quiet
    // ones; reservations are upper bounds, see prompt.ts), never more than the hard per-agent
    // cap; the day's total cap holds regardless
    const base = split(cfg.rowsPerDay, weights, Math.floor(cfg.rowsPerDay / Math.max(1, Object.keys(weights).length) / 4));
    this.shares = Object.fromEntries(Object.entries(base).map(([k, v]) => [k, Math.min(cfg.agentRowsCap, 3 * v)]));
  }
  roll(now: number) {
    if (this.st.day !== dayOf(now)) this.st = freshState(now);
  }
  agentCap(agent: string): number {
    return this.shares[agent] ?? 0;
  }
  /** Reserve `est` rows for `agent`; false (and a refusal count) when any cap would be passed. */
  reserve(agent: string, est: number, now: number): boolean {
    this.roll(now);
    const why = this.st.rows + this.st.reserved + est > this.cfg.rowsPerDay ? "day_rows" : (this.st.agentRows[agent] ?? 0) >= this.agentCap(agent) ? "agent_rows" : null;
    if (why) {
      this.st.refused[why] = (this.st.refused[why] ?? 0) + 1;
      return false;
    }
    this.st.reserved += est;
    return true;
  }
  /** Would `reserve` admit `est` rows for `agent` now? (no side effects; the DM decline check) */
  admits(agent: string, est: number, now: number): boolean {
    if (this.st.day !== dayOf(now)) return est <= this.cfg.rowsPerDay && this.agentCap(agent) > 0;
    return this.st.rows + this.st.reserved + est <= this.cfg.rowsPerDay && (this.st.agentRows[agent] ?? 0) < this.agentCap(agent);
  }
  /** Book the receipt's rows (release the reservation). */
  commit(agent: string, est: number, actual: number) {
    this.st.reserved = Math.max(0, this.st.reserved - est);
    this.st.rows += actual;
    this.st.agentRows[agent] = (this.st.agentRows[agent] ?? 0) + actual;
    this.st.thoughts += 1;
  }
  release(est: number) {
    this.st.reserved = Math.max(0, this.st.reserved - est);
  }
  canTopup(lamports: bigint, now: number): boolean {
    this.roll(now);
    return BigInt(this.st.topupLamports) + lamports <= this.cfg.topupLamportsPerDay;
  }
  bookTopup(lamports: bigint) {
    this.st.topupLamports = (BigInt(this.st.topupLamports) + lamports).toString();
  }
  canTransfer(units: bigint, now: number): boolean {
    this.roll(now);
    return BigInt(this.st.transferUnits) + units <= this.cfg.transferUnitsPerDay;
  }
  bookTransfer(units: bigint) {
    this.st.transferUnits = (BigInt(this.st.transferUnits) + units).toString();
  }
}
