// Practice styles: the entry rule sets a practice agent starts from (docs/STRATEGIES.md 6). The
// first three are the house practice agents' sets (bin/practice.ts HOUSE: tape_lark, floor_wren,
// still_heron); contrarian and accumulator were added for user agents (2026-10-22). Every number
// here is a design choice, not a measurement; the agent picks its own stop, target and hold in
// its first paid thought and revises only from post-mortems that name a fixable bucket.
import type { Base } from "./agent.ts";

export const STYLE_SIZE = { equity_fraction_bps: 1_000, max_lamports: 100_000_000, max_pool_share_bps: 20 };
export const STYLE_BASE = { size: STYLE_SIZE, max_risk_bps: 100, max_slippage_bps: 150, practice_balance_lamports: 1_000_000_000 };

export interface Style { id: string; name: string; persona: string; base: Base }

export const STYLES: Style[] = [
  { id: "momentum", name: "Momentum", persona: "a momentum trader who buys strength in deep pools", base: { ...STYLE_BASE, conditions: [{ signal: "pool_sol", op: ">=", value: 60 }, { signal: "ret_5m", op: ">=", value: 0.01 }, { signal: "ret_15m", op: ">=", value: 0.03 }, { signal: "flow_15m", op: ">=", value: 0 }] } },
  { id: "bounce", name: "First bounce", persona: "a trader who buys the first bounce after a fall", base: { ...STYLE_BASE, conditions: [{ signal: "pool_sol", op: ">=", value: 80 }, { signal: "ret_15m", op: "<=", value: -0.05 }, { signal: "ret_5m", op: ">=", value: 0.005 }] } },
  { id: "calm", name: "Calm and liquid", persona: "a trader who only trades calm, liquid pools", base: { ...STYLE_BASE, conditions: [{ signal: "pool_sol", op: ">=", value: 150 }, { signal: "ret_15m", op: ">=", value: 0.01 }, { signal: "vol_15m", op: "<=", value: 0.01 }, { signal: "top10_share", op: "<=", value: 0.3 }] } },
  { id: "contrarian", name: "Contrarian", persona: "a contrarian who buys a hard fall when the coin creator is not selling", base: { ...STYLE_BASE, conditions: [{ signal: "pool_sol", op: ">=", value: 80 }, { signal: "ret_60m", op: "<=", value: -0.1 }, { signal: "dev_sold_15m", op: "<=", value: 0 }, { signal: "top1_share", op: "<=", value: 0.2 }] } },
  { id: "accumulator", name: "Accumulator", persona: "a patient buyer of deep pools with steady net buying", base: { ...STYLE_BASE, conditions: [{ signal: "pool_sol", op: ">=", value: 120 }, { signal: "flow_15m", op: ">=", value: 0.005 }, { signal: "vol_15m", op: "<=", value: 0.02 }] } },
];

export const STYLE_IDS = STYLES.map((s) => s.id);
export const styleById = (id: string) => STYLES.find((s) => s.id === id) ?? null;
