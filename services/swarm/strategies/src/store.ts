// Content-addressed storage for the off-chain halves of every commitment (strategy specs, entry
// snapshots, trade records, post-mortems). The id is blake3 of the exact bytes, the same id a
// Cadre blob gets, so the hash committed on chain names the blob. Three homes:
//   1. this runner's own directory (always; the runner never commits a hash it has not kept),
//   2. cadre-api's durable store, POST /v1/strategies/blobs (kept for good, served at
//      GET /v1/strategies/blobs/{id}); best effort here, `practice sync-blobs` re-sends,
//   3. a Cadre node blob (putBlob), so anyone can fetch it by id while the node keeps it (nodes
//      prune blobs no chain record references after grace_blocks; docs/RPC.md "Retention").
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { blobId, canonicalJson } from "./spec.ts";

export const BLOB_FORMATS = ["cadre-strategy-v1", "cadre-practice-snapshot-v1", "cadre-practice-trade-v1", "cadre-postmortem-v1"];
export const MAX_BLOB_BYTES = 256 * 1024;

export class BlobStore {
  dir: string;
  api: string | null;
  nodeRpc: ((method: string, params: unknown) => Promise<any>) | null;
  log: (e: string, x: Record<string, unknown>) => void;
  constructor(o: { dir: string; api?: string | null; nodeRpc?: ((method: string, params: unknown) => Promise<any>) | null; log?: (e: string, x: Record<string, unknown>) => void }) {
    this.dir = o.dir;
    this.api = o.api ? o.api.replace(/\/$/, "") : null;
    this.nodeRpc = o.nodeRpc ?? null;
    this.log = o.log ?? (() => {});
    mkdirSync(this.dir, { recursive: true });
  }
  path(id: string) { return join(this.dir, `${id}.json`); }

  /** Keep `obj` as canonical JSON; returns its id. The local copy is written before anything else. */
  async put(obj: { format: string }): Promise<{ id: string; bytes: number; api: string; node: string }> {
    if (!BLOB_FORMATS.includes(obj.format)) throw new Error(`unknown blob format ${obj.format}`);
    const bytes = Buffer.from(canonicalJson(obj), "utf8");
    if (bytes.length > MAX_BLOB_BYTES) throw new Error(`blob is ${bytes.length} bytes, above ${MAX_BLOB_BYTES}`);
    const id = blobId(bytes);
    if (!existsSync(this.path(id))) {
      const tmp = `${this.path(id)}.tmp-${process.pid}`;
      writeFileSync(tmp, bytes);
      renameSync(tmp, this.path(id));
    }
    const [api, node] = await Promise.all([this.toApi(id, bytes), this.toNode(bytes, id)]);
    return { id, bytes: bytes.length, api, node };
  }

  get(id: string): any | null {
    if (!existsSync(this.path(id))) return null;
    const b = readFileSync(this.path(id));
    return blobId(b) === id ? JSON.parse(b.toString("utf8")) : null;
  }

  async toApi(id: string, bytes: Buffer): Promise<string> {
    if (!this.api) return "no api configured";
    try {
      const r = await fetch(`${this.api}/v1/strategies/blobs`, { method: "POST", headers: { "content-type": "application/json" }, body: bytes, signal: AbortSignal.timeout(15_000) });
      const j: any = await r.json().catch(() => ({}));
      if (r.status === 201 && j.id === id) return "stored";
      return `not stored: HTTP ${r.status}${j?.error?.code ? ` ${j.error.code}` : ""}`;
    } catch (e) {
      return `not stored: ${(e as Error).message.slice(0, 80)}`;
    }
  }

  async toNode(bytes: Buffer, id: string): Promise<string> {
    if (!this.nodeRpc) return "no node configured";
    try {
      const r = await this.nodeRpc("putBlob", { data: bytes.toString("base64") });
      return r?.blob_id === id ? "stored" : "unexpected reply";
    } catch (e) {
      return `not stored: ${(e as Error).message.slice(0, 80)}`;
    }
  }

  /** Re-send every local blob to cadre-api (after its strategies routes are deployed). */
  async syncToApi(): Promise<{ sent: number; failed: number }> {
    let sent = 0, failed = 0;
    for (const f of readdirSync(this.dir).filter((x) => /^[0-9a-f]{64}\.json$/.test(x))) {
      const id = f.slice(0, 64);
      const r = await this.toApi(id, readFileSync(join(this.dir, f)));
      if (r === "stored") sent++;
      else { failed++; this.log("blob_sync_failed", { id, why: r }); }
    }
    return { sent, failed };
  }
}
