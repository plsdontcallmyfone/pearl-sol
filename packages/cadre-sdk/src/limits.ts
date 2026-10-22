// Spending limits enforced BEFORE signing (local-key mode). Per tx and per UTC day, for SOL
// (Solana trades, x402 top-ups, raw Solana txs), CADRE (Cadre fees, tips, hire escrow, DM pay,
// contract value) and inference (lamports paid for prepaid inference). A limit left undefined is
// "not configured": spending of that kind is refused, never unlimited by accident. Pass
// "unlimited" explicitly to lift one. The day's spend is kept in a small JSON file when `ledger`
// is given, so a restart does not reset it.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

export type Amount = bigint | "unlimited";
export interface LimitsConfig {
  maxSolPerTx?: Amount; // lamports
  maxSolPerDay?: Amount;
  maxCadrePerTx?: Amount; // chain base units (9 decimals)
  maxCadrePerDay?: Amount;
  maxInferenceLamportsPerDay?: Amount;
  ledger?: string; // file for the day's totals
}

export class LimitError extends Error {
  code = "limit";
}

type Kind = "sol" | "cadre" | "inference";
const day = () => new Date().toISOString().slice(0, 10);

export class Limits {
  c: LimitsConfig;
  st: { day: string; sol: string; cadre: string; inference: string };

  constructor(c: LimitsConfig = {}) {
    this.c = c;
    this.st = { day: day(), sol: "0", cadre: "0", inference: "0" };
    if (c.ledger && existsSync(c.ledger)) {
      try { const j = JSON.parse(readFileSync(c.ledger, "utf8")); if (j.day === day()) this.st = j; } catch { /* fresh */ }
    }
  }

  static fromEnv(env: Record<string, string | undefined>, prefix = "CADRE_"): LimitsConfig {
    const amt = (k: string): Amount | undefined => {
      const v = env[prefix + k];
      if (v === undefined || v === "") return undefined;
      if (v === "unlimited") return "unlimited";
      if (/^[0-9]+(\.[0-9]+)?$/.test(v) && k.includes("SOL")) return solToLamports(v);
      if (/^[0-9]+(\.[0-9]+)?$/.test(v) && k.includes("CADRE")) return cadreToUnits(v);
      if (/^[0-9]+$/.test(v)) return BigInt(v);
      throw new Error(`${prefix}${k} is a number or "unlimited"`);
    };
    return {
      maxSolPerTx: amt("MAX_SOL_PER_TX"), maxSolPerDay: amt("MAX_SOL_PER_DAY"),
      maxCadrePerTx: amt("MAX_CADRE_PER_TX"), maxCadrePerDay: amt("MAX_CADRE_PER_DAY"),
      maxInferenceLamportsPerDay: env[prefix + "MAX_INFERENCE_SOL_PER_DAY"] ? solToLamports(env[prefix + "MAX_INFERENCE_SOL_PER_DAY"]!) : amt("MAX_INFERENCE_LAMPORTS_PER_DAY"),
      ledger: env[prefix + "LIMITS_LEDGER"],
    };
  }

  private roll() { if (this.st.day !== day()) this.st = { day: day(), sol: "0", cadre: "0", inference: "0" }; }
  private save() {
    if (!this.c.ledger) return;
    mkdirSync(dirname(this.c.ledger), { recursive: true });
    const tmp = `${this.c.ledger}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify(this.st), { mode: 0o600 });
    renameSync(tmp, this.c.ledger);
  }
  private caps(k: Kind): [Amount | undefined, Amount | undefined, string] {
    if (k === "sol") return [this.c.maxSolPerTx, this.c.maxSolPerDay, "SOL (lamports)"];
    if (k === "cadre") return [this.c.maxCadrePerTx, this.c.maxCadrePerDay, "CADRE (base units)"];
    return [undefined, this.c.maxInferenceLamportsPerDay, "inference (lamports)"];
  }

  /** Throws LimitError when `amount` would pass a cap; nothing is booked. */
  check(k: Kind, amount: bigint) {
    this.roll();
    const [perTx, perDay, unit] = this.caps(k);
    if (k !== "inference" && perTx === undefined) throw new LimitError(`no per-tx limit for ${unit} is configured; set it (e.g. CADRE_MAX_${k === "sol" ? "SOL" : "CADRE"}_PER_TX) before spending`);
    if (perDay === undefined) throw new LimitError(`no daily limit for ${unit} is configured; set it before spending`);
    if (perTx !== undefined && perTx !== "unlimited" && amount > perTx) throw new LimitError(`${amount} ${unit} is above the per-tx limit ${perTx}`);
    const used = BigInt(this.st[k]);
    if (perDay !== "unlimited" && used + amount > perDay) throw new LimitError(`${amount} ${unit} would pass today's limit ${perDay} (used ${used})`);
  }
  book(k: Kind, amount: bigint) {
    this.roll();
    this.st[k] = (BigInt(this.st[k]) + amount).toString();
    this.save();
  }
  status() {
    this.roll();
    const s = (v: Amount | undefined) => (v === undefined ? "not set (refused)" : v.toString());
    return { day: this.st.day, used: { sol_lamports: this.st.sol, cadre_units: this.st.cadre, inference_lamports: this.st.inference }, limits: { max_sol_per_tx: s(this.c.maxSolPerTx), max_sol_per_day: s(this.c.maxSolPerDay), max_cadre_per_tx: s(this.c.maxCadrePerTx), max_cadre_per_day: s(this.c.maxCadrePerDay), max_inference_lamports_per_day: s(this.c.maxInferenceLamportsPerDay) } };
  }
}

export function solToLamports(v: string): bigint {
  const [a, b = ""] = v.split(".");
  return BigInt(a) * 1_000_000_000n + BigInt((b + "000000000").slice(0, 9));
}
export function cadreToUnits(v: string): bigint { return solToLamports(v); } // both 9 decimals
