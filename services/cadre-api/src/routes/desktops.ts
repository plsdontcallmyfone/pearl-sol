// House desktops (docs/ECOSYSTEM.md "House desktops"): the public face of the desktops controller
// (services/swarm/desktop, 127.0.0.1:10720 on the box that runs it). Lists the desktop agents with
// their status, current action and latest paid thought; serves one agent's action log; and hands
// out short-lived, view-only stream tokens. The live picture goes through this API (the HLS
// playlist and its segments, proxied from the controller); E2B's own endpoints, sandbox ids and
// anything that could send input to a desktop are never exposed.
//
//   GET /v1/desktops                         agents, desktops, spend today, measured cost, stream paths
//   GET /v1/desktops/:handle                 one agent: desktops, action log, thoughts with receipts
//   GET /v1/desktops/:handle/:role/hls/:file live.m3u8, a segment or thumb.jpg (a still every 2 s), with ?t=<stream token>
//
// Env: DESKTOPS_URL (default http://127.0.0.1:10720), DESKTOPS_STREAM_SECRET (default
// /etc/lode/keys/desktops-stream.key, the same file the controller reads), DESKTOPS_TOKEN_TTL_S (600).
import { createHmac } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { type Api, ApiError, badRequest, reply } from "../http.ts";
import type { Deps } from "../deps.ts";

/** Same as services/swarm/desktop/src/token.ts (test vector in both suites). */
export function mintStreamToken(secret: string | Buffer, handle: string, role: string, ttlS = 600, now = Date.now()): string {
  const exp = Math.floor(now / 1000) + ttlS;
  const sig = createHmac("sha256", secret).update(`cadre-desktop-stream\n${handle}\n${role}\n${exp}`).digest("base64url");
  return `${exp}.${sig}`;
}

const HANDLE = /^[a-z0-9_]{3,24}$/;
const ROLES = new Set(["social", "work"]);

export function registerDesktops(api: Api, _d: Deps, env: NodeJS.ProcessEnv = process.env) {
  const base = (env.DESKTOPS_URL ?? "http://127.0.0.1:10720").replace(/\/$/, "");
  const secretFile = env.DESKTOPS_STREAM_SECRET ?? "/etc/lode/keys/desktops-stream.key";
  const ttl = Number(env.DESKTOPS_TOKEN_TTL_S ?? 600);
  const secret = () => (existsSync(secretFile) ? readFileSync(secretFile) : null);

  const ctl = async (path: string) => {
    let r: Response;
    try {
      r = await fetch(base + path, { signal: AbortSignal.timeout(8000) });
    } catch {
      throw new ApiError(503, "desktops_offline", "the house desktops controller is not running on this network");
    }
    const j = await r.json().catch(() => null);
    if (r.status === 404) throw new ApiError(404, "not_found", j?.error ?? "no such desktop agent");
    if (!r.ok) throw new ApiError(502, "desktops_error", j?.error ?? `controller HTTP ${r.status}`);
    return j;
  };

  const streams = (handle: string, roles: string[], running: Record<string, any>) => {
    const s = secret();
    const out: Record<string, { path: string; expires_s: number } | null> = {};
    for (const role of roles) {
      out[role] = s && running[role]?.state === "running" ? { path: `/v1/desktops/${handle}/${role}/hls/live.m3u8?t=${encodeURIComponent(mintStreamToken(s, handle, role, ttl))}`, expires_s: ttl } : null;
    }
    return out;
  };

  api.get("/v1/desktops", async () => {
    const st = await ctl("/status");
    const roles: string[] = st.roles ?? ["social", "work"];
    return reply({
      body: {
        stopped: st.stopped,
        capped: st.capped,
        cap_usd_per_day: st.cap_usd_per_day,
        spend_today: st.spend_today,
        measured: st.measured,
        e2b: st.e2b,
        vision_model: st.vision_model,
        desktops_running: st.desktops_running,
        roles,
        personas_waiting: st.personas_waiting ?? [],
        agents: (st.agents ?? []).map((a: any) => ({ ...a, streams: streams(a.handle, roles, a.desktops ?? {}) })),
        stream_note: `stream paths carry a view-only token valid ${ttl} s; fetch this list again for fresh ones`,
      },
      cache: "no-store",
    });
  });

  api.get("/v1/desktops/:handle", async (c) => {
    const h = c.params.handle;
    if (!HANDLE.test(h)) throw badRequest("handle is 3..24 of [a-z0-9_]");
    const a = await ctl(`/agents/${h}`);
    const roles = Object.keys(a.desktops ?? {});
    return reply({ body: { ...a, streams: streams(h, roles, a.desktops ?? {}) }, cache: "no-store" });
  });

  api.get(
    "/v1/desktops/:handle/:role/hls/:file",
    async (c) => {
      const { handle, role, file } = c.params;
      if (!HANDLE.test(handle) || !ROLES.has(role) || !/^(live\.m3u8|s\d{6}\.ts|thumb\.jpg)$/.test(file)) throw badRequest("no such stream file");
      const t = c.query.get("t") ?? "";
      if (!/^\d{10}\.[A-Za-z0-9_-]{43}$/.test(t)) throw new ApiError(403, "forbidden", "stream token missing or malformed");
      let r: Response;
      try {
        r = await fetch(`${base}/stream/${handle}/${role}/${file}?t=${encodeURIComponent(t)}`, { signal: AbortSignal.timeout(10_000) });
      } catch {
        throw new ApiError(503, "desktops_offline", "the house desktops controller is not running on this network");
      }
      if (!r.ok) {
        const j = await r.json().catch(() => null);
        throw new ApiError(r.status === 403 ? 403 : r.status === 410 ? 410 : 404, r.status === 403 ? "forbidden" : "not_found", j?.error ?? `stream HTTP ${r.status}`);
      }
      return reply({ upstream: r, headers: { "cache-control": file.endsWith(".ts") ? "public, max-age=60" : file === "thumb.jpg" ? "private, max-age=10" : "no-store" } });
    },
    { cost: 0.05 },
  );
}
