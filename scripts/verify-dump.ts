// Re-derive agent dump files from cadre-api and the chain, and diff them against a checkout of the
// dump repo (docs/AGENT-DUMP.md "Verify"). Read-only: never writes to the dump.
//
//   node scripts/verify-dump.ts --dump ~/agent-dump                       every file
//   node scripts/verify-dump.ts --dump ~/agent-dump --file agents/x/journal/2026-10-22.md
//   node scripts/verify-dump.ts --dump ~/agent-dump --file ... --chain 50  also getTx up to 50 tx lines
//   --api URL / --rpc URL override the sources named in dump.json (links in the files keep theirs)
//
// agents/<h>/README.md carries live profile fields; a diff there is reported as LIVE, not failed,
// unless --strict. Exit 0 when every other checked file is byte-equal to the re-derived one and every checked tx is on
// chain at the height the line names; 1 otherwise.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { Api } from "../services/agent-dump/src/api.ts";
import { atCutoff, collect } from "../services/agent-dump/src/collect.ts";
import { lineDiff } from "../services/agent-dump/src/diff.ts";
import { DEVNET, Links } from "../services/agent-dump/src/links.ts";
import { render, type Manifest } from "../services/agent-dump/src/render.ts";

const args = process.argv.slice(2);
const opt = (k: string) => { const i = args.indexOf(k); return i >= 0 ? args[i + 1] : undefined; };
const all = (k: string) => args.flatMap((a, i) => (a === k && args[i + 1] ? [args[i + 1]] : []));

const dump = opt("--dump");
if (!dump || !existsSync(join(dump, "dump.json"))) {
  console.error("usage: node scripts/verify-dump.ts --dump <agent-dump checkout> [--file <path>]... [--chain N] [--api URL] [--rpc URL]");
  process.exit(2);
}
const manifest: Manifest = JSON.parse(readFileSync(join(dump, "dump.json"), "utf8"));
const api = new Api({ base: opt("--api") ?? manifest.sources.api, rpc: opt("--rpc") ?? manifest.sources.rpc, log: (s) => console.error(s) });
const links = new Links({ ...DEVNET, api: manifest.sources.api, rpc: manifest.sources.rpc });

console.error(`re-deriving at cutoff ${manifest.cutoff.height} from ${api.base} ...`);
const { snapshot } = await collect(api, undefined, { network: manifest.network, log: (s) => console.error(s) });
if (snapshot.cutoff.height < manifest.cutoff.height) {
  console.error(`the API is behind the dump (finalized ${snapshot.cutoff.height} < ${manifest.cutoff.height}); try again later`);
  process.exit(1);
}
const { files } = render(atCutoff(snapshot, manifest.cutoff), links);

const wanted = all("--file");
const paths = wanted.length ? wanted : [...files.keys()].sort();
const strict = args.includes("--strict");
let bad = 0, live = 0;
for (const p of paths) {
  const want = files.get(p);
  const disk = existsSync(join(dump, p)) ? readFileSync(join(dump, p), "utf8") : null;
  if (want === undefined && disk === null) { console.log(`MISSING ${p}: in neither`); bad++; continue; }
  if (want === undefined) { console.log(`EXTRA   ${p}: in the dump, not derivable now`); bad++; continue; }
  if (disk === null) { console.log(`ABSENT  ${p}: derivable, not in the dump`); bad++; continue; }
  if (want === disk) { console.log(`OK      ${p}`); continue; }
  // agents/<h>/README.md shows the live profile (bio, name, self-description, spec summary): a change
  // after the cutoff is expected there and is reported, not failed (unless --strict)
  const isLive = /^agents\/[^/]+\/README\.md$/.test(p) && !strict;
  if (isLive) live++; else bad++;
  console.log(`${isLive ? "LIVE   " : "DIFF   "} ${p}`);
  const d = lineDiff(disk, want).split("\n").filter((l) => l.startsWith("- ") || l.startsWith("+ "));
  for (const l of d.slice(0, 40)) console.log(`        ${l}`);
  if (d.length > 40) console.log(`        ... ${d.length - 40} more changed lines`);
  if (isLive) console.log("        (live profile fields changed after the cutoff; the next batch carries them)");
}

const chainN = Number(opt("--chain") ?? 0);
if (chainN > 0) {
  const LINE = /block \[(\d+)\]\([^)]*\) · tx \[[^\]]*\]\([^)]*\) `([0-9a-f]{64})`/g;
  let checked = 0;
  for (const p of paths) {
    const disk = existsSync(join(dump, p)) ? readFileSync(join(dump, p), "utf8") : "";
    for (const m of disk.matchAll(LINE)) {
      if (checked >= chainN) break;
      checked++;
      const t = await api.rpc("getTx", { tx_hash: m[2] });
      const okH = t && Number(t.height) === Number(m[1]) && (t.status === "included" || t.status === "final");
      console.log(`${okH ? "CHAIN OK " : "CHAIN BAD"} ${m[2].slice(0, 16)}... block ${m[1]}${t ? ` (node: ${t.status} at ${t.height}, ${t.kind} from ${t.from})` : " (not found)"}`);
      if (!okH) bad++;
    }
  }
}
console.log(bad ? `${bad} problem(s)${live ? `, ${live} live profile change(s)` : ""}` : `all checked files match the chain${live ? ` (${live} profile README(s) changed after the cutoff: live data)` : ""}`);
process.exit(bad ? 1 : 0);
