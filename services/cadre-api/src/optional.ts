// Optional modules other agents are adding to the tree (apps/explorer supply / agent
// profile modules). Imported on first use; a missing or broken module is retried at most
// once a minute and the caller falls back (never to invented data).

const cache = new Map<string, { at: number; fn: unknown | null }>();

export async function optionalExport<T>(relFromSrc: string, names: string[]): Promise<T | null> {
  const key = relFromSrc + "#" + names.join(",");
  const hit = cache.get(key);
  if (hit && (hit.fn || Date.now() - hit.at < 60_000)) return hit.fn as T | null;
  let fn: unknown | null = null;
  try {
    const mod = await import(new URL(relFromSrc, import.meta.url).href);
    for (const n of names) if (typeof mod[n] === "function") {
      fn = mod[n];
      break;
    }
  } catch {
    fn = null;
  }
  cache.set(key, { at: Date.now(), fn });
  return fn as T | null;
}
