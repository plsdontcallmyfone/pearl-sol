// Message signing framing (Commonware) and the dApp guards.
import { Keypair, PublicKey, SystemProgram, Transaction } from "@solana/web3.js";
import { describe, expect, it } from "vitest";
import { concat, fromHex, hex, utf8 } from "../src/core/bytes";
import { TX_NAMESPACE, frame, pubkeyOf, signFramed, verifyRaw } from "../src/core/crypto";
import { signPearlMessage, signSolanaMessage, verifyPearlMessage } from "../src/core/sign";
import { summarizeChainTx, summarizeMessage, summarizeSolanaTx } from "../src/shared/summary";
import { decodeTx } from "../src/chain/tx";
import { parseMessage } from "../src/solana/tx";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const seed = fromHex("b5802e2c4b58c29279a877868859e11d9c2ae86ee8bdb15ab912176fa6e74c9e");
const pub = pubkeyOf(seed);
const txv = JSON.parse(readFileSync(resolve(__dirname, "../../chain/agent-types/vectors/tx.json"), "utf8"));

describe("Commonware framing", () => {
  it("varint(len) || ns || msg, as in the node's framed_message", () => {
    const f = frame(TX_NAMESPACE, fromHex(txv[0].signing_bytes));
    expect(hex(f)).toBe(txv[0].framed_message);
    expect(f[0]).toBe(15);
  });
  it("multi-byte varint for a long namespace", () => {
    const ns = new Uint8Array(200).fill(97);
    const f = frame(ns, Uint8Array.of(1));
    expect(Array.from(f.subarray(0, 2))).toEqual([0xc8, 0x01]);
    expect(f.length).toBe(2 + 200 + 1);
  });
  it("pearl message signature uses its own namespace", () => {
    const msg = utf8("hello agent chain");
    const sig = signPearlMessage(seed, msg);
    expect(verifyPearlMessage(sig, msg, pub)).toBe(true);
    expect(verifyRaw(sig, concat(Uint8Array.of(20), utf8("pearl-sol:message:v1"), msg), pub)).toBe(true);
    // not valid as a tx signature over the same bytes, nor as a raw signature
    expect(verifyRaw(sig, frame(TX_NAMESPACE, msg), pub)).toBe(false);
    expect(verifyRaw(sig, msg, pub)).toBe(false);
    expect(hex(sig)).not.toBe(hex(signFramed(seed, TX_NAMESPACE, msg)));
  });
});

describe("Solana signMessage guards", () => {
  it("signs plain text raw", () => {
    const m = utf8("Sign in to example.com");
    expect(verifyRaw(signSolanaMessage(seed, m), m, pub)).toBe(true);
  });
  it("refuses a Solana transaction message", () => {
    const kp = Keypair.fromSeed(seed);
    const t = new Transaction({ feePayer: kp.publicKey, recentBlockhash: "EETubP5AKHgjPAhzPAFcb8BAY1hMH639CWCFTqi3hq1k" }).add(SystemProgram.transfer({ fromPubkey: kp.publicKey, toPubkey: new PublicKey(new Uint8Array(32).fill(7)), lamports: 1 }));
    expect(() => signSolanaMessage(seed, new Uint8Array(t.serializeMessage()))).toThrow(/Solana transaction/);
  });
  it("refuses framed agent-chain tx bytes", () => {
    expect(() => signSolanaMessage(seed, fromHex(txv[0].framed_message))).toThrow(/agent-chain/);
  });
});

describe("approval summaries", () => {
  it("decodes every chain tx kind", () => {
    for (const v of txv) {
      const s = summarizeChainTx(decodeTx(fromHex(v.tx)), v.from, v.chain_id);
      expect(s.headline.length).toBeGreaterThan(3);
      expect(s.warnings.filter((w) => /not the connected/.test(w))).toHaveLength(0);
    }
    const s = summarizeChainTx(decodeTx(fromHex(txv[0].tx)), "someoneelse", 1);
    expect(s.warnings.join(" ")).toMatch(/not the connected account/);
    expect(s.warnings.join(" ")).toMatch(/does not match/);
    expect(summarizeChainTx(decodeTx(fromHex(txv[0].tx))).headline).toBe("Send 0.123456789 PEARL");
  });
  it("decodes a Solana SOL transfer", () => {
    const kp = Keypair.fromSeed(seed);
    const t = new Transaction({ feePayer: kp.publicKey, recentBlockhash: "EETubP5AKHgjPAhzPAFcb8BAY1hMH639CWCFTqi3hq1k" }).add(SystemProgram.transfer({ fromPubkey: kp.publicKey, toPubkey: new PublicKey(new Uint8Array(32).fill(7)), lamports: 1_500_000_000 }));
    const s = summarizeSolanaTx(parseMessage(new Uint8Array(t.serializeMessage())), { me: kp.publicKey.toBase58() });
    expect(s.headline).toBe("Send 1.5 SOL");
    expect(s.warnings).toHaveLength(0);
  });
  it("flags binary messages", () => {
    expect(summarizeMessage(Uint8Array.of(0, 1, 2)).warnings.length).toBe(1);
    expect(summarizeMessage(utf8("hi")).warnings.length).toBe(0);
  });
});
