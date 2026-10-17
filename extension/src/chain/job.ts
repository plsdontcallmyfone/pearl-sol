// Pearl job commitments for PostJob, ported from crates/pearl-job (job.rs, merkle.rs,
// pattern.rs) and the v2 header rule in docs/RPC.md. Parity with the `pearl-job`
// CLI is checked in test/job-parity.test.ts.
//
//   header76 = job_id || terms_hash || 0 u64 || 0 u32
//   job_key  = blake3(header76 || config52)
//   hash_a   = blake3_keyed(pad1024(A row-major, m x k), job_key)
//   hash_b   = blake3_keyed(pad1024(B^T row-major), job_key)   (B is k x n)

import { concat, fromHex, hex, u32, u64 } from "../core/bytes";
import { blake3, blake3Keyed } from "../core/crypto";
import { jobIdForPost, termsHash } from "./messages";

export const CHUNK_LEN = 1024;

/** Localnet default config (k=1024, r=32, 4x8 tile): `pearl-job config` output. */
export const LOCALNET_CONFIG_HEX = "00040000200000000003000000000007000000000000000000000000000000000000000000000000000000000000000000000000";

export function jobHeader(jobId: Uint8Array, terms: Uint8Array): Uint8Array {
  if (jobId.length !== 32 || terms.length !== 32) throw new Error("job id and terms hash are 32 bytes");
  return concat(jobId, terms, u64(0n), u32(0));
}

export function jobKey(header: Uint8Array, config: Uint8Array): Uint8Array {
  if (header.length !== 76) throw new Error("job header is 76 bytes");
  if (config.length !== 52) throw new Error("config is 52 bytes");
  return blake3(header, config);
}

export function padToChunk(data: Uint8Array): Uint8Array {
  const len = Math.ceil(data.length / CHUNK_LEN) * CHUNK_LEN;
  if (len === data.length) return data;
  const out = new Uint8Array(len);
  out.set(data);
  return out;
}

export function transpose(x: Uint8Array, rows: number, cols: number): Uint8Array {
  if (x.length !== rows * cols) throw new Error(`matrix is ${x.length} bytes, expected ${rows} x ${cols}`);
  const out = new Uint8Array(x.length);
  for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) out[c * rows + r] = x[r * cols + c];
  return out;
}

export const commitA = (key: Uint8Array, a: Uint8Array) => blake3Keyed(key, padToChunk(a));
export const commitB = (key: Uint8Array, b: Uint8Array, k: number, n: number) => blake3Keyed(key, padToChunk(transpose(b, k, n)));

// ---------------------------------------------------------------- config

export interface Pattern {
  shape: [number, number][]; // (stride, length) x 3
  period: number;
  size: number;
}

/** PeriodicPattern::from_bytes (6 bytes, canonical form only). */
export function decodePattern(d: Uint8Array): Pattern {
  if (d.length !== 6) throw new Error("pattern is 6 bytes");
  const shape: [number, number][] = [];
  let minStride = 1;
  let done = false;
  for (let i = 0; i < 3; i++) {
    const factor = 1 + d[2 * i];
    const length = 1 + d[2 * i + 1];
    if (length === 1 || done) {
      if (factor !== 1 || length !== 1) throw new Error("pattern: non-canonical representation");
      done = true;
    } else if (factor <= 1 && minStride !== 1) {
      throw new Error("pattern: a single stride must not be broken");
    }
    if (minStride > Math.floor((1 << 24) / (factor * length))) throw new Error("pattern: period must be <= 2^24");
    const stride = factor * minStride;
    shape.push([stride, length]);
    minStride = stride * length;
  }
  const [s, l] = shape[2];
  return { shape, period: s * l, size: shape.reduce((a, [, len]) => a * len, 1) };
}

export interface MiningConfig {
  k: number;
  rank: number;
  rows: Pattern;
  cols: Pattern;
}

export function decodeConfig(c: Uint8Array): MiningConfig {
  if (c.length !== 52) throw new Error("config is 52 bytes");
  const dv = new DataView(c.buffer, c.byteOffset, c.byteLength);
  if (dv.getUint16(6, true) !== 0) throw new Error("unsupported mma_type");
  if (c.subarray(20).some((x) => x !== 0)) throw new Error("MoE / reserved trailer must be zero");
  return { k: dv.getUint32(0, true), rank: dv.getUint16(4, true), rows: decodePattern(c.subarray(8, 14)), cols: decodePattern(c.subarray(14, 20)) };
}

/** check_shape from pearl-job (the parts a poster can get wrong). Returns a reason or null. */
export function shapeProblem(c: MiningConfig, m: number, n: number): string | null {
  const { k, rank: r } = c;
  const h = c.rows.size, w = c.cols.size;
  const dpl = k - (k % r);
  if (!(r >= 32 && r <= 1024 && (r & (r - 1)) === 0)) return `rank must be a power of two in 32..1024, got ${r}`;
  if (k % 64 !== 0 || k < 1024 || k > 1 << 16) return `k must be a multiple of 64 in 1024..65536, got ${k}`;
  if (k > 4 * r * r || k < 16 * r) return "k must be within 16r..4r^2";
  if (h % 2 || w % 2 || h * w < 32 || h * w > 256) return "tile h x w must be even and within 32..256";
  if (dpl % 64 !== 0) return "dot product length must be a multiple of 64";
  if ((h + w) * dpl > 1 << 22) return "worker input over 4 MiB";
  if (!(m > 0 && m % c.rows.period === 0)) return `m must be a positive multiple of ${c.rows.period}`;
  if (!(n > 0 && n % c.cols.period === 0)) return `n must be a positive multiple of ${c.cols.period}`;
  if (m > 1 << 24 || n > 1 << 24) return "m, n must be <= 2^24";
  return null;
}

// ---------------------------------------------------------------- one call

export interface JobCommitInput {
  poster: Uint8Array;
  nonce: bigint; // the PostJob tx nonce (job_id = blake3(poster || nonce))
  m: number;
  n: number;
  config: Uint8Array;
  workerPay: bigint;
  protocolFee: bigint;
  deadlineHeight: bigint;
  maxBlocks: number;
  a: Uint8Array; // m x k int8 row-major
  b: Uint8Array; // k x n int8 row-major
}

export function commitJob(i: JobCommitInput) {
  const cfg = decodeConfig(i.config);
  const problem = shapeProblem(cfg, i.m, i.n);
  if (problem) throw new Error(problem);
  if (i.a.length !== i.m * cfg.k) throw new Error(`A must be m x k = ${i.m * cfg.k} bytes, got ${i.a.length}`);
  if (i.b.length !== cfg.k * i.n) throw new Error(`B must be k x n = ${cfg.k * i.n} bytes, got ${i.b.length}`);
  const jobId = jobIdForPost(i.poster, i.nonce);
  const terms = termsHash({ poster: i.poster, jobId, m: i.m, n: i.n, k: cfg.k, workerPay: i.workerPay, protocolFee: i.protocolFee, deadlineHeight: i.deadlineHeight, maxBlocks: i.maxBlocks });
  const header = jobHeader(jobId, terms);
  const key = jobKey(header, i.config);
  return { jobId, termsHash: terms, header, jobKey: key, k: cfg.k, hashA: commitA(key, i.a), hashB: commitB(key, i.b, cfg.k, i.n) };
}

export const localnetConfig = () => fromHex(LOCALNET_CONFIG_HEX);
export { hex };
