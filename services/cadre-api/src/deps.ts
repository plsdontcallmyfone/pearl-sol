// Shared dependencies for the routes: the explorer indexer (reused, not a second
// implementation), JSON-RPC clients for the agent-chain node and Solana, and the HTTP
// upstreams (inference gateway, storefront-x402, mining pool).

import { Indexer } from "../../../apps/explorer/src/indexer.ts";
import { JsonRpc, SolanaRpc } from "../../../apps/explorer/src/rpc.ts";
import { ApiError } from "./http.ts";
import { bridgeConfigPda, decodeBridgeConfig, type BridgeConfig } from "./sol.ts";

export interface Config {
  port: number;
  host: string;
  nodeRpc: string;
  solanaRpc: string;
  gatewayUrl: string | null;
  storefrontUrl: string | null;
  poolUrl: string | null;
  explorerUrl: string | null;
  stateDir: string;
  pollMs: number;
  allowedOrigins: "*" | string[];
  rateLimitPerMin: number;
  rateBurst: number;
  trustProxy: boolean;
  /** Prepaid balances / top-ups: "off" until the gateway's prepaid API (SPEC 5.4) lands; "gateway" proxies it. */
  prepaid: "off" | "gateway";
  /** Cluster name shown to wallets (solana:<cluster>), e.g. localnet, devnet. */
  solanaCluster: string;
  network: string;
  publicUrl: string | null;
}

export function configFromEnv(env: NodeJS.ProcessEnv, defaults: { stateDir: string }): Config {
  const opt = (k: string, d: string | null) => (env[k] === undefined ? d : env[k] === "" || env[k] === "off" ? null : env[k]!);
  const origins = (env.CADRE_ALLOWED_ORIGINS ?? "*").trim();
  return {
    port: Number(env.CADRE_API_PORT ?? 10685),
    host: env.CADRE_API_HOST ?? "127.0.0.1",
    nodeRpc: env.NODE_RPC ?? "http://127.0.0.1:10630",
    solanaRpc: env.SOLANA_RPC ?? "http://127.0.0.1:10600",
    gatewayUrl: opt("GATEWAY_URL", "http://127.0.0.1:10664"),
    storefrontUrl: opt("STOREFRONT_URL", "http://127.0.0.1:10662"),
    poolUrl: opt("POOL_URL", "http://127.0.0.1:10663"),
    explorerUrl: opt("EXPLORER_PUBLIC_URL", null),
    stateDir: env.STATE_DIR ?? defaults.stateDir,
    pollMs: Number(env.POLL_MS ?? 3000),
    allowedOrigins: origins === "*" ? "*" : origins.split(",").map((s) => s.trim()).filter(Boolean),
    rateLimitPerMin: Number(env.CADRE_RATE_LIMIT_PER_MIN ?? 300),
    rateBurst: Number(env.CADRE_RATE_BURST ?? 120),
    trustProxy: env.CADRE_TRUST_PROXY === "1",
    prepaid: env.CADRE_PREPAID === "gateway" ? "gateway" : "off",
    solanaCluster: env.SOLANA_CLUSTER ?? "localnet",
    network: env.CADRE_NETWORK ?? "localnet",
    publicUrl: opt("CADRE_PUBLIC_URL", null),
  };
}

export class Deps {
  cfg: Config;
  ix: Indexer;
  node: JsonRpc;
  sol: SolanaRpc;
  private bridgeCache: { at: number; v: BridgeConfig | null } | null = null;
  startedAt = Date.now();

  constructor(cfg: Config, ix?: Indexer) {
    this.cfg = cfg;
    this.ix = ix ?? new Indexer({ nodeUrl: cfg.nodeRpc, solUrl: cfg.solanaRpc, stateDir: cfg.stateDir, pollMs: cfg.pollMs, log: (m) => console.log(JSON.stringify({ msg: m })) });
    this.node = new JsonRpc(cfg.nodeRpc, 15_000);
    this.sol = new SolanaRpc(cfg.solanaRpc);
  }

  /** Agent-chain node call; node errors become 502 with the node's message, -32602 style input errors 400. */
  async chain<T = any>(method: string, params: unknown): Promise<T> {
    try {
      return await this.node.call<T>(method, params);
    } catch (e) {
      const m = (e as Error).message;
      if (/fetch failed|ECONNREFUSED|timeout|aborted/i.test(m)) throw new ApiError(503, "node_unavailable", `agent-chain node unreachable: ${m.slice(0, 160)}`);
      if (/invalid|bad param|expected|decode|parse|not a|unknown/i.test(m)) throw new ApiError(400, "node_rejected", m.slice(0, 300));
      throw new ApiError(502, "node_error", m.slice(0, 300));
    }
  }

  /** Solana JSON-RPC call. */
  async solana<T = any>(method: string, params: unknown): Promise<T> {
    try {
      return await this.sol.rpc.call<T>(method, params);
    } catch (e) {
      const m = (e as Error).message;
      if (/fetch failed|ECONNREFUSED|timeout|aborted/i.test(m)) throw new ApiError(503, "solana_unavailable", `Solana RPC unreachable: ${m.slice(0, 160)}`);
      throw new ApiError(502, "solana_error", m.slice(0, 400));
    }
  }

  async accounts(addresses: string[]) {
    try {
      return await this.sol.getMultipleAccounts(addresses);
    } catch (e) {
      throw new ApiError(503, "solana_unavailable", `Solana RPC: ${(e as Error).message.slice(0, 160)}`);
    }
  }

  /** The bridge Config (mint, vault, deposit nonce); cached 2 s. */
  async bridge(): Promise<BridgeConfig> {
    if (this.bridgeCache && Date.now() - this.bridgeCache.at < 2000 && this.bridgeCache.v) return this.bridgeCache.v;
    const [a] = await this.accounts([bridgeConfigPda()]);
    const v = a ? decodeBridgeConfig(a.data) : null;
    this.bridgeCache = { at: Date.now(), v };
    if (!v) throw new ApiError(503, "bridge_not_initialized", "the bridge Config account is missing on this Solana cluster");
    return v;
  }

  /** GET/POST to an HTTP upstream with a timeout; returns the fetch Response. */
  async upstream(name: "gateway" | "storefront" | "pool", path: string, init: RequestInit & { timeoutMs?: number } = {}): Promise<Response> {
    const base = name === "gateway" ? this.cfg.gatewayUrl : name === "storefront" ? this.cfg.storefrontUrl : this.cfg.poolUrl;
    if (!base) throw new ApiError(501, "upstream_not_configured", `${name} is not configured on this deployment`);
    try {
      return await fetch(base.replace(/\/$/, "") + path, { ...init, signal: AbortSignal.timeout(init.timeoutMs ?? 20_000) });
    } catch (e) {
      throw new ApiError(503, `${name}_unavailable`, `${name} unreachable: ${(e as Error).message.slice(0, 160)}`);
    }
  }

  async upstreamJson(name: "gateway" | "storefront" | "pool", path: string, init: RequestInit & { timeoutMs?: number } = {}): Promise<{ status: number; body: any; headers: Headers }> {
    const r = await this.upstream(name, path, init);
    const text = await r.text();
    let body: any;
    try {
      body = text ? JSON.parse(text) : null;
    } catch {
      body = { raw: text.slice(0, 2000) };
    }
    return { status: r.status, body, headers: r.headers };
  }
}
