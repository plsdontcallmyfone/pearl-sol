// Shared helpers for the M1 end to end demo (scripts/demo.sh -> scripts/demo/run.ts).
// Imports resolve through scripts/demo/node_modules, a symlink demo.sh makes to
// services/relayer/node_modules, so web3.js is the same module instance the relayer
// code uses.

import { spawn, execFileSync } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
export const DEMO = resolve(ROOT, process.env.DEMO_DIR ?? ".demo");
export const LOGS = resolve(DEMO, "logs");
export const PIDS = resolve(DEMO, "pids");
for (const d of [DEMO, LOGS, PIDS, resolve(DEMO, "keys")]) mkdirSync(d, { recursive: true });

export const TOKEN_RPC = "http://127.0.0.1:10600";
export const NODE_RPC = "http://127.0.0.1:10630";
export const RELAYER_HEALTH = "http://127.0.0.1:10660/health";
export const ATTESTER_URL = "http://127.0.0.1:10661";
export const STOREFRONT_URL = "http://127.0.0.1:10662";

export const conn = new Connection(TOKEN_RPC, "confirmed");

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export function loadKp(path: string): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(resolve(ROOT, path), "utf8"))));
}

/** A demo key under .demo/keys (created once). */
export function demoKey(name: string): { kp: Keypair; file: string } {
  const file = resolve(DEMO, "keys", `${name}.json`);
  if (!existsSync(file)) writeFileSync(file, JSON.stringify(Array.from(Keypair.generate().secretKey)));
  return { kp: loadKp(file), file };
}

export const KEYS = {
  payer: "keys/localnet/payer-keypair.json",
  attester: "keys/localnet/attester-keypair.json",
  miner: "keys/localnet/miner-keypair.json",
};

export function pubkeyOfKeyfile(path: string): PublicKey {
  return loadKp(path).publicKey;
}

// ------------------------------------------------------------ processes

export interface Proc {
  name: string;
  pid: number;
  log: string;
}
export const procs: Proc[] = [];

/** Starts a long-running process, logs to .demo/logs/<name>.log, records its PID. */
export function start(name: string, cmd: string, args: string[], opts: { cwd?: string; env?: Record<string, string> } = {}): Proc {
  const log = resolve(LOGS, `${name}.log`);
  const fd = openSync(log, "a");
  const child = spawn(cmd, args, { cwd: opts.cwd ?? ROOT, env: { ...process.env, ...opts.env }, stdio: ["ignore", fd, fd], detached: false });
  closeSync(fd);
  if (!child.pid) throw new Error(`failed to start ${name}`);
  child.unref();
  const p = { name, pid: child.pid, log };
  procs.push(p);
  writeFileSync(resolve(PIDS, `${name}.pid`), `${child.pid}\n`);
  return p;
}

/** Runs a command to completion; returns stdout (throws with output on non-zero). */
export function run(cmd: string, args: string[], opts: { cwd?: string; env?: Record<string, string>; timeoutMs?: number } = {}): string {
  try {
    return execFileSync(cmd, args, { cwd: opts.cwd ?? ROOT, env: { ...process.env, ...opts.env }, encoding: "utf8",
      timeout: opts.timeoutMs ?? 300_000, stdio: ["ignore", "pipe", "pipe"], maxBuffer: 64 << 20 });
  } catch (e) {
    const x = e as { stdout?: string; stderr?: string; message: string };
    throw new Error(`${cmd} ${args.join(" ")} failed: ${x.message}\n${x.stdout ?? ""}\n${x.stderr ?? ""}`.slice(0, 4000));
  }
}

/** Last JSON object line of a command's stdout. */
export function lastJson(out: string): Record<string, any> {
  const lines = out.trim().split("\n").reverse();
  for (const l of lines) {
    const t = l.trim();
    if (t.startsWith("{")) return JSON.parse(t);
  }
  throw new Error(`no JSON in output: ${out.slice(0, 500)}`);
}

export async function waitFor<T>(desc: string, f: () => Promise<T | null | undefined | false>, timeoutMs = 120_000, everyMs = 1000): Promise<T> {
  const until = Date.now() + timeoutMs;
  let lastErr = "";
  for (;;) {
    try {
      const v = await f();
      if (v) return v as T;
    } catch (e) {
      lastErr = String((e as Error).message ?? e).slice(0, 300);
    }
    if (Date.now() > until) throw new Error(`timed out waiting for ${desc}${lastErr ? ` (last error: ${lastErr})` : ""}`);
    await sleep(everyMs);
  }
}

export async function httpJson(url: string, init?: RequestInit): Promise<{ status: number; body: any; headers: Headers }> {
  const r = await fetch(url, { ...init, signal: AbortSignal.timeout(30_000) });
  const text = await r.text();
  let body: any = text;
  try {
    body = JSON.parse(text);
  } catch {
    /* plain text */
  }
  return { status: r.status, body, headers: r.headers };
}

export async function nodeCall(method: string, params: Record<string, unknown> = {}, url = NODE_RPC): Promise<any> {
  const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }), signal: AbortSignal.timeout(10_000) });
  const j = (await r.json()) as { result?: unknown; error?: { message: string } };
  if (j.error) throw new Error(`node ${method}: ${j.error.message}`);
  return j.result;
}

export function portBusy(port: number): string | null {
  try {
    const out = execFileSync("lsof", ["-ti", `:${port}`], { encoding: "utf8" }).trim();
    return out || null;
  } catch {
    return null;
  }
}
