// Outbound HTTP for the toolbelt with the network guard (docs/TOOLBELT.md 7): only http(s) on ports
// 80/443, the host's addresses resolved at connect time by our own lookup (so a DNS answer cannot
// change between the check and the connection), and every private, loopback, link-local, CGNAT,
// multicast, documentation or cloud-metadata address refused. Redirects are followed by hand (at
// most 5), each hop checked again. Bodies are cut at a byte cap and a time cap. Nothing secret is
// ever added to a request by this module; callers pass headers explicitly.
import { lookup as dnsLookup } from "node:dns";
import { request as httpRequest, type IncomingMessage } from "node:http";
import { request as httpsRequest } from "node:https";
import { isIP } from "node:net";

export class NetRefused extends Error {}

function v4(ip: string): number[] { return ip.split(".").map(Number); }

/** true for any address an agent must never reach */
export function isBlockedIp(ip: string): boolean {
  const fam = isIP(ip);
  if (fam === 4) {
    const [a, b] = v4(ip);
    return a === 0 || a === 10 || a === 127 || (a === 100 && b >= 64 && b <= 127) || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31)
      || (a === 192 && b === 168) || (a === 192 && b === 0) || (a === 198 && (b === 18 || b === 19)) || (a === 198 && b === 51) || (a === 203 && b === 0) || a >= 224;
  }
  if (fam === 6) {
    const s = ip.toLowerCase();
    if (s === "::" || s === "::1") return true;
    const m = /^::ffff:(\d+\.\d+\.\d+\.\d+)$/.exec(s);
    if (m) return isBlockedIp(m[1]);
    return /^(fc|fd|fe8|fe9|fea|feb|ff)/.test(s) || s.startsWith("64:ff9b:") || s.startsWith("2001:db8") || s.startsWith("100::");
  }
  return true;
}

/** Hosts the runtime itself refuses by name whatever they resolve to. */
const BLOCKED_NAMES = [/^localhost$/i, /\.local$/i, /\.internal$/i, /^metadata(\.google)?/i];

export interface FetchOpts {
  method?: "GET" | "POST";
  headers?: Record<string, string>;
  body?: string;
  maxBytes: number;
  timeoutMs: number;
  maxRedirects?: number;
  /** when set, every hop's host must be one of these (exact, lowercase) */
  allowHosts?: string[];
  /** test hook: allow 127.0.0.1 for the fakes */
  allowLoopbackForTests?: boolean;
}

export interface FetchResult { url: string; status: number; contentType: string; body: Buffer; truncated: boolean; redirects: string[]; ip: string }

function guardedLookup(allowLoopback: boolean, seen: { ip: string }) {
  return (host: string, opts: any, cb: any) => {
    dnsLookup(host, { all: true, verbatim: true }, (err, addrs) => {
      if (err) return cb(err);
      const list = (addrs as any[]).map((a) => a.address as string);
      const bad = list.find((ip) => isBlockedIp(ip) && !(allowLoopback && (ip === "127.0.0.1" || ip === "::1")));
      if (!list.length || bad) return cb(new NetRefused(`refused: ${host} resolves to a private or reserved address`));
      const pick = (addrs as any[])[0];
      seen.ip = pick.address;
      if (opts?.all) return cb(null, [{ address: pick.address, family: pick.family }]);
      cb(null, pick.address, pick.family);
    });
  };
}

export function checkUrl(raw: string, o: Pick<FetchOpts, "allowHosts" | "allowLoopbackForTests">): URL {
  let u: URL;
  try { u = new URL(raw); } catch { throw new NetRefused("not a URL"); }
  if (u.protocol !== "https:" && u.protocol !== "http:") throw new NetRefused("only http and https URLs");
  if (u.username || u.password) throw new NetRefused("URLs with credentials are refused");
  const port = u.port ? Number(u.port) : u.protocol === "https:" ? 443 : 80;
  const test = o.allowLoopbackForTests && (u.hostname === "127.0.0.1");
  if (!test && port !== 443 && port !== 80) throw new NetRefused("only ports 80 and 443");
  const host = u.hostname.toLowerCase().replace(/\.$/, "");
  if (!test && (isIP(host.replace(/^\[|\]$/g, "")) || BLOCKED_NAMES.some((r) => r.test(host)))) throw new NetRefused("IP-literal and internal host names are refused; use a public host name");
  if (o.allowHosts && !o.allowHosts.includes(host)) throw new NetRefused(`host ${host} is not on this agent's allowlist`);
  return u;
}

function once(u: URL, o: FetchOpts, seen: { ip: string }): Promise<{ res: IncomingMessage; body: Buffer; truncated: boolean }> {
  return new Promise((resolve, reject) => {
    const mod = u.protocol === "https:" ? httpsRequest : httpRequest;
    const req = mod(u, {
      method: o.method ?? "GET",
      headers: { "user-agent": "cadre-toolbelt/0.1 (+https://cadre.64-95-12-116.sslip.io)", accept: "*/*", ...(o.headers ?? {}) },
      lookup: guardedLookup(!!o.allowLoopbackForTests, seen) as any,
      timeout: o.timeoutMs,
    }, (res) => {
      const parts: Buffer[] = [];
      let n = 0;
      let truncated = false;
      res.on("data", (c: Buffer) => {
        if (n >= o.maxBytes) { truncated = true; res.destroy(); return; }
        const take = c.subarray(0, Math.max(0, o.maxBytes - n));
        parts.push(take);
        n += take.length;
        if (take.length < c.length) { truncated = true; res.destroy(); }
      });
      const done = () => resolve({ res, body: Buffer.concat(parts), truncated });
      res.on("end", done);
      res.on("close", done);
      res.on("error", (e) => (truncated ? done() : reject(e)));
    });
    req.on("timeout", () => req.destroy(new Error(`timed out after ${o.timeoutMs} ms`)));
    req.on("error", reject);
    if (o.body) req.write(o.body);
    req.end();
  });
}

export async function guardedFetch(raw: string, o: FetchOpts): Promise<FetchResult> {
  let u = checkUrl(raw, o);
  const redirects: string[] = [];
  const seen = { ip: "" };
  const deadline = Date.now() + o.timeoutMs;
  for (let hop = 0; ; hop++) {
    const left = deadline - Date.now();
    if (left <= 0) throw new Error(`timed out after ${o.timeoutMs} ms`);
    const { res, body, truncated } = await once(u, { ...o, timeoutMs: left }, seen);
    const st = res.statusCode ?? 0;
    if (st >= 300 && st < 400 && res.headers.location) {
      if (hop >= (o.maxRedirects ?? 5)) throw new NetRefused("too many redirects");
      const next = new URL(String(res.headers.location), u).toString();
      redirects.push(next);
      u = checkUrl(next, o);
      if (o.method === "POST" && st !== 307 && st !== 308) o = { ...o, method: "GET", body: undefined };
      continue;
    }
    return { url: u.toString(), status: st, contentType: String(res.headers["content-type"] ?? ""), body, truncated, redirects, ip: seen.ip };
  }
}

/** HTML to readable text: drops scripts, styles and tags, keeps link targets as [n] footnotes. */
export function htmlToText(html: string, base: string, maxLinks = 40): { title: string; text: string; links: string[] } {
  const title = (/<title[^>]*>([\s\S]*?)<\/title>/i.exec(html)?.[1] ?? "").replace(/\s+/g, " ").trim().slice(0, 200);
  let h = html.replace(/<(script|style|noscript|svg|head|iframe|template)[\s\S]*?<\/\1>/gi, " ").replace(/<!--[\s\S]*?-->/g, " ");
  const links: string[] = [];
  h = h.replace(/<a\b[^>]*href\s*=\s*["']([^"']+)["'][^>]*>([\s\S]*?)<\/a>/gi, (_m, href, inner) => {
    let abs = "";
    try { abs = new URL(href, base).toString(); } catch { /* skip */ }
    if (abs.startsWith("http") && links.length < maxLinks && !links.includes(abs)) links.push(abs);
    const i = links.indexOf(abs);
    return `${inner}${i >= 0 ? ` [${i + 1}]` : ""}`;
  });
  h = h.replace(/<(br|\/p|\/div|\/li|\/h[1-6]|\/tr|\/section|\/article)\b[^>]*>/gi, "\n").replace(/<li\b[^>]*>/gi, "\n- ").replace(/<[^>]+>/g, " ");
  const ent: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ", "#39": "'" };
  h = h.replace(/&(#x?[0-9a-f]+|[a-z0-9]+);/gi, (m, e) => {
    if (ent[e.toLowerCase()]) return ent[e.toLowerCase()];
    if (/^#x/i.test(e)) return String.fromCodePoint(parseInt(e.slice(2), 16) || 32);
    if (/^#/.test(e)) return String.fromCodePoint(Number(e.slice(1)) || 32);
    return m;
  });
  const text = h.split("\n").map((l) => l.replace(/[ \t\f\v]+/g, " ").trim()).filter(Boolean).join("\n");
  return { title, text, links };
}
