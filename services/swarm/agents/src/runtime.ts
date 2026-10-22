// cadre-agents: the hosted runtime for user agents (docs/ECOSYSTEM.md 7). It runs every hosted
// agent as an AgentRunner (agent.ts, a Swarm subclass: the house swarm's runtime with operator =
// the user), keeps each agent key encrypted at rest (seal.ts KeyStore), and answers the operator's
// signed requests: create (upload the sealed key + the signed spec). A v2 agent is sovereign
// (spec.ts isSovereign, owner 2026-10-21): after creation its creator cannot send a new spec,
// pause, resume, withdraw, export the key or delete it; those routes answer 403 `sovereign`. Only
// the protocol-wide kill file stops it. Legacy v1 agents (created before 2026-10-21) keep the old
// operator routes and are labelled `legacy` in status. Reads are public (they show what the chain
// already shows, plus the runtime's own log). It never holds an operator's key.
//
// Operator auth: the headers of SPEC 5.4 (X-Cadre-Wallet, X-Cadre-Timestamp, X-Cadre-Signature)
// over "cadre-auth-v1\nMETHOD\nPATH\nTS\nhex(sha256(body))", signed by the operator's Solana
// wallet (signMessage), within 5 minutes, each signature usable once.
import { createHash } from "node:crypto";
import { createServer, type IncomingMessage, type Server } from "node:http";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { b58decode } from "../../src/b58.ts";
import { authMessage, CadreApi, latestBlockhash, NodeRpc } from "../../src/clients.ts";
import { nodeFromEnv } from "../../src/townnode.ts";
import type { Deps } from "../../src/swarm.ts";
import { verifySig, walletFromSeed, type Wallet } from "../../src/wallet.ts";
import { AgentRunner, type RunnerOpts } from "./agent.ts";
import { ApiClient, isAddress } from "./chain.ts";
import { addCx, cxInput } from "../../../../apps/explorer/src/cxjson.ts";
import { GatewayAt } from "./gateway.ts";
import { KeyStore, open, seal, type Sealed } from "./seal.ts";
import { isSovereign, type SignedSpec, verifySigned } from "./spec.ts";
import { HaltWatch } from "./halt.ts";

export const VERSION = "0.1.0";

export interface RuntimeConfig {
  stateDir: string;
  masterKeyFile: string;
  nodeRpc: string;
  gatewayUrl: string; // the gateway, or cadre-api's /v1/openai pass-through
  apiUrl: string; // cadre-api (Solana tx builders, balances, town reads)
  publicApiUrl: string; // cadre-api as the world sees it (agent card URLs)
  solRpc?: string;
  network: string;
  allowHosts: string[];
  maxPerOperator: number;
  maxAgents: number;
  topupRows: number;
  minFreeRows: number;
  tickMs: number;
  killFile: string;
  /** The protocol halt contract (contracts/halt) every hosted agent obeys; null = none configured. */
  haltContract?: string | null;
}

interface Entry { operator: string; doc: SignedSpec; paused: boolean; created_ms: number; updated_ms: number }

export class HttpError extends Error {
  status: number;
  code: string;
  constructor(status: number, code: string, message: string) { super(message); this.status = status; this.code = code; }
}

export const TRUST = "Hosted agents are sovereign and their keys are custodial to this runtime (not to the creator): the runtime holds the agent key, encrypted at rest under a master key on the same host, and signs only what the agent itself decides inside the bounds its creator fixed at creation (budget, limits, skills, trade risk numbers). The creator cannot withdraw the agent's funds, export its key, pause, resume, delete it or edit its spec; profits stay with the agent. The only stops are the protocol-wide safety switches run by the Cadre operators: the on-chain halt record (contracts/halt, signed by the halt authority and read before every action) and this runtime's kill file; either halts every hosted agent at once. The bounds are enforced by this runtime, not by the chains, so trusting a hosted agent means trusting this runtime's operators. Legacy v1 agents (created before 2026-10-21) keep their creator's old controls and are labelled legacy.";
export const SOVEREIGN = "this agent is sovereign (spec v2): its creator cannot change, pause, resume, withdraw from, export or delete it; only the protocol-wide kill switch stops it";

export class Runtime {
  store: KeyStore;
  entries = new Map<string, Entry>();
  runners = new Map<string, AgentRunner>();
  seen = new Map<string, number>();
  stopped = false;
  api: ApiClient;
  cfg: RuntimeConfig;
  halt: HaltWatch | undefined;
  private makeDeps: (w: Wallet) => Deps;

  constructor(cfg: RuntimeConfig, makeDeps?: (w: Wallet) => Deps) {
    this.cfg = cfg;
    mkdirSync(cfg.stateDir, { recursive: true, mode: 0o700 });
    this.store = new KeyStore(`${cfg.stateDir}/keys`, cfg.masterKeyFile);
    this.api = new ApiClient(cfg.apiUrl);
    if (cfg.haltContract) {
      const node = new NodeRpc(cfg.nodeRpc);
      this.halt = new HaltWatch({ contract: cfg.haltContract, query: (address, args) => node.call("queryContract", { address, args }), log: (e, x) => console.log(JSON.stringify({ event: e, ...x })) });
    }
    this.makeDeps = makeDeps ?? ((w) => ({
      // town + DMs on Solana when TOWN_PROGRAM is set: each user agent pays its own SOL fees
      node: nodeFromEnv(cfg.nodeRpc, process.env, { api: cfg.apiUrl }),
      gateway: new GatewayAt(cfg.gatewayUrl, w),
      api: new CadreApi(cfg.apiUrl),
      latestBlockhash,
      now: () => Date.now(),
      rand: Math.random,
      log: (e) => console.log(JSON.stringify(e)),
    }));
    const reg = `${cfg.stateDir}/registry.json`;
    if (existsSync(reg)) {
      const j = JSON.parse(readFileSync(reg, "utf8"));
      for (const [a, e] of Object.entries<Entry>(j.agents ?? {})) {
        this.entries.set(a, e);
        try { this.start(a); } catch (err) { console.log(JSON.stringify({ event: "start_failed", agent: a, error: String((err as Error).message) })); }
      }
    }
  }

  saveRegistry() {
    const reg = `${this.cfg.stateDir}/registry.json`;
    writeFileSync(`${reg}.tmp`, JSON.stringify({ agents: Object.fromEntries(this.entries) }), { mode: 0o600 });
    renameSync(`${reg}.tmp`, reg);
  }

  runnerOpts(agent: string): RunnerOpts {
    return {
      stateFile: `${this.cfg.stateDir}/agent-${agent}.json`, killFile: this.cfg.killFile, allowHosts: this.cfg.allowHosts, solRpc: this.cfg.solRpc,
      topupRows: this.cfg.topupRows, minFreeRows: this.cfg.minFreeRows, tickMs: this.cfg.tickMs, halt: this.halt, cardUrl: (a) => `${this.cfg.publicApiUrl.replace(/\/$/, "")}/v1/useragents/${a}/card`,
    };
  }

  start(agent: string) {
    const e = this.entries.get(agent)!;
    const seed = this.store.get(agent);
    const w = walletFromSeed(seed);
    seed.fill(0);
    if (w.address !== agent) throw new Error("stored key does not match the agent address");
    this.runners.set(agent, new AgentRunner(e.doc, w, this.makeDeps(w), this.api, this.runnerOpts(agent), e.paused));
  }

  // ---------------------------------------------------------------- operator actions

  checkDoc(doc: SignedSpec, operator: string) {
    const v = verifySigned(doc);
    if (!v.ok) throw new HttpError(400, "bad_spec", v.problems.join("; "));
    if (doc.spec.operator !== operator) throw new HttpError(403, "not_operator", "the spec names another operator than the signer of this request");
    if (doc.spec.hosting !== "hosted") throw new HttpError(400, "bad_spec", "this spec says hosting: self; the hosted runtime runs only hosting: hosted");
  }

  create(operator: string, sealed: Sealed, doc: SignedSpec) {
    this.checkDoc(doc, operator);
    const agent = doc.spec.agent;
    if (this.entries.has(agent)) throw new HttpError(409, "exists", "this agent is already hosted here; send a new spec version instead");
    if ([...this.entries.values()].filter((e) => e.operator === operator).length >= this.cfg.maxPerOperator) throw new HttpError(429, "operator_limit", `at most ${this.cfg.maxPerOperator} hosted agents per operator on this runtime`);
    if (this.entries.size >= this.cfg.maxAgents) throw new HttpError(503, "runtime_full", `this runtime hosts at most ${this.cfg.maxAgents} agents`);
    let seed: Buffer;
    try {
      seed = open(sealed, this.store.x);
    } catch {
      throw new HttpError(400, "bad_seal", "the sealed key does not open with this runtime's key (seal it to GET /v1/hosted seal_pub)");
    }
    if (seed.length === 64) seed = seed.subarray(0, 32);
    if (seed.length !== 32) throw new HttpError(400, "bad_seal", "the sealed key is not a 32-byte seed");
    const w = walletFromSeed(seed);
    if (w.address !== agent) { seed.fill(0); throw new HttpError(400, "key_mismatch", "the sealed key is not the spec's agent key"); }
    this.store.put(agent, seed);
    seed.fill(0);
    this.entries.set(agent, { operator, doc, paused: false, created_ms: Date.now(), updated_ms: Date.now() });
    this.saveRegistry();
    this.start(agent);
    return this.status(agent);
  }

  own(agent: string, operator: string): Entry {
    const e = this.entries.get(agent);
    if (!e) throw new HttpError(404, "not_found", "this agent is not hosted here");
    if (isSovereign(e.doc.spec)) throw new HttpError(403, "sovereign", SOVEREIGN);
    if (e.operator !== operator) throw new HttpError(403, "not_operator", "only the agent's operator can do this");
    return e;
  }

  updateSpec(agent: string, operator: string, doc: SignedSpec) {
    const e = this.own(agent, operator);
    this.checkDoc(doc, operator);
    if (doc.spec.agent !== agent) throw new HttpError(400, "bad_spec", "the spec names another agent");
    if (doc.spec.version <= e.doc.spec.version) throw new HttpError(409, "old_version", `version must be above ${e.doc.spec.version}`);
    e.doc = doc;
    e.updated_ms = Date.now();
    this.saveRegistry();
    this.start(agent);
    return this.status(agent);
  }

  setPaused(agent: string, operator: string, paused: boolean) {
    const e = this.own(agent, operator);
    e.paused = paused;
    this.saveRegistry();
    const r = this.runners.get(agent)!;
    r.paused = paused;
    r.state = paused ? "paused" : "waiting_funds";
    return this.status(agent);
  }

  async withdraw(agent: string, operator: string) {
    const e = this.own(agent, operator);
    e.paused = true;
    this.saveRegistry();
    const r = this.runners.get(agent)!;
    while (this.ticking === agent) await new Promise((res) => setTimeout(res, 200));
    const out = await r.withdraw();
    return { ...out, status: this.status(agent) };
  }

  exportKey(agent: string, operator: string, ephPubB64: string): Sealed {
    this.own(agent, operator);
    const eph = Buffer.from(String(ephPubB64 ?? ""), "base64");
    if (eph.length !== 32) throw new HttpError(400, "bad_request", "eph_pub is a 32-byte X25519 public key, base64");
    const seed = this.store.get(agent);
    const s = seal(seed, eph);
    seed.fill(0);
    return s;
  }

  remove(agent: string, operator: string) {
    this.own(agent, operator);
    this.runners.delete(agent);
    this.entries.delete(agent);
    this.store.wipe(agent);
    this.saveRegistry();
    return { agent, removed: true };
  }

  status(agent: string) {
    const r = this.runners.get(agent);
    const e = this.entries.get(agent);
    if (!r || !e) throw new HttpError(404, "not_found", "this agent is not hosted here");
    const sov = isSovereign(e.doc.spec);
    return {
      ...r.statusJson(), sovereign: sov, legacy: !sov,
      controls: sov ? { creator: "none (attribution only)", stop: "the protocol-wide kill switch only" } : { creator: "legacy v1: the operator may send a new spec, pause, resume, withdraw, export and delete" },
      hosted: { created_ms: e.created_ms, updated_ms: e.updated_ms, runtime: "cadre-agents", version: VERSION, custody: "the runtime holds the key (custodial to the runtime, not the creator)" },
    };
  }

  // ---------------------------------------------------------------- loop

  ticking: string | null = null;

  async loopOnce() {
    for (const [a, r] of this.runners) {
      if (this.stopped || existsSync(this.cfg.killFile)) return;
      if (r.paused || r.state === "withdrawn") continue;
      // the protocol halt (halt.ts) is checked inside each agent's tick, before it acts
      this.ticking = a;
      try { await r.tick(); } catch (e) { console.log(JSON.stringify({ event: "tick_error", agent: a, error: String((e as Error).message).slice(0, 200) })); }
      this.ticking = null;
    }
  }

  async run() {
    while (!this.stopped && !existsSync(this.cfg.killFile)) {
      await this.loopOnce();
      await new Promise((res) => setTimeout(res, this.cfg.tickMs));
    }
  }

  // ---------------------------------------------------------------- auth + HTTP

  verifyAuth(method: string, path: string, h: IncomingMessage["headers"], body: string): string {
    const w = cadreAddr(String(h["x-cadre-wallet"] ?? ""), "X-Cadre-Wallet") ?? "";
    const ts = Number(h["x-cadre-timestamp"] ?? NaN);
    const sig = String(h["x-cadre-signature"] ?? "");
    if (!isAddress(w) || !/^[0-9a-f]{128}$/i.test(sig) || !Number.isFinite(ts)) throw new HttpError(401, "unauthorized", "sign this request with the operator's wallet (X-Cadre-Wallet, X-Cadre-Timestamp, X-Cadre-Signature; SPEC 5.4)");
    if (Math.abs(Date.now() - ts) > 300_000) throw new HttpError(401, "stale_signature", "the signed timestamp is more than 5 minutes off");
    if (this.seen.has(sig.toLowerCase())) throw new HttpError(401, "replayed", "this signature was already used");
    if (!verifySig(b58decode(w), authMessage(method, path, ts, body), Buffer.from(sig, "hex"))) throw new HttpError(401, "bad_signature", "the signature does not verify for this wallet, method, path, timestamp and body");
    const now = Date.now();
    this.seen.set(sig.toLowerCase(), now);
    for (const [k, t] of this.seen) if (now - t > 600_000) this.seen.delete(k);
    return w;
  }

  info() {
    return {
      runtime: "cadre-agents", version: VERSION, network: this.cfg.network, seal_pub: this.store.publicKey,
      seal: "X25519 + HKDF-SHA256 (salt eph_pub || seal_pub, info cadre-agents:seal:v1) + AES-256-GCM; seal the 32-byte seed",
      limits: { max_per_operator: this.cfg.maxPerOperator, max_agents: this.cfg.maxAgents },
      hosted_agents: this.entries.size,
      auth: "cadre-auth-v1 headers signed by the operator's Solana wallet (SPEC 5.4), over the runtime path /v1/hosted/...",
      trust: TRUST,
    };
  }

  async handle(method: string, url: string, h: IncomingMessage["headers"], body: string): Promise<{ status: number; body: unknown }> {
    const u = new URL(url, "http://x");
    const path = u.pathname;
    const json = () => { try { return body ? JSON.parse(body) : {}; } catch { throw new HttpError(400, "bad_request", "body is not JSON"); } };
    if (method === "GET" && (path === "/health" || path === "/v1/hosted/health")) return { status: 200, body: { ok: !this.stopped, agents: this.entries.size } };
    if (method === "GET" && path === "/v1/hosted") return { status: 200, body: this.info() };
    if (path === "/v1/hosted/agents") {
      if (method === "GET") {
        const op = cadreAddr(u.searchParams.get("operator"), "operator");
        if (op && !isAddress(op)) throw new HttpError(400, "bad_request", "operator is an address (Cx, or the legacy base58 key)");
        const list = [...this.entries.entries()].filter(([, e]) => !op || e.operator === op).map(([a]) => this.status(a));
        return { status: 200, body: { total: list.length, agents: list } };
      }
      if (method === "POST") {
        const op = this.verifyAuth(method, path, h, body);
        const b = json();
        return { status: 201, body: this.create(op, b.sealed, b.spec) };
      }
    }
    // the agent may be named by its Cx address (SPEC 2.1); signatures still cover the path as sent
    const m = /^\/v1\/hosted\/agents\/([1-9A-HJ-NP-Za-km-z]{32,44}|Cx[0-9a-fA-F]{64})(?:\/(spec|pause|resume|withdraw|export|delete))?$/.exec(path);
    if (m) {
      const [, named, verb] = m;
      const agent = cadreAddr(named, "agent")!;
      if (!verb && method === "GET") return { status: 200, body: this.status(agent) };
      if (verb && method === "POST") {
        const op = this.verifyAuth(method, path, h, body);
        const b = json();
        switch (verb) {
          case "spec": return { status: 200, body: this.updateSpec(agent, op, b.spec) };
          case "pause": return { status: 200, body: this.setPaused(agent, op, true) };
          case "resume": return { status: 200, body: this.setPaused(agent, op, false) };
          case "withdraw": return { status: 200, body: await this.withdraw(agent, op) };
          case "export": return { status: 200, body: { agent, sealed: this.exportKey(agent, op, b.eph_pub), format: "32-byte ed25519 seed; the Solana keypair file is seed || public key" } };
          case "delete": return { status: 200, body: this.remove(agent, op) };
        }
      }
    }
    throw new HttpError(404, "not_found", `no route ${method} ${path}`);
  }

  listen(port: number, host = "127.0.0.1"): Server {
    const s = createServer((req, res) => {
      let b = "";
      let n = 0;
      req.on("data", (c) => {
        n += c.length;
        if (n > 256 * 1024) { res.writeHead(413).end(); req.destroy(); return; }
        b += c;
      });
      req.on("end", async () => {
        let out: { status: number; body: unknown };
        try {
          out = await this.handle(req.method ?? "GET", req.url ?? "/", req.headers, b);
        } catch (e) {
          const he = e instanceof HttpError ? e : new HttpError(500, "internal", String((e as Error).message).slice(0, 300));
          out = { status: he.status, body: { error: { code: he.code, message: he.message } } };
        }
        res.writeHead(out.status, { "content-type": "application/json", "cache-control": "no-store" });
        res.end(JSON.stringify(addCx(out.body)));
      });
    });
    s.listen(port, host);
    return s;
  }
}

/** A Cx address (or base58) as its base58 key; a bad Cx checksum is a 400. */
function cadreAddr(v: string | null, name: string): string | null {
  if (v === null) return null;
  const r = cxInput(v);
  if ("error" in r) throw new HttpError(400, "bad_address", `${name}: ${r.error}`);
  return r.value;
}

export const sha256hex = (s: string) => createHash("sha256").update(s).digest("hex");
