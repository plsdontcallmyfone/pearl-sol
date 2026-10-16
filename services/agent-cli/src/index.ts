#!/usr/bin/env node
// pearl-agent: agent-side CLI for the pearl-sol agent chain (SPEC 3.2, 3.3, section 4).
//
//   init       --attester <pubkey> [--epoch-slots N --emission-bps N --alpha-bps N --max-nonce N]
//   register   --name <s> [--uri <s>] [--payout <pubkey>]
//   post-job   --m N --n N [--k 1024] --worker-pay <lamports> --protocol-fee <lamports>
//              [--ttl-slots 9000] [--share-target 0x207fffff] [--job-id N] [--data-dir DIR]
//              [--seed N] (data dir default: $TMPDIR/pearl-sol-jobs/<job>; or skip pearl-job with --hash-a --hash-b --config --data-uri)
//   list       [--poster <pubkey>]
//   show       --job <pubkey>
//   watch      --job <pubkey> [--interval-ms 2000] [--follow]
//   accept     --job <pubkey> [--check]   (--check runs a Freivalds check via pearl-job first)
//   expire     --job <pubkey>
//   selftest   [--pearl-job]  (offline: terms_hash/header vs the Rust vector; with
//              --pearl-job also a gen/config/commit round trip through pearl-job)
//
// Common flags: --rpc (default http://127.0.0.1:10630), --keypair (default
// keys/localnet/payer-keypair.json), --idl-dir (default agent/target/idl).

import * as anchor from "@coral-xyz/anchor";
import { BN } from "@coral-xyz/anchor";
import { Connection, Keypair, PublicKey, SystemProgram} from "@solana/web3.js";
import { createHash, randomBytes } from "crypto";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { pearlJob } from "./pearljob";

const REPO = path.resolve(__dirname, "..", "..", "..");
const AGENTS_ID = new PublicKey("7C92xwB9g7vStXqtwJLmuioFofKQjaxb8ATYAoNVkpit");
const JOBS_ID = new PublicKey("7e9pEhjwEqKemjC6zUugp4isi2zQMvaZPhn9kdnDuzqL");
const MINING_ID = new PublicKey("3GrrgQMSHC3PjkW6UmvQJPGK1WBvhxiz3tkigafj3GhP");
const TERMS_DOMAIN = Buffer.from("pearl-sol:terms:v1");

// ---------- args ----------

type Args = { cmd: string; flags: Record<string, string> };
function parseArgs(argv: string[]): Args {
  const [cmd = "help", ...rest] = argv;
  const flags: Record<string, string> = {};
  for (let i = 0; i < rest.length; i++) {
    const a = rest[i];
    if (!a.startsWith("--")) throw new Error(`unexpected argument ${a}`);
    const k = a.slice(2);
    const next = rest[i + 1];
    if (next === undefined || next.startsWith("--")) flags[k] = "true";
    else {
      flags[k] = next;
      i++;
    }
  }
  return { cmd, flags };
}
function need(f: Record<string, string>, k: string): string {
  if (f[k] === undefined) throw new Error(`missing --${k}`);
  return f[k];
}
function num(s: string): bigint {
  return BigInt(s);
}

// ---------- pure helpers (mirrors of jobs::terms_hash / build_header) ----------

export type Terms = {
  jobId: bigint;
  m: number;
  n: number;
  k: number;
  workerPay: bigint;
  protocolFee: bigint;
  deadlineSlot: bigint;
  shareTarget: number;
};

export function termsHash(poster: PublicKey, t: Terms): Buffer {
  const b = Buffer.alloc(8 + 4 * 3 + 8 * 3 + 4);
  let o = 0;
  b.writeBigUInt64LE(t.jobId, o); o += 8;
  b.writeUInt32LE(t.m, o); o += 4;
  b.writeUInt32LE(t.n, o); o += 4;
  b.writeUInt32LE(t.k, o); o += 4;
  b.writeBigUInt64LE(t.workerPay, o); o += 8;
  b.writeBigUInt64LE(t.protocolFee, o); o += 8;
  b.writeBigUInt64LE(t.deadlineSlot, o); o += 8;
  b.writeUInt32LE(t.shareTarget >>> 0, o);
  return createHash("sha256").update(TERMS_DOMAIN).update(poster.toBuffer()).update(b).digest();
}

export function buildHeader(job: PublicKey, terms: Buffer, createdSlot: bigint, targetBits: number): Buffer {
  const h = Buffer.alloc(76);
  job.toBuffer().copy(h, 0);
  terms.copy(h, 32);
  h.writeBigUInt64LE(createdSlot, 64);
  h.writeUInt32LE(targetBits >>> 0, 72);
  return h;
}

export function jobPda(poster: PublicKey, jobId: bigint): PublicKey {
  const id = Buffer.alloc(8);
  id.writeBigUInt64LE(jobId);
  return PublicKey.findProgramAddressSync([Buffer.from("job"), poster.toBuffer(), id], JOBS_ID)[0];
}
const pda = (seeds: (Buffer | Uint8Array)[], p: PublicKey) => PublicKey.findProgramAddressSync(seeds, p)[0];
const u64le = (v: bigint | number) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(v));
  return b;
};
const miningConfigPda = () => pda([Buffer.from("config")], MINING_ID);
const miningPoolPda = () => pda([Buffer.from("mining_pool")], MINING_ID);
const epochPda = (i: bigint | number) => pda([Buffer.from("epoch"), u64le(i)], MINING_ID);
const jobEpochPda = (i: bigint | number, job: PublicKey) =>
  pda([Buffer.from("job_epoch"), u64le(i), job.toBuffer()], MINING_ID);
const jobsConfigPda = () => pda([Buffer.from("jobs_config")], JOBS_ID);
const agentPda = (a: PublicKey) => pda([Buffer.from("agent"), a.toBuffer()], AGENTS_ID);

const hex = (b: number[] | Uint8Array | Buffer) => Buffer.from(b).toString("hex");
const unhex = (s: string, len: number): number[] => {
  const b = Buffer.from(s.replace(/^0x/, ""), "hex");
  if (b.length !== len) throw new Error(`expected ${len} bytes of hex, got ${b.length}`);
  return [...b];
};
const statusName = (s: any) => Object.keys(s)[0];

// ---------- chain context ----------

type Ctx = {
  conn: Connection;
  me: Keypair;
  agents: anchor.Program;
  jobs: anchor.Program;
  mining: anchor.Program;
};

function loadKeypair(p: string): Keypair {
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(p, "utf8"))));
}

function ctx(f: Record<string, string>): Ctx {
  const rpc = f.rpc ?? process.env.AGENT_RPC ?? "http://127.0.0.1:10630";
  const kp = f.keypair ?? path.join(REPO, "keys", "localnet", "payer-keypair.json");
  const idlDir = f["idl-dir"] ?? path.join(REPO, "agent", "target", "idl");
  const conn = new Connection(rpc, "confirmed");
  const me = loadKeypair(kp);
  const provider = new anchor.AnchorProvider(conn, new anchor.Wallet(me), { commitment: "confirmed" });
  const load = (n: string) => new anchor.Program(JSON.parse(fs.readFileSync(path.join(idlDir, `${n}.json`), "utf8")), provider);
  return { conn, me, agents: load("agents"), jobs: load("jobs"), mining: load("mining") };
}

const out = (v: any) =>
  console.log(JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : BN.isBN(x) ? x.toString() : x)));

// ---------- commands ----------

async function cmdInit(f: Record<string, string>) {
  const c = ctx(f);
  const att = new PublicKey(need(f, "attester"));
  const res: any = {};
  if (!(await c.conn.getAccountInfo(miningConfigPda()))) {
    const params = {
      admin: null,
      attesters: [att],
      threshold: null,
      epochSlots: f["epoch-slots"] ? new BN(f["epoch-slots"]) : null,
      emissionBps: f["emission-bps"] ? Number(f["emission-bps"]) : null,
      alphaBps: f["alpha-bps"] ? Number(f["alpha-bps"]) : null,
      maxNonce: f["max-nonce"] ? Number(f["max-nonce"]) : null,
    };
    res.mining = await c.mining.methods
      .initialize(params)
      .accountsPartial({
        admin: c.me.publicKey,
        config: miningConfigPda(),
        miningPool: miningPoolPda(),
        epoch: epochPda(0),
        systemProgram: SystemProgram.programId,
      })
      .rpc();
  } else res.mining = "exists";
  if (!(await c.conn.getAccountInfo(jobsConfigPda()))) {
    const params = {
      admin: null,
      evaluators: [att],
      threshold: null,
      minProtocolFee: null,
      maxSlotLag: null,
      maxDeadlineSlots: null,
    };
    res.jobs = await c.jobs.methods
      .initializeConfig(params)
      .accountsPartial({ admin: c.me.publicKey, config: jobsConfigPda(), systemProgram: SystemProgram.programId })
      .rpc();
  } else res.jobs = "exists";
  out({ ok: true, ...res, mining_pool: miningPoolPda().toBase58() });
}

async function cmdRegister(f: Record<string, string>) {
  const c = ctx(f);
  const payout = f.payout ? new PublicKey(f.payout) : c.me.publicKey;
  const sig = await c.agents.methods
    .register(need(f, "name"), f.uri ?? "", payout)
    .accountsPartial({ authority: c.me.publicKey, profile: agentPda(c.me.publicKey), systemProgram: SystemProgram.programId })
    .rpc();
  out({ ok: true, sig, profile: agentPda(c.me.publicKey).toBase58() });
}

async function cmdPostJob(f: Record<string, string>) {
  const c = ctx(f);
  const poster = c.me.publicKey;
  const m = Number(need(f, "m"));
  const n = Number(need(f, "n"));
  const k = Number(f.k ?? "1024");
  const workerPay = num(need(f, "worker-pay"));
  const protocolFee = num(need(f, "protocol-fee"));
  const shareTarget = Number(f["share-target"] ?? "0x207fffff");
  const jobId = f["job-id"] ? num(f["job-id"]) : randomBytes(8).readBigUInt64LE();
  const job = jobPda(poster, jobId);
  const createdSlot = BigInt(await c.conn.getSlot("confirmed"));
  const deadlineSlot = createdSlot + BigInt(f["ttl-slots"] ?? "9000");
  const terms: Terms = { jobId, m, n, k, workerPay, protocolFee, deadlineSlot, shareTarget };
  const th = termsHash(poster, terms);
  const header = buildHeader(job, th, createdSlot, shareTarget);

  let hashA: string, hashB: string, config: string, dataUri: string;
  if (f["hash-a"]) {
    hashA = need(f, "hash-a");
    hashB = need(f, "hash-b");
    config = need(f, "config");
    dataUri = need(f, "data-uri");
  } else {
    const dir = path.resolve(f["data-dir"] ?? path.join(os.tmpdir(), "pearl-sol-jobs", job.toBase58()));
    fs.mkdirSync(dir, { recursive: true });
    const g = pearlJob(REPO, ["gen", "--m", `${m}`, "--n", `${n}`, "--k", `${k}`, "--out-dir", dir, ...(f.seed ? ["--seed", f.seed] : [])]);
    dataUri = g.data_uri;
    config = f.config ?? pearlJob(REPO, ["config", "--k", `${k}`]).config;
    const cm = pearlJob(REPO, [
      "commit",
      "--job", job.toBase58(),
      "--terms-hash", th.toString("hex"),
      "--slot", createdSlot.toString(),
      "--target-bits", `${shareTarget}`,
      "--config", config,
      "--data", dataUri,
    ]);
    if (cm.header !== header.toString("hex")) {
      throw new Error(`header mismatch: pearl-job ${cm.header} vs cli ${header.toString("hex")}`);
    }
    hashA = cm.hash_a;
    hashB = cm.hash_b;
  }

  const p = {
    jobId: new BN(jobId.toString()),
    hashA: unhex(hashA, 32),
    hashB: unhex(hashB, 32),
    config: unhex(config, 52),
    m, n, k,
    dataUri,
    workerPay: new BN(workerPay.toString()),
    protocolFee: new BN(protocolFee.toString()),
    createdSlot: new BN(createdSlot.toString()),
    deadlineSlot: new BN(deadlineSlot.toString()),
    shareTarget,
  };
  const sig = await c.jobs.methods
    .postJob(p)
    .accountsPartial({
      poster,
      config: jobsConfigPda(),
      job,
      miningPool: miningPoolPda(),
      systemProgram: SystemProgram.programId,
    })
    .rpc();

  // JobSpec for `pearl-job --job-file` (miner, attester, freivalds)
  let maxNonce = 0;
  try {
    maxNonce = ((await (c.mining.account as any).miningConfig.fetch(miningConfigPda())) as any).maxNonce;
  } catch {}
  const spec = {
    job: job.toBase58(),
    terms_hash: th.toString("hex"),
    created_slot: Number(createdSlot),
    target_bits: shareTarget,
    config,
    m, n,
    hash_a: hashA,
    hash_b: hashB,
    data_uri: dataUri,
    max_nonce: maxNonce,
  };
  let specPath: string | undefined;
  if (dataUri.startsWith("file://")) {
    specPath = path.join(path.dirname(dataUri.slice(7)), "job.json");
    fs.writeFileSync(specPath, JSON.stringify(spec, null, 2));
  }
  out({ ok: true, sig, job_id: jobId, header: header.toString("hex"), job_file: specPath, ...spec });
}

async function fetchJob(c: Ctx, job: PublicKey): Promise<any> {
  return (c.jobs.account as any).job.fetch(job);
}

function jobView(job: PublicKey, j: any) {
  return {
    job: job.toBase58(),
    status: statusName(j.status),
    poster: j.poster.toBase58(),
    worker_pay: j.workerPay.toString(),
    protocol_fee: j.protocolFee.toString(),
    m: j.m, n: j.n, k: j.k,
    deadline_slot: j.deadlineSlot.toString(),
    share_target: j.shareTarget,
    data_uri: j.dataUri,
    header: hex(j.header),
    terms_hash: hex(j.termsHash),
    hash_a: hex(j.hashA),
    hash_b: hex(j.hashB),
    deliverer: j.deliverer.toBase58(),
    result_hash: hex(j.resultHash),
    result_uri: j.resultUri,
  };
}

async function cmdShow(f: Record<string, string>) {
  const c = ctx(f);
  const job = new PublicKey(need(f, "job"));
  out({ ok: true, ...jobView(job, await fetchJob(c, job)) });
}

async function cmdList(f: Record<string, string>) {
  const c = ctx(f);
  const filters = f.poster ? [{ memcmp: { offset: 8, bytes: new PublicKey(f.poster).toBase58() } }] : [];
  const all = await (c.jobs.account as any).job.all(filters);
  out({ ok: true, jobs: all.map((a: any) => jobView(a.publicKey, a.account)) });
}

async function cmdWatch(f: Record<string, string>) {
  const c = ctx(f);
  const job = new PublicKey(need(f, "job"));
  const interval = Number(f["interval-ms"] ?? "2000");
  const follow = f.follow === "true";
  let last = "";
  for (;;) {
    const j = await fetchJob(c, job);
    let shares: string | null = null;
    let epoch: string | null = null;
    try {
      const cfg: any = await (c.mining.account as any).miningConfig.fetch(miningConfigPda());
      epoch = cfg.currentEpoch.toString();
      const je: any = await (c.mining.account as any).jobEpoch.fetchNullable(jobEpochPda(BigInt(epoch!), job));
      shares = je ? je.totalShares.toString() : "0";
    } catch {}
    const v = {
      slot: await c.conn.getSlot("confirmed"),
      status: statusName(j.status),
      deliverer: j.deliverer.toBase58(),
      result_hash: hex(j.resultHash),
      epoch,
      shares_this_epoch: shares,
    };
    const key = JSON.stringify({ ...v, slot: 0 });
    if (key !== last) {
      out({ ok: true, job: job.toBase58(), ...v });
      last = key;
    }
    if (!follow && ["completed", "expired", "refunded"].includes(v.status)) return;
    await new Promise((r) => setTimeout(r, interval));
  }
}

async function cmdAccept(f: Record<string, string>) {
  const c = ctx(f);
  const job = new PublicKey(need(f, "job"));
  const j = await fetchJob(c, job);
  const st = statusName(j.status);
  if (st !== "delivered" && st !== "disputed") throw new Error(`job is ${st}, not delivered`);
  let check: any = undefined;
  if (f.check === "true") {
    if (!j.resultUri.startsWith("file://")) throw new Error(`--check needs a file:// result_uri, got ${j.resultUri}`);
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pearl-accept-"));
    const specPath = path.join(dir, "job.json");
    fs.writeFileSync(
      specPath,
      JSON.stringify({
        job: job.toBase58(),
        terms_hash: hex(j.termsHash),
        created_slot: Number(j.createdSlot.toString()),
        target_bits: j.shareTarget,
        config: hex(j.config),
        m: j.m,
        n: j.n,
        hash_a: hex(j.hashA),
        hash_b: hex(j.hashB),
        data_uri: j.dataUri,
      }),
    );
    check = pearlJob(REPO, ["freivalds", "--job-file", specPath, "--data", j.dataUri, "--c", j.resultUri.slice(7)]);
    if (!check.valid) throw new Error("Freivalds check failed: C != AB; not accepting (consider dispute)");
    if (check.result_hash !== hex(j.resultHash)) throw new Error("delivered C does not match the committed result_hash");
  }
  const sig = await c.jobs.methods
    .accept()
    .accountsPartial({ poster: c.me.publicKey, job, deliverer: j.deliverer })
    .rpc();
  out({ ok: true, sig, job: job.toBase58(), paid: j.workerPay.toString(), to: j.deliverer.toBase58(), check });
}

async function cmdExpire(f: Record<string, string>) {
  const c = ctx(f);
  const job = new PublicKey(need(f, "job"));
  const j = await fetchJob(c, job);
  const sig = await c.jobs.methods.expire().accountsPartial({ job, poster: j.poster }).rpc();
  out({ ok: true, sig, job: job.toBase58(), refunded: j.workerPay.toString() });
}

function cmdSelftest(f: Record<string, string>) {
  // Same vector as jobs::tests::terms_hash_vector (agent/programs/jobs/src/lib.rs).
  const poster = new PublicKey(Buffer.alloc(32, 1));
  const th = termsHash(poster, {
    jobId: 7n, m: 64, n: 128, k: 1024,
    workerPay: 2_000_000_000n, protocolFee: 500_000_000n, deadlineSlot: 123_456n, shareTarget: 0x207fffff,
  });
  const want = "a7769ee3567d6dff4da7a3a846682838cd07084c3d506c7568331d798b7e0031";
  if (th.toString("hex") !== want) throw new Error(`terms_hash ${th.toString("hex")} != ${want}`);
  const h = buildHeader(new PublicKey(Buffer.alloc(32, 2)), th, 99n, 0x207fffff);
  if (h.length !== 76 || h.readBigUInt64LE(64) !== 99n || h.subarray(72).toString("hex") !== "ffff7f20")
    throw new Error("header layout");
  const res: any = { ok: true, terms_hash: th.toString("hex"), header: h.toString("hex") };
  if (f["pearl-job"] === "true") {
    // offline round trip through pearl-job: gen -> config -> commit, header must match ours
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "pearl-selftest-"));
    const job = Keypair.generate().publicKey;
    const g = pearlJob(REPO, ["gen", "--m", "64", "--n", "64", "--k", "1024", "--out-dir", dir, "--seed", "1"]);
    const config = pearlJob(REPO, ["config", "--k", "1024"]).config;
    const hh = buildHeader(job, th, 1234n, 0x207fffff);
    const cm = pearlJob(REPO, ["commit", "--job", job.toBase58(), "--terms-hash", th.toString("hex"), "--slot", "1234",
      "--target-bits", `${0x207fffff}`, "--config", config, "--data", g.data_uri]);
    if (cm.header !== hh.toString("hex")) throw new Error(`pearl-job header ${cm.header} != ${hh.toString("hex")}`);
    res.pearl_job = { header_match: true, hash_a: cm.hash_a, hash_b: cm.hash_b, job_key: cm.job_key, config_len: config.length / 2 };
  }
  out(res);
}

async function main() {
  const { cmd, flags } = parseArgs(process.argv.slice(2));
  switch (cmd) {
    case "init": return cmdInit(flags);
    case "register": return cmdRegister(flags);
    case "post-job": return cmdPostJob(flags);
    case "list": return cmdList(flags);
    case "show": return cmdShow(flags);
    case "watch": return cmdWatch(flags);
    case "accept": return cmdAccept(flags);
    case "expire": return cmdExpire(flags);
    case "selftest": return cmdSelftest(flags);
    default:
      console.log(fs.readFileSync(__filename, "utf8").split("\n").filter((l) => l.startsWith("//")).slice(0, 19).map((l) => l.replace(/^\/\/ ?/, "")).join("\n"));
  }
}

main().catch((e) => {
  out({ ok: false, error: String(e?.message ?? e) });
  process.exit(1);
});

