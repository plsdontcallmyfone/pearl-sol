// Relayer configuration from environment variables (all optional on localnet).
//
//   TOKEN_RPC        Solana (token chain) RPC                 default http://127.0.0.1:10600
//   NODE_RPC         agent-chain node JSON-RPC (docs/RPC.md)  default http://127.0.0.1:10630
//   SIGNER_KEYS      comma-separated signer keypair files: used where they are members of the
//                    chain's attester set (BridgeUnlock), the bridge signer set (release) or the
//                    storefront attester set (finalize_order)  default keys/localnet/payer-keypair.json
//   TOKEN_PAYER      Solana fee payer                         default keys/localnet/payer-keypair.json
//   AGENT_KEY        agent-chain account sending BridgeUnlock (fee 0)  default keys/localnet/payer-keypair.json
//   STOREFRONT_PROGRAM  storefront program id                default DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr
//   STATE_FILE       persisted cursors + in-flight state       default services/relayer/.state/relayer.json
//   POLL_MS          tick interval                            default 2000
//   HEALTH_PORT      health endpoint                          default 10660
//   COMMITMENT       Solana commitment                        default confirmed (use finalized outside localnet)
//   TOTAL_SUPPLY     agent-chain total supply (chain units)   default 1e18
//   ALLOW_SURPLUS    0 to also refuse on a surplus (over-backed vault)  default 1; a deficit always refuses
//
// Finality depth K is read from the node (getChainInfo.finality_k), not configured here.

import { mkdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Keypair, PublicKey } from "@solana/web3.js";
import type { Commitment } from "@solana/web3.js";
import { STOREFRONT_PROGRAM_ID, TOTAL_SUPPLY } from "./codec.ts";

export const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const LOCAL_PAYER = resolve(REPO_ROOT, "keys/localnet/payer-keypair.json");

export function loadKeypair(path: string): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(resolve(path), "utf8"))));
}

export interface Config {
  tokenRpc: string;
  nodeRpc: string;
  signerKeyFiles: string[];
  tokenPayerFile: string;
  agentKeyFile: string;
  storefrontProgram: PublicKey;
  stateFile: string;
  pollMs: number;
  healthPort: number;
  commitment: Commitment;
  totalSupply: bigint;
  allowSurplus: boolean;
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const stateFile = resolve(env.STATE_FILE ?? resolve(REPO_ROOT, "services/relayer/.state/relayer.json"));
  mkdirSync(dirname(stateFile), { recursive: true });
  return {
    tokenRpc: env.TOKEN_RPC ?? "http://127.0.0.1:10600",
    nodeRpc: env.NODE_RPC ?? "http://127.0.0.1:10630",
    signerKeyFiles: (env.SIGNER_KEYS ?? LOCAL_PAYER).split(",").map((s) => s.trim()).filter(Boolean),
    tokenPayerFile: env.TOKEN_PAYER ?? LOCAL_PAYER,
    agentKeyFile: env.AGENT_KEY ?? LOCAL_PAYER,
    storefrontProgram: new PublicKey(env.STOREFRONT_PROGRAM ?? STOREFRONT_PROGRAM_ID.toBase58()),
    stateFile,
    pollMs: Number(env.POLL_MS ?? 2000),
    healthPort: Number(env.HEALTH_PORT ?? 10660),
    commitment: (env.COMMITMENT ?? "confirmed") as Commitment,
    totalSupply: BigInt(env.TOTAL_SUPPLY ?? TOTAL_SUPPLY),
    allowSurplus: env.ALLOW_SURPLUS !== "0",
  };
}
