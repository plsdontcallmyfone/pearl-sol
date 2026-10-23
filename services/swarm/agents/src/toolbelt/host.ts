// Glue between the hosted runtime and the toolbelt (docs/TOOLBELT.md 10): one Toolbelt per
// AgentRunner (so per swarm member), built from env once per process:
//   TOOLBELT_PAYEE           base58 Solana address that receives top-ups (the runtime operator, who
//                            pays the outside providers); unset = toolbelt top-ups refused
//   TOOLBELT_CONTRACT        the receipts contract (contracts/toolbelt); default deploy/toolbelt-devnet.json
//   TOOLBELT_RUNTIME_USD_PER_DAY  outside spend cap for ALL agents of this runtime (default 5, a bound)
//   TOOLBELT_ENV_FILE / FIRECRAWL_KEY_FILE / BRAVE_KEY_FILE / AGENTMAIL_KEY_FILE: provider keys
//   CADRE_AGENTS_ENV         the house desktops' key index (E2B, OpenRouter), reused read-only
//   TOOLBELT_GITHUB_APP_ID, TOOLBELT_GITHUB_APP_KEY_FILE, TOOLBELT_GITHUB_ORG: the Cadre GitHub App (github.ts)
//   AGENTS_STATE: the runtime's own state dir
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { blake3 } from "../../../src/blake3.ts";
import { Abi, contractFee, contractSigningBytes, signContractTx } from "../../../trading/src/cadre.ts";
import type { AgentRunner } from "../agent.ts";
import { appConfigFromEnv, GitHubApp } from "./github.ts";
import { toCx } from "../../../../../packages/cx-address/src/index.ts";
import { Toolbelt, type ToolHost } from "./index.ts";
import { RuntimeLedger } from "./meter.ts";
import { priceConfigFromEnv } from "./price.ts";
import { loadProviderKeys, type ProviderKeys } from "./providers.ts";
import type { ToolBatch } from "./receipts.ts";
import { e2bCodeSandbox, e2bComputer } from "./sandbox.ts";

const here = dirname(fileURLToPath(import.meta.url));

export function toolbeltContractFrom(env: Record<string, string | undefined>, file = join(here, "../../../../../deploy/toolbelt-devnet.json")): string | null {
  const v = env.TOOLBELT_CONTRACT;
  if (v === "off") return null;
  if (v) return v;
  try { return existsSync(file) ? String(JSON.parse(readFileSync(file, "utf8")).contract) : null; } catch { return null; }
}

interface Shared { code?: ReturnType<typeof e2bCodeSandbox> | null; computer?: ReturnType<typeof e2bComputer> | null; keys: ProviderKeys; rt: RuntimeLedger; githubApp: GitHubApp | null; payee: string | null; contract: string | null }
let shared: Shared | null = null;

export function sharedToolbelt(env: Record<string, string | undefined> = process.env): Shared {
  if (shared) return shared;
  const state = env.AGENTS_STATE ?? "/var/lib/lode/agents";
  const app = appConfigFromEnv(env);
  const cap = Number(env.TOOLBELT_RUNTIME_USD_PER_DAY ?? 5);
  shared = { keys: loadProviderKeys(env), rt: new RuntimeLedger(Number.isFinite(cap) && cap >= 0 ? cap : 5, `${state}/toolbelt-runtime.json`), githubApp: app ? new GitHubApp(app) : null, payee: env.TOOLBELT_PAYEE ?? null, contract: toolbeltContractFrom(env) };
  return shared;
}
export function resetSharedForTests(s: Shared | null = null) { shared = s; }

async function solRpc(url: string, method: string, params: unknown[]) {
  const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }), signal: AbortSignal.timeout(30_000) });
  return ((await r.json()) as any).result;
}

/** The ToolHost of one runner: halt, blobs, top-ups from the agent's own SOL, batch commits. */
export function hostFor(r: AgentRunner, s: Shared): ToolHost {
  return {
    agent: r.address, handle: r.spec.handle, name: r.spec.name,
    now: () => r.d.now(),
    halted: () => (r.state === "halted" ? r.why || "halted" : r.haltReason("all") || (existsSync(r.o.killFile) ? "the hosted runtime's kill switch is on" : "")),
    putBlob: async (data) => { const p = await r.d.node.putBlob(data, true); return { uri: p.uri, id: Buffer.from(blake3(data)).toString("hex") }; },
    topup: async (lamports) => {
      if (!s.payee) throw new Error("this runtime has no TOOLBELT_PAYEE configured");
      const x = await r.solanaAction("/v1/tx/sol-transfer", { from: r.address, to: s.payee, lamports: lamports.toString() }, lamports + 20_000n);
      await r.receipt(`toolbelt top-up: ${lamports} lamports from its own wallet to the toolbelt payee ${s.payee}`, [x.signature]);
      return { signature: x.signature };
    },
    verifyTopup: async (sig, lamports) => {
      if (!r.o.solRpc || !s.payee) return false;
      for (let i = 0; i < 10; i++) {
        const tx = await solRpc(r.o.solRpc, "getTransaction", [sig, { encoding: "jsonParsed", commitment: "confirmed", maxSupportedTransactionVersion: 0 }]).catch(() => null);
        if (tx) {
          if (tx.meta?.err) return false;
          return (tx.transaction?.message?.instructions ?? []).some((ix: any) => ix.program === "system" && ix.parsed?.type === "transfer" && ix.parsed.info?.source === r.address && ix.parsed.info?.destination === s.payee && String(ix.parsed.info?.lamports) === lamports.toString());
        }
        await new Promise((res) => setTimeout(res, 2000));
      }
      return false;
    },
    commitBatch: (b, blob) => commitBatch(r, s, b, blob),
    note: (k, t, x) => r.note(k, t, x as any),
    cx: toCx(r.address),
    sign: (msg) => Buffer.from(r.me.wallet.sign(msg)).toString("hex"),
  };
}

/** contracts/toolbelt commit(root, first_seq, n, price, blob), signed by the agent; else a receipt post. */
export async function commitBatch(r: AgentRunner, s: Shared, b: ToolBatch, blob: { uri: string; id: string }): Promise<{ tx: string; via: string }> {
  if (!s.contract) {
    const h = await r.receipt(`toolbelt receipts ${b.first_seq}..${b.first_seq + b.n - 1} root ${b.root} paid ${b.price_lamports} lamports batch ${blob.uri}`, []);
    if (!h) throw new Error("the receipt post was refused");
    return { tx: h, via: "receipt_post" };
  }
  const args = Abi.method("commit").hash(b.root).u64(BigInt(b.first_seq)).u32(b.n).u64(BigInt(b.price_lamports)).hash(blob.id).out();
  const send = async () => {
    if (!r.params) await r.refreshParams();
    const sim = await r.d.node.call("simulateCall", { from: r.address, address: s.contract, args: Buffer.from(args).toString("hex"), value: 0 });
    if (!sim?.ok) throw new Error(`receipts commit simulation failed: ${sim?.error ? Buffer.from(sim.error, "hex").toString("utf8") : "unknown"}`);
    const body = { kind: "Call" as const, contract: s.contract!, value: 0n, gasLimit: BigInt(Math.ceil(sim.gas_used * 1.25)) + 20_000n, args };
    const w = r.me.wallet;
    const nonce = await r.nonceOf(w);
    const len = contractSigningBytes(r.chainId, w.publicKey, nonce, 0n, body).length + 64;
    const fee = contractFee(r.params as any, len, body.gasLimit);
    if (!r.budget.canTransfer(fee, r.d.now())) throw new Error(`CADRE cap: the receipts commit fee ${fee} would pass today's cap`);
    try {
      const res = await r.d.node.sendTx(signContractTx(w, r.chainId, nonce, fee, body));
      r.nonces.set(w.address, nonce + 1n);
      r.budget.bookTransfer(fee);
      return res.tx_hash as string;
    } catch (e) {
      if (/nonce/i.test((e as Error).message)) r.nonces.delete(w.address);
      throw e;
    }
  };
  // one nonce sequence per key: queue behind the runner's other sends
  const p = r.sendChain.then(send);
  r.sendChain = p.catch(() => {});
  return { tx: await p, via: `contract ${s.contract}` };
}

/** Build the agent's Toolbelt when its v2 spec has a toolbelt section. */
export function toolbeltFor(r: AgentRunner, saved?: any, env: Record<string, string | undefined> = process.env): Toolbelt | null {
  const t = (r.spec as any).toolbelt;
  if (!t || !r.sovereign) return null;
  const s = sharedToolbelt(env);
  return new Toolbelt(t, hostFor(r, s), {
    keys: s.keys, rt: s.rt, price: priceConfigFromEnv(env), githubApp: s.githubApp,
    meterFile: `${r.o.stateDir ?? dirname(r.o.stateFile)}/toolbelt-${r.address}.json`,
    // one code sandbox factory and one warm desktop pool per runtime, shared by every agent
    code: s.code === undefined ? (s.code = s.keys.e2b ? e2bCodeSandbox(s.keys.e2b) : null) : s.code,
    computer: !t.computer ? null : s.computer === undefined ? (s.computer = s.keys.e2b && s.keys.openrouter ? e2bComputer(s.keys.e2b, s.keys.openrouter) : null) : s.computer,
  }, saved);
}

// ---------------------------------------------------------------- public status route

export interface RouteHost { entries: Map<string, unknown>; runners: Map<string, AgentRunner> }

export class RouteError extends Error { status: number; code: string; constructor(s: number, c: string, m: string) { super(m); this.status = s; this.code = c; } }

const RE = /^\/v1\/hosted\/agents\/([1-9A-HJ-NP-Za-km-z]{32,44})\/toolbelt$/;

/** GET /v1/hosted/agents/{agent}/toolbelt (public, like status). null when the path is not ours. */
export async function toolbeltRoute(rt: RouteHost, method: string, path: string): Promise<{ status: number; body: unknown } | null> {
  const m = RE.exec(path);
  if (!m) return null;
  if (method !== "GET") throw new RouteError(405, "method_not_allowed", "GET only");
  if (!rt.entries.has(m[1])) throw new RouteError(404, "not_found", "this agent is not hosted here");
  return { status: 200, body: { agent: m[1], toolbelt: rt.runners.get(m[1])?.toolbelt?.status() ?? null } };
}
