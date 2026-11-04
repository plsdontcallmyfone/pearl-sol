// RH (Robinhood Chain, EVM) side configuration. One relayer process serves ONE side:
// `RELAYER_SIDE=rh node src/main.ts` runs this side; without RELAYER_SIDE (or `solana`) the
// Solana relayer runs exactly as before (src/config.ts). Its own state file, health port
// and keys; never point it at the Solana relayer's STATE_FILE.
//
// Robinhood Chain:
//   RH_RPC            EVM JSON-RPC                                 required (testnet https://rpc.testnet.chain.robinhood.com)
//   RH_CHAIN_ID       expected eth_chainId; refuses to start on another chain    default 46630
//   RH_BRIDGE         CadreBridge address                          required
//   RH_STOREFRONT     CadreStorefront address (orders: unlock with the job, finalize/expire, refunds)  optional, unset = no orders
//   RH_HEADER_ORACLE  CadreHeaderOracle (proof-mode releases)      default: the bridge's headers()
//   RH_DOMAIN         the bridge's Cadre domain (2..=15)           default 2
//   RH_SIGNER_KEY     secp256k1 key file (one line, 0x + 64 hex): sends every RH tx and pays its gas.
//                     For finalize/expire it must be a storefront attester (or the owner).   required
//   RH_ATTESTER_KEYS  comma-separated secp256k1 key files that sign EIP-712 messages; each set
//                     (bridge signers: Release/Refund; storefront attesters: Result/Expired)
//                     uses only its own members                    default RH_SIGNER_KEY
//   RH_FINALITY       block a deposit must be in before it is unlocked, and the block every
//                     invariant read is taken at: safe | finalized | latest | N (N blocks below latest)
//                     default safe (testnet, plan E8); use finalized on mainnet
//   RH_RELEASE_MODE   auto | signers | proof: auto signs a Release when the bridge has the
//                     signer path on and this process holds `threshold` signer keys, else
//                     releaseProof against the header oracle   default auto
//   RH_STATE_FILE     persisted cursors + in-flight state         default services/relayer/.state/relayer-rh.json
//   RH_REFUND_GRACE_MS  a held-back order deposit is refunded (refundDeposit + markRefunded +
//                     settleShare) this long after it was first held back  default 3600000; -1 = never
//   RH_LOG_RANGE      eth_getLogs block range cap (RH public RPC caps results)   default 5000
//
// Cadre chain (same names as the Solana relayer; per process):
//   NODE_RPC          agent-chain node JSON-RPC                   default http://127.0.0.1:10630
//   SIGNER_KEYS       comma-separated ed25519 keypair files (Solana JSON format): those in the
//                     chain's attester set sign BridgeUnlockV2     required
//   AGENT_KEY         chain account that sends BridgeUnlockV2 (fee 0)  default the first SIGNER_KEYS file
//   TOTAL_SUPPLY      chain total supply (chain units)            default 1e18
//   ALLOW_SURPLUS     0 to refuse a vault surplus too               default 1; a deficit always refuses
//   POLL_MS           tick interval                                default 5000
//   HEALTH_PORT       GET /health                                   default 10666 (10660 is the Solana relayer)
//
// Finality depth K on the chain is read from the node (getChainInfo.finality_k).

import { mkdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import type { Address, Hex } from "viem";
import { isAddress, getAddress } from "viem";
import { REPO_ROOT } from "../config.ts";
import { TOTAL_SUPPLY } from "../codec.ts";
import type { Finality } from "./io.ts";
import { evmKey } from "./sign.ts";
import type { EvmKey } from "./sign.ts";

export interface EvmConfig {
  rpc: string;
  chainId: bigint;
  bridge: Address;
  storefront: Address | null;
  oracle: Address | null;
  domain: number;
  signerKeyFile: string;
  attesterKeyFiles: string[];
  finality: Finality;
  releaseMode: "auto" | "signers" | "proof";
  stateFile: string;
  refundGraceMs: number;
  logRange: number;
  nodeRpc: string;
  chainSignerFiles: string[];
  agentKeyFile: string;
  totalSupply: bigint;
  allowSurplus: boolean;
  pollMs: number;
  healthPort: number;
}

function need(env: NodeJS.ProcessEnv, k: string): string {
  const v = env[k]?.trim();
  if (!v) throw new Error(`${k} is required for the RH side`);
  return v;
}

function addr(v: string, name: string): Address {
  if (!isAddress(v, { strict: false })) throw new Error(`${name} is not an address`);
  return getAddress(v);
}

export function parseFinality(v: string | undefined): Finality {
  const s = (v ?? "safe").trim();
  if (s === "safe" || s === "finalized" || s === "latest") return s;
  if (/^\d+$/.test(s)) return Number(s);
  throw new Error(`RH_FINALITY must be safe, finalized, latest or a block count (got ${JSON.stringify(s)})`);
}

/** Reads a secp256k1 key file (0x + 64 hex, or 64 hex). The key is never printed. */
export function loadEvmKey(path: string): EvmKey {
  const t = readFileSync(resolve(path), "utf8").trim();
  const h = (t.startsWith("0x") ? t : `0x${t}`) as Hex;
  if (!/^0x[0-9a-fA-F]{64}$/.test(h)) throw new Error(`${path}: not a secp256k1 key file (0x + 64 hex)`);
  return evmKey(h);
}

const list = (v: string | undefined): string[] => (v ?? "").split(",").map((s) => s.trim()).filter(Boolean);

export function loadEvmConfig(env: NodeJS.ProcessEnv = process.env): EvmConfig {
  const stateFile = resolve(env.RH_STATE_FILE ?? resolve(REPO_ROOT, "services/relayer/.state/relayer-rh.json"));
  if (env.STATE_FILE && resolve(env.STATE_FILE) === stateFile) throw new Error("RH_STATE_FILE must not be the Solana relayer's STATE_FILE");
  mkdirSync(dirname(stateFile), { recursive: true });
  const signerKeyFile = need(env, "RH_SIGNER_KEY");
  const chainSignerFiles = list(need(env, "SIGNER_KEYS"));
  const mode = (env.RH_RELEASE_MODE ?? "auto").trim();
  if (mode !== "auto" && mode !== "signers" && mode !== "proof") throw new Error("RH_RELEASE_MODE must be auto, signers or proof");
  const domain = Number(env.RH_DOMAIN ?? 2);
  if (!Number.isInteger(domain) || domain < 2 || domain > 15) throw new Error("RH_DOMAIN must be 2..=15");
  return {
    rpc: need(env, "RH_RPC"),
    chainId: BigInt(env.RH_CHAIN_ID ?? 46630),
    bridge: addr(need(env, "RH_BRIDGE"), "RH_BRIDGE"),
    storefront: env.RH_STOREFRONT ? addr(env.RH_STOREFRONT, "RH_STOREFRONT") : null,
    oracle: env.RH_HEADER_ORACLE ? addr(env.RH_HEADER_ORACLE, "RH_HEADER_ORACLE") : null,
    domain,
    signerKeyFile,
    attesterKeyFiles: list(env.RH_ATTESTER_KEYS).length ? list(env.RH_ATTESTER_KEYS) : [signerKeyFile],
    finality: parseFinality(env.RH_FINALITY),
    releaseMode: mode,
    stateFile,
    refundGraceMs: Number(env.RH_REFUND_GRACE_MS ?? 3_600_000),
    logRange: Number(env.RH_LOG_RANGE ?? 5000),
    nodeRpc: env.NODE_RPC ?? "http://127.0.0.1:10630",
    chainSignerFiles,
    agentKeyFile: env.AGENT_KEY ?? chainSignerFiles[0],
    totalSupply: BigInt(env.TOTAL_SUPPLY ?? TOTAL_SUPPLY),
    allowSurplus: env.ALLOW_SURPLUS !== "0",
    pollMs: Number(env.POLL_MS ?? 5000),
    healthPort: Number(env.HEALTH_PORT ?? 10666),
  };
}
