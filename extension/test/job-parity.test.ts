// PostJob commitments: TS port vs the `pearl-job` CLI (crates/target/release/pearl-job).
// The CLI output is committed in test/fixtures/job/commit.json; when the binary is
// present the test also runs it live. Regenerate with UPDATE_JOB_FIXTURE=1.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { fromHex, hex, key32 } from "../src/core/bytes";
import { LOCALNET_CONFIG_HEX, commitJob, commitOrder, decodeConfig, jobHeaderProblem, shapeProblem, transpose } from "../src/chain/job";
import { orderTermsHash } from "../src/chain/messages";

const FIX = resolve(__dirname, "fixtures/job");
const CLI = [process.env.PEARL_JOB_BIN, resolve(__dirname, "../../crates/target/release/pearl-job"), resolve(__dirname, "../../crates/target/debug/pearl-job")].filter((p): p is string => !!p).find(existsSync);
// CI builds the CLI and sets PEARL_JOB_BIN, so a missing binary there is a broken pipeline, not a skip.
if (!CLI && process.env.CI) throw new Error("CI: pearl-job CLI not found (set PEARL_JOB_BIN); the live parity tests would be skipped");

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
    expect([c.k, c.rank, c.rows.size, c.cols.size, c.rows.period, c.cols.period]).toEqual([2048, 128, 4, 8, 4, 8]);
    expect(shapeProblem(c, 8, 16)).toBeNull();
    expect(shapeProblem(c, 6, 16)).toMatch(/multiple of 4/);
    expect(shapeProblem(c, 8, 12)).toMatch(/multiple of 8/);
    expect(() => commitJob({ ...input, a: input.a.subarray(1) })).toThrow(/A must be/);
  });

  it("transpose", () => {
    expect(Array.from(transpose(Uint8Array.of(1, 2, 3, 4, 5, 6), 2, 3))).toEqual([1, 4, 2, 5, 3, 6]);
  });
});

// Storefront orders: header = poster || order_terms (chain/state schema order_terms_hash).
// order_terms parity against the Rust vector chain/state/vectors/order_terms.json
// (written by chain/state/tests/order_terms_vector.rs); full commitments against pearl-job.
describe("storefront order commitments", () => {
  const VEC = resolve(__dirname, "../../chain/state/vectors/order_terms.json");
  const vec = JSON.parse(readFileSync(VEC, "utf8"));

  it("orderTermsHash matches the Rust vector byte for byte", () => {
    expect(vec.cases.length).toBeGreaterThanOrEqual(3);
    for (const c of vec.cases) {
      const got = orderTermsHash({ poster: fromHex(c.poster_hex), m: c.m, n: c.n, k: c.k, maxBlocks: c.max_blocks, deadlineBlocks: BigInt(c.deadline_blocks) });
      expect(hex(got)).toBe(c.order_terms);
    }
  });

  const orderInput = { poster: input.poster, m: 8, n: 16, config: input.config, maxBlocks: 10, deadlineBlocks: 500n, a: input.a, b: input.b };
  const ord = commitOrder(orderInput);

  it("header is poster || order_terms || 0 u64 || 0 u32", () => {
    expect(hex(ord.header.subarray(0, 32))).toBe(hex(input.poster));
    expect(hex(ord.header.subarray(32, 64))).toBe(hex(ord.termsHash));
    expect(ord.header.subarray(64).every((x) => x === 0)).toBe(true);
    // the terms depend only on what the buyer knows at buy time
    expect(hex(commitOrder({ ...orderInput, deadlineBlocks: 501n }).hashA)).not.toBe(hex(ord.hashA));
    expect(hex(commitOrder({ ...orderInput, poster: key32("11111111111111111111111111111112") }).hashA)).not.toBe(hex(ord.hashA));
  });

  it("matches the committed pearl-job fixture for an order header", () => {
    const fixPath = resolve(FIX, "order-commit.json");
    if (process.env.UPDATE_JOB_FIXTURE && CLI) {
      const cli = runCli(hex(input.poster), hex(ord.termsHash));
      writeFileSync(fixPath, JSON.stringify({ cli: CLI.split("/crates/")[1], header_id: hex(input.poster), order_terms: hex(ord.termsHash), max_blocks: 10, deadline_blocks: "500", ...cli }, null, 2) + "\n");
    }
    const fx = JSON.parse(readFileSync(fixPath, "utf8"));
    expect(fx.header).toBe(hex(ord.header));
    expect(fx.job_key).toBe(hex(ord.jobKey));
    expect(fx.hash_a).toBe(hex(ord.hashA));
    expect(fx.hash_b).toBe(hex(ord.hashB));
  });

  it.runIf(!!CLI)("matches a live pearl-job run for an order header", () => {
    const cli = runCli(hex(input.poster), hex(ord.termsHash));
    expect(cli.header).toBe(hex(ord.header));
    expect(cli.job_key).toBe(hex(ord.jobKey));
    expect(cli.hash_a).toBe(hex(ord.hashA));
    expect(cli.hash_b).toBe(hex(ord.hashB));
  });

  it("jobHeaderProblem checks a node job against header_id", () => {
    const j = { job_id: "ab".repeat(32), header_id: hex(input.poster), terms_hash: hex(ord.termsHash), config: LOCALNET_CONFIG_HEX, job_key: hex(ord.jobKey), job_header: hex(ord.header) };
    expect(jobHeaderProblem(j)).toBeNull();
    expect(jobHeaderProblem({ ...j, header_id: undefined })).toMatch(/job_header/);
    expect(jobHeaderProblem({ ...j, header_id: undefined, job_header: undefined })).toMatch(/job_key/);
    // a PostJob: header_id = job_id
    const ts = commitJob(input);
    expect(jobHeaderProblem({ job_id: hex(ts.jobId), header_id: hex(ts.jobId), terms_hash: hex(ts.termsHash), config: LOCALNET_CONFIG_HEX, job_key: hex(ts.jobKey), job_header: hex(ts.header) })).toBeNull();
  });
});
