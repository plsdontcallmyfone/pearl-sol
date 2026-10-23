// Who the writer pushes as. Two modes, one config switch (docs/AGENT-DUMP.md "Switching to the GitHub App"):
//   DUMP_AUTH=token  DUMP_TOKEN_FILE=/etc/lode/keys/agent-dump/github.token   (org owner token for now)
//   DUMP_AUTH=app    DUMP_APP_ID=... DUMP_APP_INSTALLATION_ID=... DUMP_APP_KEY_FILE=/etc/lode/keys/agent-dump/app.pem
// Tokens are read from files at use time and handed to git through the environment (never argv,
// never logged, never written to the repo or the state file).
import { createSign } from "node:crypto";
import { readFileSync } from "node:fs";

export interface Identity { name: string; email: string }
export interface Auth { token(): Promise<string>; identity(): Promise<Identity>; mode: string }

const b64url = (b: Buffer | string) => Buffer.from(b).toString("base64").replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");

async function gh(token: string, path: string, init: RequestInit = {}, scheme = "token"): Promise<any> {
  const r = await fetch(`https://api.github.com${path}`, { ...init, headers: { authorization: `${scheme} ${token}`, accept: "application/vnd.github+json", "x-github-api-version": "2022-11-28", "user-agent": "cadre-agent-dump", ...(init.headers ?? {}) }, signal: AbortSignal.timeout(30_000) });
  const body = await r.json().catch(() => null);
  if (!r.ok) throw new Error(`GitHub ${init.method ?? "GET"} ${path}: ${r.status} ${body?.message ?? ""}`);
  return body;
}

export function tokenAuth(file: string, env: NodeJS.ProcessEnv = process.env): Auth {
  const read = () => readFileSync(file, "utf8").trim();
  let who: Identity | null = null;
  return {
    mode: "token",
    token: async () => read(),
    identity: async () => {
      if (env.DUMP_GIT_NAME && env.DUMP_GIT_EMAIL) return { name: env.DUMP_GIT_NAME, email: env.DUMP_GIT_EMAIL };
      if (!who) {
        const u = await gh(read(), "/user");
        who = { name: `${u.login} (cadre agent-dump writer)`, email: `${u.id}+${u.login}@users.noreply.github.com` };
      }
      return who;
    },
  };
}

export function appAuth(env: NodeJS.ProcessEnv = process.env): Auth {
  const appId = env.DUMP_APP_ID, inst = env.DUMP_APP_INSTALLATION_ID, keyFile = env.DUMP_APP_KEY_FILE;
  if (!appId || !inst || !keyFile) throw new Error("DUMP_AUTH=app needs DUMP_APP_ID, DUMP_APP_INSTALLATION_ID and DUMP_APP_KEY_FILE");
  let cached: { token: string; exp: number } | null = null;
  const jwt = () => {
    const now = Math.floor(Date.now() / 1000);
    const head = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
    const body = b64url(JSON.stringify({ iat: now - 60, exp: now + 540, iss: appId }));
    const sig = createSign("RSA-SHA256").update(`${head}.${body}`).sign(readFileSync(keyFile, "utf8"));
    return `${head}.${body}.${b64url(sig)}`;
  };
  return {
    mode: "app",
    token: async () => {
      if (cached && cached.exp - Date.now() > 5 * 60_000) return cached.token;
      const r = await gh(jwt(), `/app/installations/${inst}/access_tokens`, { method: "POST" }, "Bearer");
      cached = { token: r.token, exp: Date.parse(r.expires_at) };
      return cached.token;
    },
    identity: async () => {
      if (env.DUMP_GIT_NAME && env.DUMP_GIT_EMAIL) return { name: env.DUMP_GIT_NAME, email: env.DUMP_GIT_EMAIL };
      const slug = env.DUMP_APP_SLUG ?? "cadre";
      const botId = env.DUMP_APP_BOT_USER_ID;
      return { name: `${slug}[bot]`, email: botId ? `${botId}+${slug}[bot]@users.noreply.github.com` : `${slug}[bot]@users.noreply.github.com` };
    },
  };
}

export function authFromEnv(env: NodeJS.ProcessEnv = process.env): Auth {
  const mode = env.DUMP_AUTH ?? "token";
  if (mode === "app") return appAuth(env);
  if (mode === "token") return tokenAuth(env.DUMP_TOKEN_FILE ?? "/etc/lode/keys/agent-dump/github.token", env);
  throw new Error(`DUMP_AUTH is token or app, not ${mode}`);
}

export { gh };
