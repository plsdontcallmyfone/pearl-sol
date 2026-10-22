// Line diff (LCS) in unified style without hunks: small inputs only (strategy specs, a few hundred lines).

export function canonical(v: unknown): string {
  const sort = (x: any): any => (Array.isArray(x) ? x.map(sort) : x && typeof x === "object" ? Object.fromEntries(Object.keys(x).sort().map((k) => [k, sort(x[k])])) : x);
  return JSON.stringify(sort(v), null, 2);
}

export function lineDiff(a: string, b: string): string {
  const A = a.split("\n"), B = b.split("\n");
  const n = A.length, m = B.length;
  if (n * m > 4_000_000) return `- (${n} lines)\n+ (${m} lines)`;
  const L: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) L[i][j] = A[i] === B[j] ? L[i + 1][j + 1] + 1 : Math.max(L[i + 1][j], L[i][j + 1]);
  const out: string[] = [];
  let i = 0, j = 0;
  while (i < n && j < m) {
    if (A[i] === B[j]) { out.push(`  ${A[i]}`); i++; j++; }
    else if (L[i + 1][j] >= L[i][j + 1]) out.push(`- ${A[i++]}`);
    else out.push(`+ ${B[j++]}`);
  }
  while (i < n) out.push(`- ${A[i++]}`);
  while (j < m) out.push(`+ ${B[j++]}`);
  return out.join("\n");
}

export const changedLines = (d: string) => d.split("\n").filter((l) => l.startsWith("- ") || l.startsWith("+ ")).length;
