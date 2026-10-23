// cadre agent-dump writer (docs/AGENT-DUMP.md).
//   node src/main.ts dry-run --out DIR   collect + render into a local folder, no git
//   node src/main.ts once                one batch: collect, render, commit, push
//   node src/main.ts run                 batches every DUMP_INTERVAL_S (default 600)
//   node src/main.ts create-repo         create DUMP_REPO (public) if it does not exist (token mode)
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { Api } from "./api.ts";
import { authFromEnv, gh } from "./auth.ts";
import { collect, emptyCache } from "./collect.ts";
import { commit, push, sync, writeFiles } from "./git.ts";
import { DEVNET, Links, type LinkConfig } from "./links.ts";
import { render, type Manifest } from "./render.ts";
import { loadState, saveState } from "./state.ts";
import { commitMessage } from "./summary.ts";

const env = process.env;
const log = (s: string) => console.log(`${new Date().toISOString()} ${s}`);

export function linkConfig(e: NodeJS.ProcessEnv = env): LinkConfig {
  return {
    api: e.DUMP_API ?? DEVNET.api,
    rpc: e.DUMP_RPC ?? DEVNET.rpc,
    explorer: e.DUMP_EXPLORER ?? DEVNET.explorer,
    site: e.DUMP_SITE ?? DEVNET.site,
    solanaCluster: e.DUMP_SOLANA_CLUSTER ?? DEVNET.solanaCluster,
    repo: e.DUMP_REPO ?? DEVNET.repo,
  };
}

export function apiFromEnv(lc: LinkConfig, e: NodeJS.ProcessEnv = env): Api {
  // DUMP_API_READ lets box 0 read the local cadre-api while links keep the public URL
  return new Api({ base: e.DUMP_API_READ ?? lc.api, rpc: e.DUMP_RPC_READ ?? lc.rpc, concurrency: Number(e.DUMP_CONCURRENCY ?? 3), log });
}

async function batch(opts: { dryOut?: string }): Promise<void> {
  const lc = linkConfig();
  const api = apiFromEnv(lc);
  const stateDir = env.DUMP_STATE_DIR ?? "/var/lib/lode/agent-dump";
  const statePath = join(stateDir, "state.json");
  const state = opts.dryOut && env.DUMP_DRY_STATE !== "1" ? { version: 1 as const, cache: emptyCache(), cursor: null } : loadState(statePath);
  const t0 = Date.now();
  const { snapshot, cache } = await collect(api, state.cache, { log, network: env.DUMP_NETWORK ?? "devnet", solanaRpc: env.DUMP_SOLANA_RPC });
  const { files, manifest } = render(snapshot, new Links(lc));
  log(`collected at cutoff ${snapshot.cutoff.height} in ${Date.now() - t0} ms, ${api.calls} API calls, ${files.size} files`);
  if (opts.dryOut) {
    writeFiles(opts.dryOut, files);
    if (env.DUMP_DRY_STATE === "1") { state.cache = cache; saveState(statePath, state); }
    log(`dry run written to ${opts.dryOut}`);
    console.log(JSON.stringify(manifest.totals));
    return;
  }
  const auth = authFromEnv(env);
  const dir = join(stateDir, "repo");
  const remote = `https://github.com/${lc.repo}.git`;
  await sync(dir, remote, auth);
  const prevPath = join(dir, "dump.json");
  const prev: Manifest | null = existsSync(prevPath) ? JSON.parse(readFileSync(prevPath, "utf8")) : null;
  if (prev && prev.cutoff.height > manifest.cutoff.height) {
    log(`remote is ahead (cutoff ${prev.cutoff.height} > ${manifest.cutoff.height}); skipping this batch`);
    return;
  }
  writeFiles(dir, files);
  const msgDraft = commitMessage(prev, manifest, 0);
  const res = await commit(dir, msgDraft, await auth.identity());
  if (res.committed) {
    // amend the file count into the message now that git knows it (still one commit)
    const { git } = await import("./git.ts");
    const who = await auth.identity();
    await git(dir, ["-c", `user.name=${who.name}`, "-c", `user.email=${who.email}`, "commit", "-q", "--amend", "-m", commitMessage(prev, manifest, res.changed)]);
    await push(dir, auth);
    const sha = await git(dir, ["rev-parse", "HEAD"]);
    log(`pushed ${sha.slice(0, 10)}: ${res.changed} files`);
    state.cursor = { cutoff_height: manifest.cutoff.height, solana_slot: manifest.cutoff.solana_slot, commit: sha, pushed_at_ms: Date.now(), totals: manifest.totals };
  } else log("no change");
  state.cache = cache;
  saveState(statePath, state);
}

async function createRepo(): Promise<void> {
  const lc = linkConfig();
  const [org, name] = lc.repo.split("/");
  const auth = authFromEnv(env);
  const tok = await auth.token();
  try {
    const r = await gh(tok, `/repos/${org}/${name}`);
    log(`${r.full_name} exists (private: ${r.private})`);
    return;
  } catch { /* create */ }
  const r = await gh(tok, `/orgs/${org}/repos`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name, private: false, description: "What every Cadre agent said, decided, delivered and learned, each entry tied to its on-chain receipt. Generated from public chain data.", has_issues: false, has_projects: false, has_wiki: false, auto_init: false }),
  });
  log(`created ${r.full_name} (private: ${r.private})`);
}

async function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  if (cmd === "dry-run") {
    const i = rest.indexOf("--out");
    const out = i >= 0 ? rest[i + 1] : "./out";
    return batch({ dryOut: out });
  }
  if (cmd === "once") return batch({});
  if (cmd === "create-repo") return createRepo();
  if (cmd === "run") {
    const every = Math.max(60, Number(env.DUMP_INTERVAL_S ?? 600)) * 1000;
    let stop = false;
    let wake: (() => void) | null = null;
    for (const s of ["SIGTERM", "SIGINT"] as const) process.on(s, () => { stop = true; wake?.(); });
    let fails = 0;
    while (!stop) {
      try {
        await batch({});
        fails = 0;
      } catch (e) {
        fails++;
        log(`batch failed (${fails}): ${(e as Error).message}`);
      }
      const wait = fails ? Math.min(every, 60_000 * 2 ** Math.min(fails, 4)) : every;
      await new Promise<void>((r) => { wake = r; setTimeout(r, wait); });
      wake = null;
    }
    log("stopped");
    return;
  }
  console.error("usage: main.ts dry-run --out DIR | once | run | create-repo");
  process.exit(2);
}

if (import.meta.url === `file://${process.argv[1]}`) main().catch((e) => { console.error((e as Error).message); process.exit(1); });
