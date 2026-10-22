// The dump repo working copy: reset to the remote, re-render every file, one commit, one push.
// The writer owns the repo, so a hard reset to origin/main is the restart-safe starting point.
import { execFile } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { promisify } from "node:util";
import type { Auth, Identity } from "./auth.ts";

const run = promisify(execFile);

async function authEnv(auth: Auth | null): Promise<NodeJS.ProcessEnv> {
  const env: NodeJS.ProcessEnv = { ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_ASKPASS: "/bin/false" };
  if (auth) {
    const basic = Buffer.from(`x-access-token:${await auth.token()}`).toString("base64");
    env.GIT_CONFIG_COUNT = "1";
    env.GIT_CONFIG_KEY_0 = "http.https://github.com/.extraheader";
    env.GIT_CONFIG_VALUE_0 = `AUTHORIZATION: basic ${basic}`;
  }
  return env;
}

export async function git(dir: string, args: string[], auth: Auth | null = null): Promise<string> {
  try {
    const { stdout } = await run("git", args, { cwd: dir, env: await authEnv(auth), maxBuffer: 64 << 20 });
    return stdout.trim();
  } catch (e: any) {
    // never echo the environment; stderr from git does not contain the header
    throw new Error(`git ${args[0]} failed: ${String(e.stderr ?? e.message).trim().slice(0, 400)}`);
  }
}

/** Clone or fetch, then put the working copy exactly at origin/main (or an unborn main). */
export async function sync(dir: string, remote: string, auth: Auth | null): Promise<void> {
  if (!existsSync(join(dir, ".git"))) {
    mkdirSync(dir, { recursive: true });
    await git(dir, ["init", "-q", "-b", "main"]);
    await git(dir, ["remote", "add", "origin", remote]);
  }
  await git(dir, ["fetch", "-q", "origin"], auth);
  const hasMain = await git(dir, ["ls-remote", "--heads", "origin", "main"], auth).then((s) => s.length > 0);
  if (hasMain) {
    await git(dir, ["checkout", "-q", "-B", "main", "origin/main"]);
    await git(dir, ["reset", "-q", "--hard", "origin/main"]);
  } else {
    await git(dir, ["symbolic-ref", "HEAD", "refs/heads/main"]);
  }
  await git(dir, ["clean", "-q", "-fdx"]);
}

/** Replace every tracked file with the rendered set (files not rendered any more are removed). */
export function writeFiles(dir: string, files: Map<string, string>): void {
  for (const e of readdirSync(dir)) if (e !== ".git") rmSync(join(dir, e), { recursive: true, force: true });
  for (const [p, body] of [...files.entries()].sort(([a], [b]) => a.localeCompare(b))) {
    if (p.startsWith("/") || p.split("/").includes("..")) throw new Error(`refusing path ${p}`);
    const f = join(dir, p);
    mkdirSync(dirname(f), { recursive: true });
    writeFileSync(f, body);
  }
}

export interface CommitResult { committed: boolean; sha: string | null; changed: number; paths: string[] }

export async function commit(dir: string, message: string, who: Identity): Promise<CommitResult> {
  await git(dir, ["add", "-A"]);
  const status = await git(dir, ["status", "--porcelain"]);
  if (!status) return { committed: false, sha: null, changed: 0, paths: [] };
  const paths = status.split("\n").map((l) => l.slice(3));
  await git(dir, ["-c", `user.name=${who.name}`, "-c", `user.email=${who.email}`, "commit", "-q", "-m", message]);
  return { committed: true, sha: await git(dir, ["rev-parse", "HEAD"]), changed: paths.length, paths };
}

export async function push(dir: string, auth: Auth): Promise<void> {
  await git(dir, ["push", "-q", "origin", "HEAD:refs/heads/main"], auth);
}
