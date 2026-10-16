// Thin wrapper around the `pearl-job` CLI (crates/pearl-job, src/bin/pearl-job.rs).
//
// Contract: every subcommand prints exactly one JSON object on stdout. On failure it
// prints {"ok": false, "error": "..."} and exits non-zero. Subcommands used here:
//
//   gen      --m --n --k --out-dir [--seed]
//            -> {ok, m, n, k, manifest, a, b, data_uri}
//   config   --k [--rank --rows --cols]
//            -> {ok, config (52-byte hex), k, rank, h, w, ...}
//   commit   --job <base58> --terms-hash <hex> --slot <u64> --target-bits <u32>
//            --config <hex> --data <uri>
//            -> {ok, hash_a, hash_b, job_key, header (76-byte hex), config, m, n, k}
//   freivalds --job-file <JobSpec json> --data <uri> --c <path> [--rounds]
//            -> {ok, valid, rounds, elapsed_ms, result_hash, hash_a, hash_b}
//
// Binary lookup: $PEARL_JOB, then crates/target/{release,debug}/pearl-job, then PATH.

import { execFileSync } from "child_process";
import * as fs from "fs";
import * as path from "path";

export function pearlJobBin(repoRoot: string): string {
  if (process.env.PEARL_JOB) return process.env.PEARL_JOB;
  for (const p of ["release", "debug"]) {
    const b = path.join(repoRoot, "crates", "target", p, "pearl-job");
    if (fs.existsSync(b)) return b;
  }
  return "pearl-job";
}

export function pearlJob(repoRoot: string, args: string[]): any {
  const bin = pearlJobBin(repoRoot);
  let out: string;
  try {
    out = execFileSync(bin, args, { encoding: "utf8", maxBuffer: 64 << 20 });
  } catch (e: any) {
    const so = e.stdout?.toString?.() ?? "";
    let msg = so.trim() || String(e.message);
    try {
      msg = JSON.parse(so).error ?? msg;
    } catch {}
    throw new Error(`pearl-job ${args[0]} failed: ${msg}`);
  }
  const v = JSON.parse(out.trim().split("\n").pop()!);
  if (!v.ok) throw new Error(`pearl-job ${args[0]}: ${v.error}`);
  return v;
}
