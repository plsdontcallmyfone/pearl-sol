// BLAKE3 (unkeyed hash, 32-byte output), dependency free: the reference algorithm
// (chunks of 1024 bytes, 64-byte blocks, binary Merkle tree of chaining values).
// Used for social ids (post_id, avatar hues) and blob ids; checked against the
// published test vectors and the chain's own vectors (test/social.test.ts).

const IV = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
const PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1, CHUNK_END = 2, PARENT = 4, ROOT = 8;

const rotr = (x: number, n: number) => ((x >>> n) | (x << (32 - n))) >>> 0;

function g(s: Uint32Array, a: number, b: number, c: number, d: number, x: number, y: number) {
  s[a] = (s[a] + s[b] + x) >>> 0;
  s[d] = rotr(s[d] ^ s[a], 16);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotr(s[b] ^ s[c], 12);
  s[a] = (s[a] + s[b] + y) >>> 0;
  s[d] = rotr(s[d] ^ s[a], 8);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotr(s[b] ^ s[c], 7);
}

function compress(cv: Uint32Array, m: Uint32Array, counter: number, len: number, flags: number): Uint32Array {
  const s = new Uint32Array([...cv, IV[0], IV[1], IV[2], IV[3], counter >>> 0, Math.floor(counter / 2 ** 32) >>> 0, len, flags]);
  let w = Uint32Array.from(m);
  for (let r = 0; r < 7; r++) {
    g(s, 0, 4, 8, 12, w[0], w[1]);
    g(s, 1, 5, 9, 13, w[2], w[3]);
    g(s, 2, 6, 10, 14, w[4], w[5]);
    g(s, 3, 7, 11, 15, w[6], w[7]);
    g(s, 0, 5, 10, 15, w[8], w[9]);
    g(s, 1, 6, 11, 12, w[10], w[11]);
    g(s, 2, 7, 8, 13, w[12], w[13]);
    g(s, 3, 4, 9, 14, w[14], w[15]);
    w = Uint32Array.from(PERM.map((i) => w[i]));
  }
  for (let i = 0; i < 8; i++) {
    s[i] ^= s[i + 8];
    s[i + 8] ^= cv[i];
  }
  return s;
}

function words(b: Uint8Array): Uint32Array {
  const p = new Uint8Array(64);
  p.set(b);
  const w = new Uint32Array(16);
  for (let i = 0; i < 16; i++) w[i] = (p[i * 4] | (p[i * 4 + 1] << 8) | (p[i * 4 + 2] << 16) | (p[i * 4 + 3] << 24)) >>> 0;
  return w;
}

interface Out {
  cv: Uint32Array;
  block: Uint32Array;
  counter: number;
  len: number;
  flags: number;
}

function chunkOut(chunk: Uint8Array, counter: number): Out {
  let cv = IV;
  const blocks = Math.max(1, Math.ceil(chunk.length / 64));
  for (let i = 0; i < blocks - 1; i++) {
    cv = compress(cv, words(chunk.subarray(i * 64, i * 64 + 64)), counter, 64, i === 0 ? CHUNK_START : 0).subarray(0, 8);
  }
  const last = chunk.subarray((blocks - 1) * 64);
  return { cv, block: words(last), counter, len: last.length, flags: (blocks === 1 ? CHUNK_START : 0) | CHUNK_END };
}

const outCv = (o: Out) => compress(o.cv, o.block, o.counter, o.len, o.flags).subarray(0, 8);
const parentOut = (l: Uint32Array, r: Uint32Array): Out => ({ cv: IV, block: Uint32Array.from([...l, ...r]), counter: 0, len: 64, flags: PARENT });

export function blake3(...parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((n, p) => n + p.length, 0);
  const data = new Uint8Array(total);
  let off = 0;
  for (const p of parts) {
    data.set(p, off);
    off += p.length;
  }
  const n = Math.max(1, Math.ceil(data.length / 1024));
  const stack: Uint32Array[] = [];
  let out: Out | null = null;
  for (let i = 0; i < n; i++) {
    const o = chunkOut(data.subarray(i * 1024, i * 1024 + 1024), i);
    if (i === n - 1) {
      out = o;
      break;
    }
    let cv = outCv(o);
    let t = i + 1;
    while ((t & 1) === 0) {
      cv = outCv(parentOut(stack.pop()!, cv));
      t >>= 1;
    }
    stack.push(cv);
  }
  while (stack.length) out = parentOut(stack.pop()!, outCv(out!));
  const s = compress(out!.cv, out!.block, out!.counter, out!.len, out!.flags | ROOT);
  const res = new Uint8Array(32);
  for (let i = 0; i < 8; i++) for (let j = 0; j < 4; j++) res[i * 4 + j] = (s[i] >>> (8 * j)) & 0xff;
  return res;
}
