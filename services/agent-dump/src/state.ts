// The writer's state file: the cache (immutable chain records read once) plus the cursor of the
// last pushed batch. Written atomically (tmp + rename) only after a batch is pushed or committed.
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { emptyCache, type Cache } from "./collect.ts";

export interface State {
  version: 1;
  cache: Cache;
  cursor: null | { cutoff_height: number; solana_slot: number | null; commit: string | null; pushed_at_ms: number; totals: Record<string, number> };
}

export function loadState(path: string): State {
  try {
    const s = JSON.parse(readFileSync(path, "utf8"));
    if (s?.version === 1 && s.cache) return { version: 1, cache: { ...emptyCache(), ...s.cache }, cursor: s.cursor ?? null };
  } catch { /* first run, or unreadable: start fresh (the repo itself is re-rendered in full every batch) */ }
  return { version: 1, cache: emptyCache(), cursor: null };
}

export function saveState(path: string, s: State): void {
  mkdirSync(dirname(path), { recursive: true });
  const tmp = `${path}.tmp-${process.pid}`;
  writeFileSync(tmp, JSON.stringify(s), { mode: 0o600 });
  renameSync(tmp, path);
}
