// PostJob commitments: TS port vs the `pearl-job` CLI (crates/target/release/pearl-job).
// The CLI output is committed in test/fixtures/job/commit.json; when the binary is
// present the test also runs it live. Regenerate with UPDATE_JOB_FIXTURE=1.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { fromHex, hex, key32 } from "../src/core/bytes";
import { LOCALNET_CONFIG_HEX, commitJob, decodeConfig, shapeProblem, transpose } from "../src/chain/job";

const FIX = resolve(__dirname, "fixtures/job");
const CLI = [resolve(__dirname, "../../crates/target/release/pearl-job"), resolve(__dirname, "../../crates/target/debug/pearl-job")].find(existsSync);

const input = {
  poster: key32("6DDSqnFUxWMu66T4pgQJpaUxEmfYdrJLJdcJmUtL6HW7"),
  nonce: 3n,
  m: 8,
  n: 16,
  config: fromHex(LOCALNET_CONFIG_HEX),
  workerPay: 5_000_000_000n,
  protocolFee: 1_000_000_000n,
  deadlineHeight: 1000n,
  maxBlocks: 10,
  a: new Uint8Array(readFileSync(resolve(FIX, "A.i8"))),
  b: new Uint8Array(readFileSync(resolve(FIX, "B.i8"))),
};

function runCli(jobIdHex: string, termsHex: string) {
  const out = execFileSync(CLI!, [
    "commit", "--job", jobIdHex, "--terms-hash", termsHex, "--slot", "0", "--target-bits", "0",
    "--config", LOCALNET_CONFIG_HEX, "--data", resolve(FIX, "manifest.json"),
  ]).toString();
  return JSON.parse(out);
}

describe("PostJob commitments vs pearl-job", () => {
  const ts = commitJob(input);

  it("matches the committed CLI fixture byte for byte", () => {
    const fixPath = resolve(FIX, "commit.json");
    if (process.env.UPDATE_JOB_FIXTURE && CLI) {
      const cli = runCli(hex(ts.jobId), hex(ts.termsHash));
      writeFileSync(fixPath, JSON.stringify({ cli: CLI.split("/crates/")[1], job_id: hex(ts.jobId), terms_hash: hex(ts.termsHash), ...cli }, null, 2) + "\n");
    }
    const fx = JSON.parse(readFileSync(fixPath, "utf8"));
    expect(fx.job_id).toBe(hex(ts.jobId));
    expect(fx.terms_hash).toBe(hex(ts.termsHash));
    expect(fx.header).toBe(hex(ts.header));
    expect(fx.config).toBe(LOCALNET_CONFIG_HEX);
    expect(fx.job_key).toBe(hex(ts.jobKey));
    expect(fx.hash_a).toBe(hex(ts.hashA));
    expect(fx.hash_b).toBe(hex(ts.hashB));
  });

  it.runIf(!!CLI)("matches a live pearl-job run", () => {
    const cli = runCli(hex(ts.jobId), hex(ts.termsHash));
    expect(cli.ok).toBe(true);
    expect(cli.header).toBe(hex(ts.header));
    expect(cli.job_key).toBe(hex(ts.jobKey));
    expect(cli.hash_a).toBe(hex(ts.hashA));
    expect(cli.hash_b).toBe(hex(ts.hashB));
  });

  it.runIf(!!CLI)("matches pearl-job for a second shape and different terms", () => {
    // Same data, other job id / terms: every hash must move and still agree.
    const other = commitJob({ ...input, nonce: 4n, workerPay: 7n, maxBlocks: 99 });
    const cli = runCli(hex(other.jobId), hex(other.termsHash));
    expect(cli.hash_a).toBe(hex(other.hashA));
    expect(cli.hash_b).toBe(hex(other.hashB));
    expect(hex(other.hashA)).not.toBe(hex(ts.hashA));
  });

  it("config decode and shape rules", () => {
    const c = decodeConfig(fromHex(LOCALNET_CONFIG_HEX));
    expect([c.k, c.rank, c.rows.size, c.cols.size, c.rows.period, c.cols.period]).toEqual([1024, 32, 4, 8, 4, 8]);
    expect(shapeProblem(c, 8, 16)).toBeNull();
    expect(shapeProblem(c, 6, 16)).toMatch(/multiple of 4/);
    expect(shapeProblem(c, 8, 12)).toMatch(/multiple of 8/);
    expect(() => commitJob({ ...input, a: input.a.subarray(1) })).toThrow(/A must be/);
  });

  it("transpose", () => {
    expect(Array.from(transpose(Uint8Array.of(1, 2, 3, 4, 5, 6), 2, 3))).toEqual([1, 4, 2, 5, 3, 6]);
  });
});
