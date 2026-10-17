// Byte-for-byte checks against chain/agent-types/vectors (the node's own encoding).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";
import { Keypair, PublicKey } from "@solana/web3.js";
import {
  ESCROW_ADDRESS, KIND, POOL_ADDRESS, TX_NS, bridgeLockBody, bridgeUnlockBody, decodeTx, framed, signTx, transferBody, txHash,
} from "../src/agent-tx.ts";
import { depositMessage, depositOrderMessage, ed25519Sign, encodePostJobBody, orderJobId, sha256 } from "../src/codec.ts";
import { REPO_ROOT } from "../src/config.ts";
import { postJobFromJson } from "./vectors.ts";

const VEC = join(REPO_ROOT, "chain/agent-types/vectors");
type V = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
const txs: V[] = JSON.parse(readFileSync(join(VEC, "tx.json"), "utf8"));
const msgs: V[] = JSON.parse(readFileSync(join(VEC, "messages.json"), "utf8"));
const tx = (name: string) => txs.find((t) => t.name === name)!;
const msg = (name: string) => msgs.find((t) => t.name === name)!;
const hex = (s: string) => Buffer.from(s, "hex");
const kpFromSeed = (s: string) => Keypair.fromSeed(hex(s));

const sigsFromJson = (s: V[]) => s.map((x) => ({ pubkey: new PublicKey(x.pubkey), sig: hex(x.sig) }));

function check(v: V, body: Buffer): void {
  const kp = kpFromSeed(v.seed);
  assert.equal(kp.publicKey.toBase58(), v.from);
  const h = { chainId: v.chain_id, from: kp.publicKey, nonce: BigInt(v.nonce), fee: BigInt(v.fee), kind: v.kind };
  const out = signTx(kp, h, body);
  assert.equal(out.subarray(0, out.length - 64).toString("hex"), v.signing_bytes, `${v.name} signing bytes`);
  assert.equal(framed(TX_NS, out.subarray(0, out.length - 64)).toString("hex"), v.framed_message, `${v.name} framing`);
  assert.equal(out.toString("hex"), v.tx, `${v.name} tx`);
  assert.equal(txHash(out), v.tx_hash, `${v.name} tx_hash`);
}

describe("agent-chain tx encoding vs chain/agent-types/vectors", () => {
  it("transfer", () => {
    const v = tx("transfer");
    check(v, transferBody(new PublicKey(v.body.to), BigInt(v.body.amount)));
  });

  it("bridge_lock", () => {
    const v = tx("bridge_lock");
    check(v, bridgeLockBody(new PublicKey(v.body.solana_recipient), BigInt(v.body.amount)));
  });

  it("bridge_unlock (plain, 2 attesters)", () => {
    const v = tx("bridge_unlock");
    const b = v.body;
    check(v, bridgeUnlockBody({ solanaNonce: BigInt(b.solana_nonce), recipient: new PublicKey(b.recipient),
      amountSpl: BigInt(b.amount_spl), jobOrder: null, sigs: sigsFromJson(b.sigs) }));
    const d = decodeTx(hex(v.tx));
    assert.equal(d.kind, KIND.BridgeUnlock);
    assert.ok(d.body.kind === 6 && d.body.jobOrder === null && d.body.sigs.length === 2);
  });

  it("bridge_unlock_order: PostJob body, sha256 and the full tx", () => {
    const v = tx("bridge_unlock_order");
    const b = v.body;
    const m = msg("deposit_order");
    const body = encodePostJobBody(postJobFromJson(b.job_order));
    assert.equal(body.toString("hex"), m.post_job_body);
    assert.equal(sha256(body).toString("hex"), m.post_job_body_sha256);
    check(v, bridgeUnlockBody({ solanaNonce: BigInt(b.solana_nonce), recipient: new PublicKey(b.recipient),
      amountSpl: BigInt(b.amount_spl), jobOrder: body, sigs: sigsFromJson(b.sigs) }));
    const d = decodeTx(hex(v.tx));
    assert.ok(d.body.kind === 6 && d.body.jobOrder !== null);
    assert.equal(encodePostJobBody(d.body.jobOrder!).toString("hex"), m.post_job_body);
  });

  it("deposit and deposit_order messages + raw ed25519 signatures", () => {
    const pl = tx("bridge_unlock").body;
    const dm = msg("deposit");
    const m1 = depositMessage(BigInt(pl.solana_nonce), new PublicKey(pl.recipient), BigInt(pl.amount_spl));
    assert.equal(m1.toString("hex"), dm.message);
    assert.equal(ed25519Sign(kpFromSeed(dm.seed), m1).toString("hex"), dm.sig);

    const ob = tx("bridge_unlock_order").body;
    const om = msg("deposit_order");
    const m2 = depositOrderMessage(BigInt(ob.solana_nonce), new PublicKey(ob.recipient), BigInt(ob.amount_spl), hex(om.post_job_body_sha256));
    assert.equal(m2.toString("hex"), om.message);
    assert.equal(ed25519Sign(kpFromSeed(om.seed), m2).toString("hex"), om.sig);
    assert.equal(orderJobId(BigInt(om.solana_nonce)).toString("hex"), om.job_id);
  });

  it("reserved addresses", () => {
    const r = msg("reserved_addresses");
    assert.equal(ESCROW_ADDRESS.toBase58(), r.escrow);
    assert.equal(POOL_ADDRESS.toBase58(), r.pool);
  });
});
