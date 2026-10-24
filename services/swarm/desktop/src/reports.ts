// Work outputs of the house agents: one JSON file per report under <stateDir>/reports/<handle>/,
// with its markdown body, the data it was built from, links to the on-chain records it names and a
// sha256 of the canonical body (the receipt a reader can check). The controller serves them on
// /agents/<handle> (the newest, in full) and /agents/<handle>/reports/<id>; cadre-api passes both
// through. Nothing here is posted to the town (owner 2026-10-23).
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export interface Report {
  id: string;
  agent: string;
  job: string;
  kind: string;
  title: string;
  created: string;
  summary: string;
  /** the readable report (markdown) */
  body_md: string;
  /** links to what the report names (market hires, txs, pages) */
  links: { label: string; url: string }[];
  /** what it cost and what proves it: vision USD, steps, chain txs, sources captured in the browser */
  receipts: Record<string, unknown>;
  /** the numbers it was built from (series, findings, issues) */
  data: Record<string, unknown>;
  /** an inline SVG chart when the report has a series */
  chart_svg?: string;
  /** sha256 of the canonical report without this field */
  sha256: string;
}

export const canon = (x: unknown): string => JSON.stringify(x, (_k, v) => (v && typeof v === "object" && !Array.isArray(v) ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, v[k]])) : typeof v === "bigint" ? v.toString() : v));
export const sha256 = (s: string | Buffer) => createHash("sha256").update(s).digest("hex");

export class ReportStore {
  dir: string;
  keep: number;
  private cache = new Map<string, Report[]>();
  private n = 0;
  constructor(dir: string, keep = 60) {
    this.dir = dir;
    this.keep = keep;
    mkdirSync(dir, { recursive: true });
  }

  /** Writes a report (atomically) and returns it with its id and sha256. */
  put(r: Omit<Report, "id" | "sha256" | "created"> & { created?: string }): Report {
    const created = r.created ?? new Date().toISOString();
    const id = `${r.agent}-${created.replace(/[-:TZ.]/g, "").slice(0, 14)}-${(++this.n).toString(36)}`;
    const body = { ...r, id, created };
    const full: Report = { ...body, sha256: sha256(canon(body)) } as Report;
    const d = join(this.dir, r.agent);
    mkdirSync(d, { recursive: true });
    const list = this.list(r.agent); // read (and cache) before the new file exists, so it is not listed twice
    const f = join(d, `${id}.json`);
    writeFileSync(`${f}.tmp`, JSON.stringify(full));
    renameSync(`${f}.tmp`, f);
    list.unshift(full);
    // keep the newest `keep` per agent on disk
    for (const old of list.splice(this.keep)) rmSync(join(d, `${old.id}.json`), { force: true });
    this.cache.set(r.agent, list);
    return full;
  }

  /** Newest first. */
  list(agent: string): Report[] {
    const hit = this.cache.get(agent);
    if (hit) return hit;
    const d = join(this.dir, agent);
    const out: Report[] = [];
    if (existsSync(d)) {
      for (const f of readdirSync(d).filter((x) => x.endsWith(".json"))) {
        try { out.push(JSON.parse(readFileSync(join(d, f), "utf8"))); } catch { /* skip a torn file */ }
      }
    }
    out.sort((a, b) => (a.created < b.created ? 1 : a.created > b.created ? -1 : 0));
    this.cache.set(agent, out);
    return out;
  }

  get(agent: string, id: string): Report | null { return this.list(agent).find((r) => r.id === id) ?? null; }

  /** Every agent's reports since `since` (ms), newest first. */
  since(agents: string[], since: number): Report[] {
    return agents.flatMap((a) => this.list(a).filter((r) => Date.parse(r.created) >= since)).sort((a, b) => (a.created < b.created ? 1 : -1));
  }

  /** A short card for status lines: no body. */
  static card(r: Report) { return { id: r.id, kind: r.kind, title: r.title, created: r.created, summary: r.summary, sha256: r.sha256, links: r.links }; }
}

/** Verifies a stored report's sha256 (the receipt). */
export function checkReport(r: Report): boolean {
  const { sha256: s, ...body } = r;
  return sha256(canon(body)) === s;
}

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** A small line chart (one series), plain SVG, readable in light and dark (currentColor). */
export function lineChart(points: { t: number; v: number }[], o: { title: string; unit?: string; w?: number; h?: number; threshold?: number }): string {
  const w = o.w ?? 560, h = o.h ?? 160, pad = 32;
  const pts = points.filter((p) => Number.isFinite(p.v));
  if (pts.length < 2) return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="40" role="img" aria-label="${esc(o.title)}"><text x="4" y="24" font-family="sans-serif" font-size="12" fill="currentColor">${esc(o.title)}: not enough points yet</text></svg>`;
  const t0 = pts[0]!.t, t1 = pts[pts.length - 1]!.t;
  const vs = pts.map((p) => p.v).concat(o.threshold !== undefined ? [o.threshold] : []);
  let lo = Math.min(...vs), hi = Math.max(...vs);
  if (hi === lo) { hi += 1; lo -= 1; }
  const x = (t: number) => pad + ((t - t0) / Math.max(1, t1 - t0)) * (w - 2 * pad);
  const y = (v: number) => h - pad + 8 - ((v - lo) / (hi - lo)) * (h - 2 * pad);
  const line = pts.map((p) => `${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`).join(" ");
  const fmt = (v: number) => (Math.abs(v) >= 1000 ? v.toFixed(0) : Math.abs(v) >= 10 ? v.toFixed(1) : v.toFixed(2));
  const hhmm = (t: number) => new Date(t).toISOString().slice(11, 16);
  const thr = o.threshold !== undefined ? `<line x1="${pad}" x2="${w - pad}" y1="${y(o.threshold).toFixed(1)}" y2="${y(o.threshold).toFixed(1)}" stroke="#d64545" stroke-dasharray="4 3"/><text x="${w - pad}" y="${(y(o.threshold) - 4).toFixed(1)}" text-anchor="end" font-family="sans-serif" font-size="10" fill="#d64545">limit ${fmt(o.threshold)}</text>` : "";
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" role="img" aria-label="${esc(o.title)}">`
    + `<text x="${pad}" y="14" font-family="sans-serif" font-size="12" fill="currentColor">${esc(o.title)}${o.unit ? ` (${esc(o.unit)})` : ""}</text>`
    + `<line x1="${pad}" x2="${w - pad}" y1="${h - pad + 8}" y2="${h - pad + 8}" stroke="currentColor" stroke-opacity="0.3"/>`
    + `<text x="4" y="${(y(hi) + 4).toFixed(1)}" font-family="sans-serif" font-size="10" fill="currentColor">${fmt(hi)}</text>`
    + `<text x="4" y="${(y(lo) + 4).toFixed(1)}" font-family="sans-serif" font-size="10" fill="currentColor">${fmt(lo)}</text>`
    + `<text x="${pad}" y="${h - 4}" font-family="sans-serif" font-size="10" fill="currentColor">${hhmm(t0)} UTC</text>`
    + `<text x="${w - pad}" y="${h - 4}" text-anchor="end" font-family="sans-serif" font-size="10" fill="currentColor">${hhmm(t1)} UTC</text>`
    + thr
    + `<polyline fill="none" stroke="#2f7ae5" stroke-width="2" points="${line}"/></svg>`;
}

/** A markdown table. */
export function mdTable(head: string[], rows: (string | number | null | undefined)[][]): string {
  const cell = (c: unknown) => String(c ?? "").replace(/\|/g, "\\|").replace(/\n/g, " ");
  return [`| ${head.map(cell).join(" | ")} |`, `| ${head.map(() => "---").join(" | ")} |`, ...rows.map((r) => `| ${r.map(cell).join(" | ")} |`)].join("\n");
}
