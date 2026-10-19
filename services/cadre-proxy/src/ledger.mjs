// Spend ledger and caps. Amounts are lamports (SOL base units) as charged by the gateway's
// receipts; rows are the gateway's unit (one token through every registered matrix).
// The ledger file is rewritten atomically (write + rename) after every recorded request.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

const KEEP_REQUESTS = 200;

export const day = (ms = Date.now()) => new Date(ms).toISOString().slice(0, 10);

/** Caps; null = no cap. All BigInt for lamports, Number for rows. */
export function parseCaps(o = {}) {
  const big = (v) => (v === undefined || v === null || v === "" ? null : BigInt(v));
  const num = (v) => (v === undefined || v === null || v === "" ? null : Number(v));
  return {
    requestLamports: big(o.requestLamports),
    dayLamports: big(o.dayLamports),
    totalLamports: big(o.totalLamports),
    requestRows: num(o.requestRows),
    dayRows: num(o.dayRows),
  };
}

export class Ledger {
  constructor(path, caps) {
    this.path = path;
    this.caps = caps;
    this.inflight = new Map(); // id -> {rows, lamports}
    this.seq = 0;
    this.data = { version: 1, total_lamports: "0", total_rows: 0, requests_count: 0, days: {}, requests: [] };
    if (path && existsSync(path)) this.data = { ...this.data, ...JSON.parse(readFileSync(path, "utf8")) };
  }

  today() {
    return this.data.days[day()] ?? { lamports: "0", rows: 0, requests: 0 };
  }

  pending() {
    let rows = 0;
    let lamports = 0n;
    for (const r of this.inflight.values()) {
      rows += r.rows;
      lamports += r.lamports ?? 0n;
    }
    return { rows, lamports };
  }

  /**
   * Pre-check an estimate (rows, lamports or null when the price is unknown). Returns
   * {ok, id} or {ok: false, message}. In-flight estimates count against the caps so
   * concurrent requests cannot overspend.
   */
  reserve(rows, lamports) {
    const c = this.caps;
    const t = this.today();
    const p = this.pending();
    const lamportCaps = c.requestLamports !== null || c.dayLamports !== null || c.totalLamports !== null;
    if (lamportCaps && lamports === null) {
      return { ok: false, message: "the gateway publishes no lamports_per_row for this model, so a lamport spend cap cannot be checked; refusing (set only row caps to proceed)" };
    }
    if (c.requestRows !== null && rows > c.requestRows) return { ok: false, message: `estimated ${rows} rows exceed the per-request cap of ${c.requestRows}` };
    if (c.dayRows !== null && t.rows + p.rows + rows > c.dayRows) return { ok: false, message: `estimated ${rows} rows would pass the daily cap of ${c.dayRows} (used ${t.rows}, in flight ${p.rows})` };
    if (lamports !== null) {
      if (c.requestLamports !== null && lamports > c.requestLamports) return { ok: false, message: `estimated ${lamports} lamports exceed the per-request cap of ${c.requestLamports}` };
      if (c.dayLamports !== null && BigInt(t.lamports) + p.lamports + lamports > c.dayLamports)
        return { ok: false, message: `estimated ${lamports} lamports would pass the daily cap of ${c.dayLamports} (used ${t.lamports}, in flight ${p.lamports})` };
      if (c.totalLamports !== null && BigInt(this.data.total_lamports) + p.lamports + lamports > c.totalLamports)
        return { ok: false, message: `estimated ${lamports} lamports would pass the total cap of ${c.totalLamports} (used ${this.data.total_lamports})` };
    }
    const id = `r${++this.seq}`;
    this.inflight.set(id, { rows, lamports });
    return { ok: true, id };
  }

  release(id) {
    this.inflight.delete(id);
  }

  /** Book the gateway's actual charge for a served request. */
  record(id, receipt) {
    this.inflight.delete(id);
    const rows = Number(receipt.rows ?? 0);
    const lamports = BigInt(receipt.charged_lamports ?? 0);
    const d = day();
    const t = this.today();
    this.data.days[d] = { lamports: (BigInt(t.lamports) + lamports).toString(), rows: t.rows + rows, requests: t.requests + 1 };
    this.data.total_lamports = (BigInt(this.data.total_lamports) + lamports).toString();
    this.data.total_rows += rows;
    this.data.requests_count += 1;
    this.data.requests.push({
      at_ms: Date.now(), request_id: receipt.request_id ?? null, ticket_id: receipt.ticket_id ?? null, model: receipt.model ?? null,
      rows, charged_lamports: receipt.charged_lamports === undefined || receipt.charged_lamports === null ? null : String(receipt.charged_lamports),
    });
    if (this.data.requests.length > KEEP_REQUESTS) this.data.requests.splice(0, this.data.requests.length - KEEP_REQUESTS);
    this.save();
  }

  save() {
    if (!this.path) return;
    mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    const tmp = `${this.path}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify(this.data, null, 2), { mode: 0o600 });
    renameSync(tmp, this.path);
  }

  summary() {
    const c = this.caps;
    const s = (v) => (v === null ? null : String(v));
    return {
      today: { day: day(), ...this.today() },
      total_lamports: this.data.total_lamports, total_rows: this.data.total_rows, requests: this.data.requests_count,
      in_flight: { rows: this.pending().rows, lamports: this.pending().lamports.toString() },
      caps: { request_lamports: s(c.requestLamports), day_lamports: s(c.dayLamports), total_lamports: s(c.totalLamports), request_rows: c.requestRows, day_rows: c.dayRows },
    };
  }
}
