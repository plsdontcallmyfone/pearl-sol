// Configuration of the house desktops. Secrets are read from files named by env (never from the
// repo): CADRE_AGENTS_ENV (default ~/.config/cadre/agents.env on the Mac, /etc/lode/desktops.env on
// box 0) names E2B_ENV_FILE and VISION_ENV_FILE / PROVIDERS_ENV_FILE; only the named variables are
// taken from those files. Nothing here is ever printed or logged.
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";

/** E2B list prices, read from https://e2b.dev/pricing on 2026-10-21 (same as units read 2026-10-10). */
export const E2B_RATES = { usd_per_vcpu_s: 0.000014, usd_per_gib_s: 0.0000045, read_at: "2026-10-21", source: "https://e2b.dev/pricing" } as const;

export function usdPerSecond(vcpu: number, ramGib: number): number {
  return vcpu * E2B_RATES.usd_per_vcpu_s + ramGib * E2B_RATES.usd_per_gib_s;
}

export function parseEnvFile(path: string): Record<string, string> {
  const out: Record<string, string> = {};
  if (!path || !existsSync(path)) return out;
  for (const line of readFileSync(path, "utf8").split("\n")) {
    const m = /^\s*([A-Z0-9_]+)\s*=\s*(.*?)\s*(#.*)?$/.exec(line);
    if (m) out[m[1]!] = m[2]!.replace(/^["']|["']$/g, "");
  }
  return out;
}

const expand = (p: string) => p.replace(/^~(?=\/)/, homedir());

export interface Secrets {
  e2bKey: string | null;
  openrouterKey: string | null;
  anthropicKey: string | null;
}

export function loadSecrets(env = process.env): Secrets {
  const idx = env.CADRE_AGENTS_ENV ?? (existsSync("/etc/lode/desktops.env") ? "/etc/lode/desktops.env" : `${homedir()}/.config/cadre/agents.env`);
  const top = { ...parseEnvFile(expand(idx)), ...Object.fromEntries(Object.entries(env).filter(([k]) => k.endsWith("_ENV_FILE")).map(([k, v]) => [k, v ?? ""])) };
  const e2b = parseEnvFile(expand(top.E2B_ENV_FILE ?? ""));
  const prov = parseEnvFile(expand(top.PROVIDERS_ENV_FILE ?? ""));
  const anth = parseEnvFile(expand(top.ANTHROPIC_ENV_FILE ?? ""));
  const pick = (...v: (string | undefined)[]) => v.find((x) => x && x.trim())?.trim() ?? null;
  return {
    e2bKey: pick(env.E2B_API_KEY, e2b.E2B_API_KEY),
    openrouterKey: pick(env.OPENROUTER_API_KEY, prov.OPENROUTER_API_KEY),
    anthropicKey: pick(env.ANTHROPIC_API_KEY, anth.ANTHROPIC_API_KEY, prov.ANTHROPIC_API_KEY),
  };
}

const num = (env: NodeJS.ProcessEnv, k: string, d: number) => (env[k] === undefined || env[k] === "" ? d : Number(env[k]));

export interface DeskConfig {
  template: string;
  vcpu: number;
  ramGib: number;
  resolution: [number, number];
  /** desktops running at once (the spend cap decides; E2B Pro allows 100) */
  maxDesktops: number;
  /** hard cap, USD per UTC day, E2B time + vision model, all desktops */
  usdPerDay: number;
  /** vision model (OpenRouter id) and its step limits */
  visionModel: string;
  maxStepsPerTask: number;
  /** minimum seconds between two vision tasks of one agent */
  taskGapS: number;
  /** a running desktop's E2B timeout is pushed to this many seconds from now every minute */
  keepaliveS: number;
  /** E2B session length (Pro: 86400); a desktop is paused and resumed before it */
  sessionMaxS: number;
  port: number;
  stateDir: string;
  killFile: string;
  siteUrl: string;
  apiUrl: string;
  explorerUrl: string;
  streamSecretFile: string;
  keysDir: string;
  /** handles of the desktop agents, in order (default: personas 30, 29, ... 11) */
  handles: string[] | null;
  walletDist: string;
}

export function deskConfig(env = process.env): DeskConfig {
  const [w, h] = (env.DESK_RESOLUTION ?? "1280x800").split("x").map(Number);
  return {
    template: env.DESK_TEMPLATE ?? "cadre-desktop",
    vcpu: num(env, "DESK_VCPU", 1),
    ramGib: num(env, "DESK_MEM_MB", 1024) / 1024,
    resolution: [w!, h!],
    maxDesktops: num(env, "DESK_MAX", 1),
    usdPerDay: num(env, "DESK_USD_PER_DAY", 25),
    visionModel: env.DESK_VISION_MODEL ?? "anthropic/claude-haiku-5.5",
    maxStepsPerTask: num(env, "DESK_MAX_STEPS", 14),
    taskGapS: num(env, "DESK_TASK_GAP_S", 240),
    keepaliveS: num(env, "DESK_KEEPALIVE_S", 900),
    sessionMaxS: num(env, "DESK_SESSION_MAX_S", 86_400),
    port: num(env, "DESK_PORT", 10720),
    stateDir: env.DESK_STATE_DIR ?? "/var/lib/lode/desktops",
    killFile: env.DESK_KILL_FILE ?? "/var/lib/lode/desktops/STOP",
    siteUrl: env.DESK_SITE_URL ?? "https://cadre.64-95-12-116.sslip.io",
    apiUrl: env.CADRE_API ?? "http://127.0.0.1:10685",
    explorerUrl: env.DESK_EXPLORER_URL ?? "https://cadre.64-95-12-116.sslip.io/explorer",
    streamSecretFile: env.DESK_STREAM_SECRET ?? "/etc/lode/keys/desktops-stream.key",
    keysDir: env.SWARM_KEYS ?? "/etc/lode/keys/swarm",
    handles: env.DESK_HANDLES ? env.DESK_HANDLES.split(",").map((s) => s.trim()).filter(Boolean) : null,
    walletDist: env.DESK_WALLET_DIST ?? "/opt/lode/cadre-wallet",
  };
}
