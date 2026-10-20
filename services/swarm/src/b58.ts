// Base58 (Bitcoin / Solana alphabet), no dependencies.
const A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const MAP: Record<string, number> = Object.fromEntries([...A].map((c, i) => [c, i]));

export function b58encode(bytes: Uint8Array): string {
  let zeros = 0;
  while (zeros < bytes.length && bytes[zeros] === 0) zeros++;
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let s = "";
  while (n > 0n) {
    s = A[Number(n % 58n)] + s;
    n /= 58n;
  }
  return "1".repeat(zeros) + s;
}

export function b58decode(str: string): Uint8Array {
  let zeros = 0;
  while (zeros < str.length && str[zeros] === "1") zeros++;
  let n = 0n;
  for (const c of str) {
    const v = MAP[c];
    if (v === undefined) throw new Error(`not base58: ${JSON.stringify(c)}`);
    n = n * 58n + BigInt(v);
  }
  const out: number[] = [];
  while (n > 0n) {
    out.unshift(Number(n & 0xffn));
    n >>= 8n;
  }
  return Uint8Array.from([...new Array(zeros).fill(0), ...out]);
}

/** 32 bytes from base58 or 64 hex. */
export function key32(s: string): Uint8Array {
  const t = s.trim();
  if (/^[0-9a-fA-F]{64}$/.test(t)) return Uint8Array.from(Buffer.from(t, "hex"));
  const v = b58decode(t);
  if (v.length !== 32) throw new Error(`${JSON.stringify(s)} is not 32 bytes`);
  return v;
}
