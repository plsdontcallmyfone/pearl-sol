// Relayer tests against two in-process LiteSVM "chains" running the real
// bridge.so and escrow.so. Build both programs first:
//   (cd token && anchor build -p bridge) && (cd agent && anchor build -p escrow)

import assert from "node:assert/strict";
import { createPublicKey, verify as edVerify } from "node:crypto";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";
import type { AddressInfo } from "node:net";
import { Keypair, PublicKey, TransactionInstruction } from "@solana/web3.js";
import { LiteSVM } from "litesvm";
import {
  BRIDGE_PROGRAM_ID,
  ESCROW_PROGRAM_ID,
  GENESIS_ESCROW_LAMPORTS,
  TOKEN_2022_PROGRAM_ID,
  ata,
  bridgeDepositIx,
  bridgeInitializeIx,
  createAtaIdempotentIx,
  depositMessage,
  ed25519Sign,
  escrowInitializeIx,
  escrowLockIx,
  escrowPda,
  tokenAccountAmount,
  u64le,
  vaultAta,
  withdrawMessage,
} from "../src/codec.ts";
import { startHealth } from "../src/health.ts";
import { Relayer, loadCursors } from "../src/relayer.ts";
import { RpcChain } from "../src/chain.ts";
import { createServer } from "node:http";
import { REPO_ROOT } from "../src/config.ts";
import { LiteSvmChain } from "./litesvm-chain.ts";

const BRIDGE_SO = join(REPO_ROOT, "token/target/deploy/bridge.so");
const ESCROW_SO = join(REPO_ROOT, "agent/target/deploy/escrow.so");
const SOL = 1_000_000_000n;

interface World {
  token: LiteSvmChain;
  agent: LiteSvmChain;
  mint: PublicKey;
  mintAuthority: Keypair;
  payer: Keypair;
  signer: Keypair;
  cursorFile: string;
}

function world(): World {
  const payer = Keypair.generate();
  const signer = Keypair.generate();
  const mintAuthority = Keypair.generate();

  const tsvm = new LiteSVM();
  tsvm.addProgramFromFile(BRIDGE_PROGRAM_ID, BRIDGE_SO);
  tsvm.airdrop(payer.publicKey, 100n * SOL);
  tsvm.airdrop(mintAuthority.publicKey, 10n * SOL);
  const token = new LiteSvmChain("token", tsvm);

  const asvm = new LiteSVM();
  asvm.addProgramFromFile(ESCROW_PROGRAM_ID, ESCROW_SO);
  asvm.airdrop(payer.publicKey, 100n * SOL);
  asvm.setAccount(escrowPda(), {
    lamports: Number(GENESIS_ESCROW_LAMPORTS), // 1e18 is an exact double
    data: new Uint8Array(0),
    owner: ESCROW_PROGRAM_ID,
    executable: false,
  });
  const agent = new LiteSvmChain("agent", asvm);

  // Plain Token-2022 mint, 6 dp (initialized in boot()).
  const mint = Keypair.generate().publicKey;
  tsvm.setAccount(mint, {
    lamports: Number(tsvm.minimumBalanceForRentExemption(82n)),
    data: new Uint8Array(82),
    owner: TOKEN_2022_PROGRAM_ID,
    executable: false,
  });
  const cursorFile = join(mkdtempSync(join(tmpdir(), "relayer-test-")), "cursor.json");
  return { token, agent, mint, mintAuthority, payer, signer, cursorFile };
}

async function boot(w: World, opts: { escrowThreshold?: number; escrowSigners?: PublicKey[] } = {}): Promise<void> {
  const initMint = new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [{ pubkey: w.mint, isSigner: false, isWritable: true }],
    data: Buffer.concat([Buffer.from([20, 6]), w.mintAuthority.publicKey.toBuffer(), Buffer.from([0])]),
  });
  await w.token.send([initMint], w.payer);
  await w.token.send([bridgeInitializeIx(w.payer.publicKey, w.mint, [w.signer.publicKey], 1)], w.payer);
  await w.agent.send(
    [escrowInitializeIx(w.payer.publicKey, opts.escrowSigners ?? [w.signer.publicKey], opts.escrowThreshold ?? 1)],
    w.payer,
  );
}

function relayer(w: World, extra: Partial<ConstructorParameters<typeof Relayer>[0]> = {}): Relayer {
  return new Relayer({
    token: w.token,
    agent: w.agent,
    signers: [w.signer],
    tokenPayer: w.payer,
    agentPayer: w.payer,
    cursorFile: w.cursorFile,
    log: () => {},
    ...extra,
  });
}

/** A token-chain user with `amount` SPL who deposits it all to `agentRecipient`. */
async function userDeposits(w: World, amount: bigint, agentRecipient: PublicKey, nonce: bigint): Promise<Keypair> {
  const user = Keypair.generate();
  w.token.svm.airdrop(user.publicKey, SOL);
  const mintTo = new TransactionInstruction({
    programId: TOKEN_2022_PROGRAM_ID,
    keys: [
      { pubkey: w.mint, isSigner: false, isWritable: true },
      { pubkey: ata(user.publicKey, w.mint), isSigner: false, isWritable: true },
      { pubkey: w.mintAuthority.publicKey, isSigner: true, isWritable: false },
    ],
    data: Buffer.concat([Buffer.from([7]), u64le(amount)]),
  });
  await w.token.send([createAtaIdempotentIx(user.publicKey, user.publicKey, w.mint), mintTo], user, [w.mintAuthority]);
  await w.token.send(
    [bridgeDepositIx({ payer: user.publicKey, sender: user.publicKey, mint: w.mint, nonce, amount, agentRecipient })],
    user,
  );
  return user;
}

const lamports = (c: LiteSvmChain, k: PublicKey): bigint => BigInt(c.svm.getBalance(k) ?? 0n);
const spl = (c: LiteSvmChain, k: PublicKey): bigint => {
  const a = c.svm.getAccount(k);
  return a ? tokenAccountAmount(a.data) : 0n;
};

describe("codec", () => {
  it("builds SPEC section 6 messages byte for byte", () => {
    const r = new PublicKey(new Uint8Array(32).fill(7));
    const m = depositMessage(258n, r, 1_000_000n);
    assert.equal(m.length, 20 + 8 + 32 + 8);
    assert.equal(m.subarray(0, 20).toString(), "pearl-sol:deposit:v1");
    assert.deepEqual([...m.subarray(20, 28)], [2, 1, 0, 0, 0, 0, 0, 0]);
    assert.ok(m.subarray(28, 60).equals(r.toBuffer()));
    assert.equal(m.readBigUInt64LE(60), 1_000_000n);
    const wm = withdrawMessage(1n, r, 5n);
    assert.equal(wm.length, 21 + 48);
    assert.equal(wm.subarray(0, 21).toString(), "pearl-sol:withdraw:v1");
  });

  it("signs ed25519 the way Solana keys verify", () => {
    const kp = Keypair.generate();
    const msg = Buffer.from("hello");
    const sig = ed25519Sign(kp, msg);
    const spki = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), kp.publicKey.toBuffer()]);
    assert.ok(edVerify(null, msg, createPublicKey({ key: spki, format: "der", type: "spki" }), sig));
  });
});

describe("relayer over LiteSVM", () => {
  it("relays a deposit and a withdrawal, keeps the invariant, never double-relays", async () => {
    const w = world();
    await boot(w);
    const rl = relayer(w);

    // Quiescent start: vault 0, escrow full.
    let t = await rl.tick();
    assert.equal(t.refused, null);
    assert.ok(t.invariant.ok && t.invariant.strictFormHolds);

    // Solana user bridges 5 tokens in to an agent-chain account.
    const agentUser = Keypair.generate();
    const solUser = await userDeposits(w, 5_000_000n, agentUser.publicKey, 0n);
    t = await rl.tick();
    assert.equal(t.refused, null, String(t.errors));
    assert.deepEqual(t.unlocked, ["0"]);
    assert.equal(lamports(w.agent, agentUser.publicKey), 5_000_000_000n);
    assert.equal(lamports(w.agent, escrowPda()), GENESIS_ESCROW_LAMPORTS - 5_000_000_000n);

    // Next tick: invariant strict, nothing to do.
    t = await rl.tick();
    assert.deepEqual(t.unlocked, []);
    assert.ok(t.invariant.strictFormHolds);
    assert.equal(rl.cursors.deposit, 1n);

    // Agent user bridges 2 tokens back out to their Solana wallet.
    w.agent.svm.airdrop(agentUser.publicKey, SOL); // fees + Withdrawal rent
    await w.agent.send(
      [escrowLockIx({ sender: agentUser.publicKey, nonce: 0n, amountLamports: 2_000_000_000n, solanaRecipient: solUser.publicKey })],
      agentUser,
    );
    t = await rl.tick();
    assert.equal(t.refused, null, String(t.errors));
    assert.deepEqual(t.released, ["0"]);
    assert.equal(spl(w.token, ata(solUser.publicKey, w.mint)), 2_000_000n);
    assert.equal(spl(w.token, vaultAta(w.mint)), 3_000_000n);

    t = await rl.tick();
    assert.deepEqual(t.released, []);
    assert.ok(t.invariant.ok && t.invariant.strictFormHolds);
    assert.equal(t.invariant.vaultSpl, "3000000");
    assert.equal(t.invariant.circulatingLamports, "3000000000");

    // Cursor persisted; a fresh relayer (restart) resumes and does nothing.
    assert.deepEqual(loadCursors(w.cursorFile), { deposit: 1n, withdrawal: 1n });
    const rl2 = relayer(w);
    assert.deepEqual(rl2.cursors, { deposit: 1n, withdrawal: 1n });
    t = await rl2.tick();
    assert.deepEqual([t.unlocked, t.released, t.refused], [[], [], null]);
  });

  it("refuses to sign when the invariant breaks, and resumes when it is restored", async () => {
    const w = world();
    await boot(w);
    const rl = relayer(w);
    const agentUser = Keypair.generate();
    await userDeposits(w, 1_000_000n, agentUser.publicKey, 0n);

    // Lamports leave the escrow outside the bridge (as a bug or exploit would).
    // (Multiples of 1024 so the change survives LiteSVM's JS-number lamports near 1e18.)
    const e = w.agent.svm.getAccount(escrowPda())!;
    w.agent.svm.setAccount(escrowPda(), { ...e, lamports: e.lamports - 7 * 1024 });
    let t = await rl.tick();
    assert.match(String(t.refused), /invariant/);
    assert.equal(t.invariant.deltaLamports, String(-7 * 1024));
    assert.deepEqual(t.unlocked, []);
    assert.equal(lamports(w.agent, agentUser.publicKey), 0n);
    assert.equal(rl.health(2000).ok, false);

    // Surplus (donation into the escrow) is refused only in strict mode...
    w.agent.svm.setAccount(escrowPda(), { ...e, lamports: e.lamports + 3 * 1024 });
    t = await relayer(w, { allowSurplus: false }).tick();
    assert.match(String(t.refused), /invariant/);
    // ...and accepted by default, since over-backing cannot be exploited.
    t = await rl.tick();
    assert.equal(t.refused, null);
    assert.deepEqual(t.unlocked, ["0"]);
    assert.equal(lamports(w.agent, agentUser.publicKey), 1_000_000_000n);
  });

  it("does not submit without enough local signer keys for the threshold", async () => {
    const w = world();
    const other = Keypair.generate();
    await boot(w, { escrowThreshold: 2, escrowSigners: [w.signer.publicKey, other.publicKey] });
    const agentUser = Keypair.generate();
    await userDeposits(w, 1_000_000n, agentUser.publicKey, 0n);

    let t = await relayer(w).tick();
    assert.deepEqual(t.unlocked, []);
    assert.ok(t.errors.some((e) => /fewer than 2/.test(e)), String(t.errors));

    t = await relayer(w, { signers: [w.signer, other] }).tick();
    assert.deepEqual(t.unlocked, ["0"]);
    assert.equal(lamports(w.agent, agentUser.publicKey), 1_000_000_000n);
  });

  it("dry run checks but never submits", async () => {
    const w = world();
    await boot(w);
    const agentUser = Keypair.generate();
    await userDeposits(w, 1_000_000n, agentUser.publicKey, 0n);
    const t = await relayer(w, { dryRun: true }).tick();
    assert.deepEqual(t.pendingDeposits, ["0"]);
    assert.ok(t.invariant.ok);
    assert.deepEqual(t.unlocked, []);
    assert.equal(lamports(w.agent, agentUser.publicKey), 0n);
  });
});

describe("health endpoint", () => {
  let server: ReturnType<typeof startHealth>;
  let ok = true;
  before(() => {
    server = startHealth(0, () => ({ ok, ticks: 1 })); // ephemeral port for the test
  });
  after(() => server.close());
  it("serves JSON, 200 when healthy and 503 when not", async () => {
    await new Promise((r) => server.once("listening", r));
    const port = (server.address() as AddressInfo).port;
    let res = await fetch(`http://127.0.0.1:${port}/health`);
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { ok: true, ticks: 1 });
    ok = false;
    res = await fetch(`http://127.0.0.1:${port}/health`);
    assert.equal(res.status, 503);
    assert.equal((await fetch(`http://127.0.0.1:${port}/nope`)).status, 404);
  });
});

describe("RpcChain (mocked JSON-RPC)", () => {
  it("keeps 1e18-scale lamports exact and decodes account data", async () => {
    const owner = Keypair.generate().publicKey;
    let seen: unknown;
    const srv = createServer((req, res) => {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        seen = JSON.parse(body);
        res.writeHead(200, { "content-type": "application/json" });
        // Raw text: 999999999999999999 is not representable as a JS number.
        res.end(`{"jsonrpc":"2.0","id":1,"result":{"context":{"slot":5},"value":[{"lamports":999999999999999999,` +
          `"owner":"${owner.toBase58()}","data":["AQID","base64"],"executable":false,"rentEpoch":18446744073709551615,"space":3},null]}}`);
      });
    }).listen(0, "127.0.0.1");
    await new Promise((r) => srv.once("listening", r));
    try {
      const port = (srv.address() as AddressInfo).port;
      const chain = new RpcChain("mock", `http://127.0.0.1:${port}`);
      const [a, b] = await chain.getAccounts([escrowPda(), Keypair.generate().publicKey]);
      assert.equal(a!.lamports, 999_999_999_999_999_999n);
      assert.ok(a!.owner.equals(owner));
      assert.deepEqual([...a!.data], [1, 2, 3]);
      assert.equal(b, null);
      assert.equal((seen as { method: string }).method, "getMultipleAccounts");
    } finally {
      srv.close();
    }
  });
});
