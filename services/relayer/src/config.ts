// Relayer configuration from environment variables (all optional on localnet).
//
//   TOKEN_RPC        token chain RPC              default http://127.0.0.1:10600
//   AGENT_RPC        agent chain RPC              default http://127.0.0.1:10630
//   SIGNER_KEYS      comma-separated signer keypair files   default keys/localnet/payer-keypair.json
//   TOKEN_PAYER      fee payer on the token chain           default keys/localnet/payer-keypair.json
//   AGENT_PAYER      fee payer on the agent chain           default keys/localnet/payer-keypair.json
//   CURSOR_FILE      persisted cursors            default services/relayer/.state/cursor.json
//   POLL_MS          tick interval                default 2000
//   HEALTH_PORT      health endpoint              default 10660
//   COMMITMENT       processed|confirmed|finalized  default confirmed
//   GENESIS_ESCROW_LAMPORTS  escrow balance at genesis  default 1e18
//   ALLOW_SURPLUS    0 to also refuse on a donated surplus (over-backed)  default 1; a deficit always refuses

import { mkdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Keypair } from "@solana/web3.js";
import type { Commitment } from "@solana/web3.js";
import { GENESIS_ESCROW_LAMPORTS } from "./codec.ts";

export const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const LOCAL_PAYER = resolve(REPO_ROOT, "keys/localnet/payer-keypair.json");

export function loadKeypair(path: string): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(resolve(path), "utf8"))));
}

export interface Config {
  tokenRpc: string;
  agentRpc: string;
  signerKeyFiles: string[];
  tokenPayerFile: string;
  agentPayerFile: string;
  cursorFile: string;
  pollMs: number;
  healthPort: number;
  commitment: Commitment;
  genesisEscrowLamports: bigint;
  allowSurplus: boolean;
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const cursorFile = resolve(env.CURSOR_FILE ?? resolve(REPO_ROOT, "services/relayer/.state/cursor.json"));
  mkdirSync(dirname(cursorFile), { recursive: true });
  return {
    tokenRpc: env.TOKEN_RPC ?? "http://127.0.0.1:10600",
    agentRpc: env.AGENT_RPC ?? "http://127.0.0.1:10630",
    signerKeyFiles: (env.SIGNER_KEYS ?? LOCAL_PAYER).split(",").map((s) => s.trim()).filter(Boolean),
    tokenPayerFile: env.TOKEN_PAYER ?? LOCAL_PAYER,
    agentPayerFile: env.AGENT_PAYER ?? LOCAL_PAYER,
    cursorFile,
    pollMs: Number(env.POLL_MS ?? 2000),
    healthPort: Number(env.HEALTH_PORT ?? 10660),
    commitment: (env.COMMITMENT ?? "confirmed") as Commitment,
    genesisEscrowLamports: BigInt(env.GENESIS_ESCROW_LAMPORTS ?? GENESIS_ESCROW_LAMPORTS),
    allowSurplus: env.ALLOW_SURPLUS !== "0",
  };
}
