// Solana side: instruction data and account lists against the IDLs (Anchor's own
// Borsh coder), PDAs and message bytes against @solana/web3.js.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { BN, BorshAccountsCoder, BorshInstructionCoder } from "@coral-xyz/anchor";
import { Keypair, PublicKey, SystemProgram, Transaction, TransactionInstruction, TransactionMessage, VersionedTransaction } from "@solana/web3.js";
import { describe, expect, it } from "vitest";
import { b58, fromHex, hex } from "../src/core/bytes";
import { pubkeyOf, signRaw, verifyRaw } from "../src/core/crypto";
import {
  bridgeConfigPda, bridgeDepositIx, buybackVaultPda, buyJobAmmIx, buyJobIx, cancelDraftIx, computeUnitLimit, createAtaIdempotent, decodeBridgeConfig, depositPda, draftPda, openOrderIx,
  orderPda, processedPda, systemTransfer, transferChecked, traderPda, StoreJobSpec, BridgeConfig,
} from "../src/solana/programs";
import { Instruction, TOKEN_2022, ata, buildSigned, compileLegacy, findPda, insertSignature, parseMessage, parseTransaction, pk, serializeMessage } from "../src/solana/tx";

const idl = (n: string) => JSON.parse(readFileSync(resolve(__dirname, `../../token/target/idl/${n}.json`), "utf8"));
const bridgeIdl = idl("bridge");
const sfIdl = idl("storefront");
const BRIDGE = pk(bridgeIdl.address);
const SF = pk(sfIdl.address);

const seed = fromHex("b5802e2c4b58c29279a877868859e11d9c2ae86ee8bdb15ab912176fa6e74c9e");
const me = pubkeyOf(seed);
const mint = pk("8eJrnVjzdEYGYMF8hx1bTfxi5wSANT41fV5c3bCKpump");
const P = (b: Uint8Array) => new PublicKey(b);

/** The IDL's view of an instruction's accounts: writable/signer flags and fixed addresses must match. */
function checkAccounts(ix: Instruction, idlJson: any, name: string) {
  const def = idlJson.instructions.find((i: any) => i.name === name);
  expect(ix.keys.length).toBe(def.accounts.length);
  def.accounts.forEach((a: any, i: number) => {
    const k = ix.keys[i];
    expect([a.name, k.isWritable], `${name}.${a.name} writable`).toEqual([a.name, !!a.writable]);
    expect([a.name, k.isSigner], `${name}.${a.name} signer`).toEqual([a.name, !!a.signer]);
    if (a.address) expect(b58(k.pubkey), `${name}.${a.name} address`).toBe(a.address);
    if (a.pda && a.pda.seeds.every((s: any) => s.kind === "const")) {
      const [want] = findPda(a.pda.seeds.map((s: any) => Uint8Array.from(s.value)), pk(idlJson.address));
      expect(b58(k.pubkey), `${name}.${a.name} pda`).toBe(b58(want));
    }
  });
}

const spec: StoreJobSpec = {
  m: 64, n: 64, k: 2048, config: new Uint8Array(52).fill(3), hashA: new Uint8Array(32).fill(0xaa), hashB: new Uint8Array(32).fill(0xbb),
  dataUri: "ipfs://example-job", deadlineBlocks: 500n, maxBlocks: 10, evaluators: [me, mint], threshold: 1,
};

describe("bridge IDL", () => {
  const cfg: BridgeConfig = { admin: me, mint, vault: ata(bridgeConfigPda(BRIDGE), mint, TOKEN_2022), tokenProgram: TOKEN_2022, depositNonce: 7n, threshold: 1 };
  it("deposit data = Anchor coder", () => {
    const ix = bridgeDepositIx(BRIDGE, cfg, me, 1_234_567n, mint);
    const want = new BorshInstructionCoder(bridgeIdl).encode("deposit", { amount: new BN(1_234_567), agent_recipient: P(mint) });
    expect(hex(ix.data)).toBe(hex(new Uint8Array(want)));
    checkAccounts(ix, bridgeIdl, "deposit");
    expect(b58(ix.keys[6].pubkey)).toBe(PublicKey.findProgramAddressSync([Buffer.from("deposit"), new BN(7).toArrayLike(Buffer, "le", 8)], P(BRIDGE))[0].toBase58());
  });
  it("Config decoder reads the IDL layout", () => {
    const coder = new BorshAccountsCoder(bridgeIdl);
    return coder.encode("Config", { admin: P(me), mint: P(mint), vault: P(cfg.vault), token_program: P(TOKEN_2022), deposit_nonce: new BN(42), threshold: 2, bump: 254, signers: [P(me)] }).then((buf: Buffer) => {
      const c = decodeBridgeConfig(new Uint8Array(buf));
      expect(b58(c.mint)).toBe(b58(mint));
      expect(c.depositNonce).toBe(42n);
      expect(c.threshold).toBe(2);
    });
  });
  it("processed PDA seeds", () => {
    expect(b58(processedPda(BRIDGE, 9n))).toBe(PublicKey.findProgramAddressSync([Buffer.from("processed"), Buffer.from([1]), new BN(9).toArrayLike(Buffer, "le", 8)], P(BRIDGE))[0].toBase58());
  });
});

describe("storefront IDL", () => {
  const coder = new BorshInstructionCoder(sfIdl);
  it("open_order data = Anchor coder", () => {
    const ix = openOrderIx(SF, me, 99n, mint, spec);
    const want = coder.encode("open_order", {
      draft_id: new BN(99), recipient: P(mint),
      spec: { m: 64, n: 64, k: 2048, config: Array.from(spec.config), hash_a: Array.from(spec.hashA), hash_b: Array.from(spec.hashB), data_uri: spec.dataUri, deadline_blocks: new BN(500), max_blocks: 10, evaluators: [P(me), P(mint)], threshold: 1 },
    });
    expect(hex(ix.data)).toBe(hex(new Uint8Array(want)));
    checkAccounts(ix, sfIdl, "open_order");
    expect(b58(ix.keys[2].pubkey)).toBe(PublicKey.findProgramAddressSync([Buffer.from("draft"), Buffer.from(me), new BN(99).toArrayLike(Buffer, "le", 8)], P(SF))[0].toBase58());
  });
  const pumpAccts = { mint, curveBuyback: me, ammBuyback: me, poolBaseVault: me, poolQuoteVault: mint };
  for (const [name, build] of [["buy_job", buyJobIx], ["buy_job_amm", buyJobAmmIx]] as const) {
    it(`${name} data and accounts`, () => {
      const ix = build(SF, BRIDGE, me, 5n, 12n, 1_000_000_000n, 777n, pumpAccts);
      const want = coder.encode(name, { nonce: new BN(12), sol_in: new BN(1_000_000_000), min_tokens_out: new BN(777) });
      expect(hex(ix.data)).toBe(hex(new Uint8Array(want)));
      checkAccounts(ix, sfIdl, name);
      expect(b58(ix.keys[3].pubkey)).toBe(b58(orderPda(SF, 12n)));
      expect(b58(ix.keys[4].pubkey)).toBe(b58(traderPda(SF)));
      expect(b58(ix.keys[ix.keys.length - 3].pubkey)).toBe(b58(depositPda(BRIDGE, 12n)));
      expect(b58(ix.keys[ix.keys.length - 1].pubkey)).toBe(b58(buybackVaultPda(SF)));
    });
  }
  it("cancel_draft", () => {
    const ix = cancelDraftIx(SF, me, 5n);
    expect(hex(ix.data)).toBe(hex(new Uint8Array(coder.encode("cancel_draft", {}))));
    checkAccounts(ix, sfIdl, "cancel_draft");
    expect(b58(ix.keys[1].pubkey)).toBe(b58(draftPda(SF, me, 5n)));
  });
});

describe("message compile vs web3.js", () => {
  const toW3 = (ix: Instruction) => new TransactionInstruction({ programId: P(ix.programId), keys: ix.keys.map((k) => ({ pubkey: P(k.pubkey), isSigner: k.isSigner, isWritable: k.isWritable })), data: Buffer.from(ix.data) });
  const blockhash = "EETubP5AKHgjPAhzPAFcb8BAY1hMH639CWCFTqi3hq1k";
  const cases: Record<string, Instruction[]> = {
    "sol transfer": [systemTransfer(me, mint, 5000n)],
    "token transfer": [createAtaIdempotent(me, mint, mint, TOKEN_2022), transferChecked(ata(me, mint, TOKEN_2022), mint, ata(mint, mint, TOKEN_2022), me, 10n, 6, TOKEN_2022)],
    "storefront open": [openOrderIx(SF, me, 1n, me, spec)],
    "storefront buy": [computeUnitLimit(400_000), buyJobIx(SF, BRIDGE, me, 1n, 3n, 10n, 1n, { mint, curveBuyback: mint, ammBuyback: mint })],
  };
  for (const [name, ixs] of Object.entries(cases)) {
    it(name, () => {
      const ours = serializeMessage(compileLegacy(ixs, me, pk(blockhash)));
      const t = new Transaction({ feePayer: P(me), recentBlockhash: blockhash });
      ixs.forEach((ix) => t.add(toW3(ix)));
      expect(hex(ours)).toBe(hex(new Uint8Array(t.serializeMessage())));
      const signed = buildSigned(ixs, seed, me, blockhash);
      const kp = Keypair.fromSeed(seed);
      t.sign(kp);
      expect(hex(signed.tx)).toBe(hex(new Uint8Array(t.serialize())));
    });
  }
  it("parses v0 messages and inserts a signature", () => {
    const kp = Keypair.fromSeed(seed);
    const msg = new TransactionMessage({ payerKey: kp.publicKey, recentBlockhash: blockhash, instructions: [SystemProgram.transfer({ fromPubkey: kp.publicKey, toPubkey: P(mint), lamports: 1 })] }).compileToV0Message();
    const vt = new VersionedTransaction(msg);
    const bytes = new Uint8Array(vt.serialize());
    const p = parseTransaction(bytes);
    expect(p.parsed.version).toBe(0);
    expect(b58(p.parsed.staticKeys[0])).toBe(kp.publicKey.toBase58());
    const sig = signRaw(seed, p.message);
    const out = insertSignature(bytes, me, sig);
    const back = VersionedTransaction.deserialize(out);
    expect(verifyRaw(new Uint8Array(back.signatures[0]), new Uint8Array(back.message.serialize()), me)).toBe(true);
    expect(parseMessage(new Uint8Array(msg.serialize())).instructions.length).toBe(1);
  });
});
