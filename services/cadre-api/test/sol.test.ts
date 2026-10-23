// The unsigned-tx layer against @solana/web3.js (a devDependency of this package, test only):
// message bytes, PDA / ATA derivation, wire parsing.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { b58encode, sha256, utf8 } from "../../../apps/explorer/src/codec.ts";
import { ata, bridgeDepositIx, compileLegacy, computeUnitLimit, decodeBridgeConfig, parseWireTx, systemTransfer, TOKEN_2022, unsignedTx, depositPda, bridgeConfigPda, BRIDGE_ID } from "../src/sol.ts";
import { bridgeConfigData, key, MINT, VAULT } from "./fake.ts";

import web3 from "@solana/web3.js";
const { PublicKey, Transaction, TransactionInstruction, SystemProgram, ComputeBudgetProgram, VersionedTransaction } = web3;

const toWeb3 = (ix: ReturnType<typeof systemTransfer>) =>
  new TransactionInstruction({ programId: new PublicKey(ix.programId), keys: ix.keys.map((k) => ({ pubkey: new PublicKey(k.pubkey), isSigner: k.isSigner, isWritable: k.isWritable })), data: Buffer.from(ix.data) });

test("compileLegacy matches web3.js byte for byte (x402 payment with a separate fee payer, bridge deposit)", () => {
  const payer = key("payer"), service = key("service");
  const bh = b58encode(sha256(utf8("blockhash")));
  const ixs = [computeUnitLimit(10_000), systemTransfer(payer, service, 24_000_000n)];
  const mine = compileLegacy(ixs, service, bh);
  const t = new Transaction({ feePayer: new PublicKey(service), recentBlockhash: bh });
  t.add(ComputeBudgetProgram.setComputeUnitLimit({ units: 10_000 }), SystemProgram.transfer({ fromPubkey: new PublicKey(payer), toPubkey: new PublicKey(service), lamports: 24_000_000n }));
  assert.equal(Buffer.from(mine.message).toString("hex"), t.serializeMessage().toString("hex"));
  for (const ix of ixs) assert.deepEqual(toWeb3(ix).data, Buffer.from(ix.data));

  const cfg = decodeBridgeConfig(bridgeConfigData(42n))!;
  assert.equal(cfg.mint, MINT);
  assert.equal(cfg.vault, VAULT);
  assert.equal(cfg.depositNonce, 42n);
  const owner = key("owner2");
  const dep = [computeUnitLimit(100_000), bridgeDepositIx(cfg, owner, 2000n, key("recipient"))];
  const m2 = compileLegacy(dep, owner, bh);
  const t2 = new Transaction({ feePayer: new PublicKey(owner), recentBlockhash: bh });
  for (const ix of dep) t2.add(toWeb3(ix));
  assert.equal(Buffer.from(m2.message).toString("hex"), t2.serializeMessage().toString("hex"));

  // the wire form deserializes in web3.js with empty signature slots
  const wire = unsignedTx(mine);
  const vt = VersionedTransaction.deserialize(wire);
  assert.equal(vt.signatures.length, 2);
  assert.equal(vt.message.staticAccountKeys[0].toBase58(), service);
  const p = parseWireTx(wire);
  assert.deepEqual(p.signers, [service, payer]);
  assert.equal(p.version, "legacy");
});

test("PDAs and ATAs match web3.js", () => {
  const owner = key("o");
  const [want] = PublicKey.findProgramAddressSync([new PublicKey(owner).toBuffer(), new PublicKey(TOKEN_2022).toBuffer(), new PublicKey(MINT).toBuffer()], new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"));
  assert.equal(ata(owner, MINT, TOKEN_2022), want.toBase58());
  const nonce = Buffer.alloc(8);
  nonce.writeBigUInt64LE(9n);
  assert.equal(depositPda(9n), PublicKey.findProgramAddressSync([Buffer.from("deposit"), nonce], new PublicKey(BRIDGE_ID))[0].toBase58());
  assert.equal(bridgeConfigPda(), PublicKey.findProgramAddressSync([Buffer.from("config")], new PublicKey(BRIDGE_ID))[0].toBase58());
});

test("parseWireTx refuses junk and a signature count that does not match the message", () => {
  assert.throws(() => parseWireTx(Uint8Array.from([0])));
  assert.throws(() => parseWireTx(Uint8Array.from([1, 2, 3])));
  const m = compileLegacy([systemTransfer(key("a"), key("b"), 1n)], key("a"), b58encode(sha256(utf8("x"))));
  const w = unsignedTx(m);
  const bad = Uint8Array.from([2, ...new Uint8Array(128), ...m.message]);
  assert.throws(() => parseWireTx(bad), /needs 1 signatures/);
  assert.equal(parseWireTx(w).signers.length, 1);
});

// Imports services/storefront-x402's own source, so that package must be installed (npm ci there).
const SF_WEB3 = new URL("../../storefront-x402/node_modules/@solana/web3.js/package.json", import.meta.url);
if (!existsSync(SF_WEB3) && process.env.CI) throw new Error("CI: services/storefront-x402 node_modules missing; the x402 parity test would be skipped");
const sfSkip = existsSync(SF_WEB3) ? false : "services/storefront-x402 node_modules missing (npm ci there)";

test("the x402 payment tx, once the payer signs it, passes storefront-x402's own verifyExact", { skip: sfSkip }, async () => {
  const { verifyExact } = await import("../../storefront-x402/src/x402.ts");
  const { Keypair } = web3;
  const payer = Keypair.generate(), service = Keypair.generate();
  const req = { scheme: "exact" as const, network: "solana-localnet", maxAmountRequired: "24000000", resource: "x", description: "", mimeType: "application/json", payTo: service.publicKey.toBase58(), maxTimeoutSeconds: 120, asset: "So11111111111111111111111111111111111111112", extra: { feePayer: service.publicKey.toBase58(), assetKind: "native" as const, decimals: 9 } };
  const m = compileLegacy([computeUnitLimit(10_000), systemTransfer(payer.publicKey.toBase58(), req.payTo, 24_000_000n)], req.payTo, b58encode(sha256(utf8("bh2"))));
  const vt = VersionedTransaction.deserialize(unsignedTx(m));
  vt.sign([payer]);
  const v = verifyExact({ x402Version: 1, scheme: "exact", network: req.network, payload: { transaction: Buffer.from(vt.serialize()).toString("base64") } }, req);
  assert.equal(v.payer.toBase58(), payer.publicKey.toBase58());
  assert.equal(v.amount, 24_000_000n);
  assert.equal(v.needsFeePayerSig, true);
});
