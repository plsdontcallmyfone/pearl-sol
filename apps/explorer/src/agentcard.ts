// Agent cards (owner decision 2026-10-18, REPUTATION.md 5.1 "Shown, not computed").
//
// `RegisterAgent.uri` may point at an A2A-style agent card (JSON, the
// `/.well-known/agent-card.json` shape: name, description, url / interfaces, version,
// capabilities, skills). The explorer fetches it to show the agent's endpoints and to
// tell live profiles from abandoned ones. The card is display only: nothing in it feeds a
// reputation figure, which come from chain state alone (profile.ts).
//
// Probe rules: http(s) only, a timeout, a size cap, JSON only, and no fetch to loopback,
// private, link-local or unique-local addresses (checked on the resolved address) unless
// CARD_ALLOW_PRIVATE=1 (tests, local stacks). A redirect is not followed. The socket's own
// DNS lookup re-checks the answer, so a name that rebinds to a private address after the
// first check is refused too; IPv6 forms that embed an IPv4 address are checked by that IPv4.
//
// Env knobs (milliseconds):
//   CARD_PROBE_MS   600000     re-probe a card at most this often
//   CARD_TIMEOUT_MS 5000       per fetch
//   CARD_STALE_MS   86400000   a card whose last good probe is older than this is stale
//   AGENT_ACTIVE_MS 604800000  a delivery within this window keeps a profile listed

import { lookup } from "node:dns/promises";
import { isIP } from "node:net";
import { get as httpGet } from "node:http";
import { get as httpsGet } from "node:https";

export const CARD_MAX_BYTES = 64 * 1024;

const envMs = (k: string, d: number) => {
  const v = Number(process.env[k]);
  return Number.isFinite(v) && v > 0 ? v : d;
};

export const cardConfig = () => ({
  probeMs: envMs("CARD_PROBE_MS", 600_000),
  timeoutMs: envMs("CARD_TIMEOUT_MS", 5_000),
  staleMs: envMs("CARD_STALE_MS", 86_400_000),
  activeMs: envMs("AGENT_ACTIVE_MS", 604_800_000),
  allowPrivate: process.env.CARD_ALLOW_PRIVATE === "1",
});

export interface Card {
  name: string | null;
  description: string | null;
  url: string | null;
  version: string | null;
  protocol_version: string | null;
  provider: string | null;
  interfaces: { url: string; transport: string | null }[];
  capabilities: string[];
  skills: { id: string | null; name: string | null; description: string | null }[];
  signed: boolean;
}

export interface CardState {
  uri: string;
  checked_at: number | null;
  ok: boolean;
  error: string | null;
  last_ok_at: number | null;
  card: Card | null;
}

export type CardHealth = "ok" | "failing" | "stale" | "none";

const str = (v: unknown, max = 300) => (typeof v === "string" && v.trim() ? v.trim().slice(0, max) : null);

/** Keep a sanitized subset of an A2A agent card. Throws if it is not an object with a name. */
export function parseCard(v: unknown): Card {
  if (!v || typeof v !== "object" || Array.isArray(v)) throw new Error("card is not a JSON object");
  const o = v as Record<string, any>;
  const name = str(o.name, 120);
  if (!name) throw new Error("card has no name");
  const ifaces: { url: string; transport: string | null }[] = [];
  const pushIface = (x: any) => {
    const u = str(x?.url, 400);
    if (u && /^https?:\/\//i.test(u)) ifaces.push({ url: u, transport: str(x?.transport ?? x?.protocolBinding, 40) });
  };
  for (const k of ["interfaces", "supportedInterfaces", "additionalInterfaces"]) if (Array.isArray(o[k])) o[k].slice(0, 8).forEach(pushIface);
  const caps = o.capabilities && typeof o.capabilities === "object" ? Object.entries(o.capabilities).filter(([, x]) => x === true).map(([k]) => k.slice(0, 40)).slice(0, 12) : [];
  const skills = Array.isArray(o.skills)
    ? o.skills.slice(0, 20).map((s: any) => ({ id: str(s?.id, 80), name: str(s?.name, 120), description: str(s?.description, 300) }))
    : [];
  const url = str(o.url, 400);
  return {
    name,
    description: str(o.description, 600),
    url: url && /^https?:\/\//i.test(url) ? url : null,
    version: str(o.version, 40),
    protocol_version: str(o.protocolVersion ?? o.protocol_version, 40),
    provider: str(o.provider?.organization ?? o.provider?.name, 120),
    interfaces: ifaces,
    capabilities: caps,
    skills,
    signed: Array.isArray(o.signatures) && o.signatures.length > 0,
  };
}

/** Parse an IPv6 literal (any textual form, embedded IPv4 allowed) into 16 bytes; null if not IPv6. */
export function ipv6Bytes(ip: string): number[] | null {
  if (isIP(ip) !== 6) return null;
  let s = ip.toLowerCase().replace(/%.*$/, "");
  let tail: number[] = [];
  const v4 = s.match(/(\d+\.\d+\.\d+\.\d+)$/);
  if (v4) {
    tail = v4[1].split(".").map(Number);
    s = s.slice(0, -v4[1].length) + "0:0";
  }
  const [head, rest] = s.includes("::") ? s.split("::") : [s, null];
  const hs = head ? head.split(":") : [];
  const rs = rest === null ? [] : rest ? rest.split(":") : [];
  const fill = rest === null ? 0 : 8 - hs.length - rs.length;
  const groups = [...hs, ...Array(fill).fill("0"), ...rs].map((g) => parseInt(g || "0", 16));
  if (groups.length !== 8 || groups.some((g) => !Number.isFinite(g) || g < 0 || g > 0xffff)) return null;
  const b = groups.flatMap((g) => [g >> 8, g & 0xff]);
  if (tail.length) b.splice(12, 4, ...tail);
  return b;
}

function privateV4(o: number[]): boolean {
  const [a, b, c] = o;
  return (
    a === 0 || a === 10 || a === 127 || a >= 224 ||
    (a === 100 && b >= 64 && b <= 127) || // CGNAT
    (a === 169 && b === 254) || // link-local, cloud metadata
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168) ||
    (a === 192 && b === 0 && (c === 0 || c === 2)) || // IETF protocol assignments, TEST-NET-1
    (a === 198 && (b === 18 || b === 19)) || // benchmarking
    (a === 198 && b === 51 && c === 100) ||
    (a === 203 && b === 0 && c === 113)
  );
}

/**
 * True for loopback, private, link-local, CGNAT, unique-local, multicast, reserved and
 * documentation addresses, and for every IPv6 form that carries an IPv4 address inside
 * (mapped `::ffff:a.b.c.d` in any spelling, compatible `::a.b.c.d`, NAT64 64:ff9b::/96,
 * 6to4 2002::/16) when the IPv4 inside is one of those. Anything that is not an IP is private.
 */
export function isPrivateAddress(ip: string): boolean {
  const v = isIP(ip);
  if (v === 4) return privateV4(ip.split(".").map(Number));
  if (v !== 6) return true;
  const b = ipv6Bytes(ip);
  if (!b) return true;
  const zero = (from: number, to: number) => b.slice(from, to).every((x) => x === 0);
  if (zero(0, 10) && b[10] === 0xff && b[11] === 0xff) return privateV4(b.slice(12)); // mapped
  if (zero(0, 12)) return true; // ::, ::1, IPv4-compatible (deprecated): never probed
  if (b[0] === 0x00 && b[1] === 0x64 && b[2] === 0xff && b[3] === 0x9b) return zero(4, 12) ? privateV4(b.slice(12)) : true; // NAT64 64:ff9b::/96, local-use 64:ff9b:1::/48
  if (b[0] === 0x20 && b[1] === 0x02) return privateV4(b.slice(2, 6)); // 6to4
  if (b[0] === 0x20 && b[1] === 0x01 && b[2] === 0x00 && b[3] === 0x00) return true; // Teredo
  if (b[0] === 0x20 && b[1] === 0x01 && b[2] === 0x0d && b[3] === 0xb8) return true; // documentation
  if (b[0] === 0x01 && b[1] === 0x00 && zero(2, 8)) return true; // discard-only 100::/64
  return (b[0] & 0xfe) === 0xfc || (b[0] === 0xfe && (b[1] & 0xc0) !== 0) || b[0] === 0xff; // fc00::/7, fe80::/10 + fec0::/10, multicast
}

type LookupCb = (err: NodeJS.ErrnoException | null, address: string | { address: string; family: number }[], family?: number) => void;

/** dns.lookup for the socket itself: refuses private answers at connect time (no DNS-rebinding window between check and fetch). */
export function guardedLookup(hostname: string, options: { all?: boolean } | number | LookupCb, cb?: LookupCb): void {
  const done = (typeof options === "function" ? options : cb) as LookupCb;
  const all = typeof options === "object" && options !== null && !!options.all;
  lookup(hostname, { all: true }).then(
    (addrs) => {
      if (addrs.length === 0 || addrs.some((a) => isPrivateAddress(a.address))) {
        const e = new Error("host resolves to a private or loopback address (not probed)") as NodeJS.ErrnoException;
        e.code = "EPRIVATE";
        return done(e, all ? [] : "", 4);
      }
      if (all) return done(null, addrs);
      done(null, addrs[0].address, addrs[0].family);
    },
    (e) => done(e, all ? [] : "", 4),
  );
}

/** GET `u` over node:http(s), no redirects, the guarded lookup unless private hosts are allowed. */
function getOnce(u: URL, cfg: { timeoutMs: number; allowPrivate: boolean }): Promise<{ status: number; headers: Record<string, string | string[] | undefined>; body: AsyncIterable<Buffer>; destroy: () => void }> {
  return new Promise((resolve, reject) => {
    const mod = u.protocol === "https:" ? httpsGet : httpGet;
    const req = mod(u, { headers: { accept: "application/json" }, timeout: cfg.timeoutMs, signal: AbortSignal.timeout(cfg.timeoutMs), ...(cfg.allowPrivate ? {} : { lookup: guardedLookup as any }) }, (res) =>
      resolve({ status: res.statusCode ?? 0, headers: res.headers, body: res, destroy: () => res.destroy() }),
    );
    req.on("timeout", () => req.destroy(Object.assign(new Error("timeout"), { name: "TimeoutError" })));
    req.on("error", reject);
  });
}

/** Fetch and parse one card. Errors are returned as text, never thrown. */
export async function fetchCard(uri: string, cfg = cardConfig()): Promise<{ ok: true; card: Card } | { ok: false; error: string }> {
  let u: URL;
  try {
    u = new URL(uri);
  } catch {
    return { ok: false, error: "uri is not a URL" };
  }
  if (u.protocol !== "https:" && u.protocol !== "http:") return { ok: false, error: `${u.protocol.replace(":", "")} URIs are not probed (http and https only)` };
  if (u.username || u.password) return { ok: false, error: "URIs with credentials are not probed" };
  if (!cfg.allowPrivate) {
    const host = u.hostname.replace(/^\[|\]$/g, "");
    let addrs: string[];
    try {
      addrs = isIP(host) ? [host] : (await lookup(host, { all: true })).map((a) => a.address);
    } catch {
      return { ok: false, error: "host does not resolve" };
    }
    if (addrs.length === 0 || addrs.some(isPrivateAddress)) return { ok: false, error: "host resolves to a private or loopback address (not probed)" };
  }
  try {
    const r = await getOnce(u, cfg);
    if (r.status >= 300 && r.status < 400) {
      r.destroy();
      return { ok: false, error: `redirect (HTTP ${r.status}) not followed` };
    }
    if (r.status < 200 || r.status >= 300) {
      r.destroy();
      return { ok: false, error: `HTTP ${r.status}` };
    }
    const len = Number(r.headers["content-length"] ?? 0);
    if (len > CARD_MAX_BYTES) {
      r.destroy();
      return { ok: false, error: `card larger than ${CARD_MAX_BYTES} bytes` };
    }
    const chunks: Buffer[] = [];
    let n = 0;
    for await (const value of r.body) {
      n += value.length;
      if (n > CARD_MAX_BYTES) {
        r.destroy();
        return { ok: false, error: `card larger than ${CARD_MAX_BYTES} bytes` };
      }
      chunks.push(value);
    }
    let v: unknown;
    try {
      v = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    } catch {
      return { ok: false, error: "not JSON" };
    }
    return { ok: true, card: parseCard(v) };
  } catch (e) {
    const err = e as NodeJS.ErrnoException;
    if (err.code === "EPRIVATE") return { ok: false, error: err.message };
    const m = err.name === "TimeoutError" || err.name === "AbortError" ? `no answer within ${cfg.timeoutMs} ms` : err.message;
    return { ok: false, error: m };
  }
}

/** Probe results per account, re-probed at most every `probeMs`. In memory only. */
export class CardProber {
  states = new Map<string, CardState>();
  private inflight = new Map<string, Promise<CardState>>();
  private cfg: ReturnType<typeof cardConfig>;
  private fetcher: typeof fetchCard;
  private now: () => number;
  constructor(cfg = cardConfig(), fetcher: typeof fetchCard = fetchCard, now: () => number = Date.now) {
    this.cfg = cfg;
    this.fetcher = fetcher;
    this.now = now;
  }

  get(account: string): CardState | null {
    return this.states.get(account) ?? null;
  }

  /** Probe `uri` for `account` if it changed or the last probe is older than probeMs. */
  async probe(account: string, uri: string | null): Promise<CardState | null> {
    if (!uri) {
      this.states.delete(account);
      return null;
    }
    const prev = this.states.get(account);
    if (prev && prev.uri === uri && prev.checked_at !== null && this.now() - prev.checked_at < this.cfg.probeMs) return prev;
    const key = `${account} ${uri}`;
    const running = this.inflight.get(key);
    if (running) return running;
    const p = (async () => {
      const r = await this.fetcher(uri, this.cfg);
      const t = this.now();
      const same = prev && prev.uri === uri;
      const st: CardState = r.ok
        ? { uri, checked_at: t, ok: true, error: null, last_ok_at: t, card: r.card }
        : { uri, checked_at: t, ok: false, error: r.error, last_ok_at: same ? prev!.last_ok_at : null, card: same ? prev!.card : null };
      this.states.set(account, st);
      this.inflight.delete(key);
      return st;
    })();
    this.inflight.set(key, p);
    return p;
  }

  health(st: CardState | null): CardHealth {
    if (!st) return "none";
    if (st.last_ok_at === null) return st.checked_at === null ? "none" : "failing";
    if (this.now() - st.last_ok_at > this.cfg.staleMs) return "stale";
    return st.ok ? "ok" : "failing";
  }

  /** A profile is listed when its card answered within the stale window or it delivered within the active window. */
  listed(st: CardState | null, lastDeliveryMs: number | null): { listed: boolean; reason: string } {
    const h = this.health(st);
    const fresh = st?.last_ok_at !== null && st?.last_ok_at !== undefined && this.now() - st.last_ok_at <= this.cfg.staleMs;
    const active = lastDeliveryMs !== null && this.now() - lastDeliveryMs <= this.cfg.activeMs;
    if (fresh) return { listed: true, reason: "agent card answered within the stale window" };
    if (active) return { listed: true, reason: "delivered work within the active window" };
    return { listed: false, reason: h === "none" ? "no agent card and no recent delivery" : `agent card ${h} and no recent delivery` };
  }
}
