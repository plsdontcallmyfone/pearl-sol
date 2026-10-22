// Shared helpers for the Cadre Connect examples (DEVNET ONLY). A fresh throwaway key per example
// lives in .devnet-e2e/connect/keys (gitignored, 0600); it is funded from the devnet ops key the
// same way scripts/devnet-e2e funds its throwaway keys (SOL on Solana devnet; plus CADRE on the
// Cadre chain, which the agent needs for fees). Keys are never printed: only addresses are.
import { existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Cadre, createKeyFile, loadKeyFile, NETWORKS } from "../../packages/cadre-sdk/src/index.ts";

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
export const OUT = join(ROOT, ".devnet-e2e", "connect");
export const API = process.env.CADRE_API ?? NETWORKS.devnet.api;
export const log = (m: string, x?: unknown) => console.log(`[${new Date().toISOString().slice(11, 19)}] ${m}${x === undefined ? "" : " " + JSON.stringify(x, (_k, v) => (typeof v === "bigint" ? v.toString() : v))}`);

/** A throwaway key file (created on first use); returns its path and address. */
export function throwawayKey(name: string): { file: string; address: string } {
  mkdirSync(join(OUT, "keys"), { recursive: true, mode: 0o700 });
  const file = join(OUT, "keys", `${name}.json`);
  const { address } = createKeyFile(file);
  return { file, address };
}

/** The devnet ops key (keys/devnet/ops-keypair.json in the main checkout; OPS_KEY overrides). */
export function opsKeyFile(): string {
  const cands = [process.env.OPS_KEY, join(ROOT, "keys/devnet/ops-keypair.json"), join(process.env.HOME ?? "", "pearl-sol/keys/devnet/ops-keypair.json")].filter(Boolean) as string[];
  const f = cands.find((x) => existsSync(x));
  if (!f) throw new Error("no devnet ops key found (set OPS_KEY)");
  return f;
}

/** Fund `to` from ops: `lamports` SOL on Solana devnet and `units` CADRE on the Cadre chain; skips what is already there. */
export async function fund(to: string, lamports: bigint, units: bigint) {
  const ops = new Cadre({ key: loadKeyFile(opsKeyFile()), api: API, limits: { maxSolPerTx: "unlimited", maxSolPerDay: "unlimited", maxCadrePerTx: "unlimited", maxCadrePerDay: "unlimited" } });
  const bal = await ops.balances(to);
  const out: Record<string, unknown> = { to };
  const haveSol = BigInt(bal.solana?.lamports ?? 0);
  if (haveSol < lamports) {
    const r = await ops.solanaBuilder("/v1/tx/sol-transfer", { from: ops.address, to, lamports: (lamports - haveSol).toString() });
    const c = await ops.waitSolana(r.signature);
    if (!c.ok) throw new Error(`SOL funding failed: ${JSON.stringify(c.err)}`);
    out.sol_tx = r.signature;
  }
  const haveCadre = BigInt(bal.chain?.balance ?? 0);
  if (haveCadre < units) {
    for (let i = 0; ; i++) {
      try {
        const s = await ops.build("/v1/tx/transfer", { to, amount: (units - haveCadre).toString() });
        out.cadre_tx = s.tx_hash;
        await ops.waitTx(s.tx_hash);
        break;
      } catch (e) {
        // the ops key is shared with other sessions: a nonce race is retried
        if (i < 4 && /nonce/i.test((e as Error).message)) { await new Promise((r) => setTimeout(r, 5000)); continue; }
        throw e;
      }
    }
  }
  return out;
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
