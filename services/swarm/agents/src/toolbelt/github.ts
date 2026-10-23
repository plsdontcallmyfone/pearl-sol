// GitHub that Cadre provides (owner decision 2026-10-22, docs/TOOLBELT.md 3.3). No GitHub user
// accounts are created (mass account creation breaks GitHub's terms) and creators connect nothing:
//   - one Cadre GitHub App, installed on a Cadre-owned org (default cadredotag);
//   - each opted-in agent gets its own public repo in that org, named from its handle;
//   - every write uses a short-lived installation token (about 1 h) minted by the runtime and scoped
//     to that one repo (contents rw, pull requests rw, metadata r); repo creation uses a token scoped
//     to administration rw only. Agents never see any token; sandboxes never get one;
//   - commits are authored by the app's bot, and carry trailers naming the agent's handle, its Cx
//     address, the hash of a statement the agent signed with its own key, and that signature.
// Until the app's id and private key are on the host, every GitHub tool answers "needs_github_app".
import { createSign } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";

export interface GhFile { path: string; content: string }

export interface AppConfig { appId: string; privateKeyPem: string; org: string; installationId: string | null; api: string }

const expand = (p: string) => p.replace(/^~(?=\/)/, homedir());

/** TOOLBELT_GITHUB_APP_ID, TOOLBELT_GITHUB_APP_KEY_FILE (default ~/.config/cadre/github-app.pem),
 * TOOLBELT_GITHUB_ORG (default cadredotag), TOOLBELT_GITHUB_INSTALLATION (optional; found by org). */
export function appConfigFromEnv(env: Record<string, string | undefined> = process.env): AppConfig | null {
  const id = env.TOOLBELT_GITHUB_APP_ID?.trim();
  const file = expand(env.TOOLBELT_GITHUB_APP_KEY_FILE ?? "~/.config/cadre/github-app.pem");
  if (!id || !existsSync(file)) return null;
  const pem = readFileSync(file, "utf8");
  if (!/BEGIN (RSA )?PRIVATE KEY/.test(pem)) return null;
  return { appId: id, privateKeyPem: pem, org: env.TOOLBELT_GITHUB_ORG ?? "cadredotag", installationId: env.TOOLBELT_GITHUB_INSTALLATION ?? null, api: env.TOOLBELT_GITHUB_API ?? "https://api.github.com" };
}

const b64u = (b: Buffer | string) => Buffer.from(b).toString("base64").replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");

/** The app's JWT (RS256, 9 minutes, iat 60 s in the past as GitHub recommends). */
export function appJwt(c: AppConfig, nowS = Math.floor(Date.now() / 1000)): string {
  const head = b64u(JSON.stringify({ alg: "RS256", typ: "JWT" }));
  const body = b64u(JSON.stringify({ iat: nowS - 60, exp: nowS + 540, iss: c.appId }));
  const s = createSign("RSA-SHA256");
  s.update(`${head}.${body}`);
  return `${head}.${body}.${b64u(s.sign(c.privateKeyPem))}`;
}

async function gh(api: string, token: string, method: string, path: string, body?: unknown): Promise<any> {
  const r = await fetch(`${api}${path}`, {
    method,
    headers: { authorization: `Bearer ${token}`, accept: "application/vnd.github+json", "x-github-api-version": "2022-11-28", "user-agent": "cadre-toolbelt", ...(body ? { "content-type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(30_000),
  });
  const t = await r.text();
  let j: any = null;
  try { j = t ? JSON.parse(t) : null; } catch { j = t; }
  if (!r.ok) throw Object.assign(new Error(`GitHub ${method} ${path.split("?")[0]} answered ${r.status}: ${String(j?.message ?? "").slice(0, 160)}`), { status: r.status });
  return j;
}

/** Mints scoped installation tokens; caches each until 5 minutes before it expires. */
export class GitHubApp {
  c: AppConfig;
  private cache = new Map<string, { token: string; until: number }>();
  constructor(c: AppConfig) { this.c = c; }
  async installation(): Promise<string> {
    if (!this.c.installationId) this.c.installationId = String((await gh(this.c.api, appJwt(this.c), "GET", `/orgs/${this.c.org}/installation`)).id);
    return this.c.installationId;
  }
  /** repo = null: an org-level token with administration rw only (repo creation); else one repo, contents + PRs rw. */
  async token(repo: string | null): Promise<string> {
    const k = repo ?? "(admin)";
    const hit = this.cache.get(k);
    if (hit && hit.until > Date.now()) return hit.token;
    const body = repo ? { repositories: [repo], permissions: { contents: "write", pull_requests: "write", metadata: "read" } } : { permissions: { administration: "write", metadata: "read" } };
    const j = await gh(this.c.api, appJwt(this.c), "POST", `/app/installations/${await this.installation()}/access_tokens`, body);
    this.cache.set(k, { token: String(j.token), until: Date.parse(j.expires_at) - 300_000 });
    return String(j.token);
  }
  async repo(name: string): Promise<any | null> {
    try { return await gh(this.c.api, await this.token(name), "GET", `/repos/${this.c.org}/${name}`); } catch (e: any) { if (e.status === 404 || e.status === 403 || e.status === 422) return null; throw e; }
  }
  async createRepo(name: string, description: string): Promise<{ full_name: string; url: string; default_branch: string }> {
    const j = await gh(this.c.api, await this.token(null), "POST", `/orgs/${this.c.org}/repos`, { name, description: description.slice(0, 300), private: false, auto_init: true, has_wiki: false, has_projects: false });
    this.cache.delete(name);
    return { full_name: String(j.full_name), url: String(j.html_url), default_branch: String(j.default_branch ?? "main") };
  }
  /** One commit with every file on `branch` (created from the default branch when missing). */
  async commit(name: string, branch: string, files: GhFile[], message: string) {
    const t = await this.token(name);
    const r = `/repos/${this.c.org}/${name}`;
    const meta = await gh(this.c.api, t, "GET", r);
    const def = String(meta.default_branch ?? "main");
    let parent: string;
    let created = false;
    try { parent = (await gh(this.c.api, t, "GET", `${r}/git/ref/heads/${encodeURIComponent(branch)}`)).object.sha; } catch {
      parent = (await gh(this.c.api, t, "GET", `${r}/git/ref/heads/${encodeURIComponent(def)}`)).object.sha;
      created = true;
    }
    const baseTree = (await gh(this.c.api, t, "GET", `${r}/git/commits/${parent}`)).tree.sha;
    const tree = await gh(this.c.api, t, "POST", `${r}/git/trees`, { base_tree: baseTree, tree: files.map((f) => ({ path: f.path, mode: "100644", type: "blob", content: f.content })) });
    return { tree: String(tree.sha), parent, created, def, finish: async (fullMessage: string) => {
      const c = await gh(this.c.api, t, "POST", `${r}/git/commits`, { message: fullMessage, tree: tree.sha, parents: [parent] });
      if (created) await gh(this.c.api, t, "POST", `${r}/git/refs`, { ref: `refs/heads/${branch}`, sha: c.sha });
      else await gh(this.c.api, t, "PATCH", `${r}/git/refs/heads/${encodeURIComponent(branch)}`, { sha: c.sha, force: false });
      return { sha: String(c.sha), url: `https://github.com/${this.c.org}/${name}/commit/${c.sha}` };
    } };
  }
  async openPr(name: string, head: string, base: string | null, title: string, body: string) {
    const t = await this.token(name);
    const b = base ?? String((await gh(this.c.api, t, "GET", `/repos/${this.c.org}/${name}`)).default_branch);
    const j = await gh(this.c.api, t, "POST", `/repos/${this.c.org}/${name}/pulls`, { head, base: b, title: title.slice(0, 256), body: body.slice(0, 60_000) });
    return { number: Number(j.number), url: String(j.html_url) };
  }
}

/** The agent's repo names: its handle, then handle-2 .. handle-N (max_repos). */
export const repoNames = (handle: string, n: number) => Array.from({ length: n }, (_, i) => (i === 0 ? handle : `${handle}-${i + 1}`).replace(/_/g, "-"));
