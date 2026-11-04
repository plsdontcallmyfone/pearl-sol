// Two-side bridge codecs (docs/BRIDGE-V2.md) byte-checked against chain/agent-types/vectors/bridge2.json.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";
import { Keypair, PublicKey } from "@solana/web3.js";
import {
  KIND, bridgeLockV2Body, bridgeUnlockV2Body, decodeTx, setRouteV2Body, signTx, txHash,
} from "../src/agent-tx.ts";
import {
  DOMAIN_RH, RH_TESTNET_CHAIN_ID, WITHDRAWAL_V2_LEN, decodeWithdrawalV2, depositMessageV2, depositOrderMessageV2, ed25519Sign,
  encodePostJobBody, encodeWithdrawalV2, evmRecipient, orderJobIdV2, sha256,
} from "../src/codec.ts";
import { REPO_ROOT } from "../src/config.ts";
import { postJobFromJson } from "./vectors.ts";

type V = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
const v: V = JSON.parse(readFileSync(join(REPO_ROOT, "chain/agent-types/vectors/bridge2.json"), "utf8"));
const hex = (s: string) => Buffer.from(s, "hex");
const tx = (name: string) => v.txs.find((t: V) => t.name === name)!;
const msg = (name: string) => v.messages.find((t: V) => t.name === name)!;
const sigsFromJson = (s: V[]) => s.map((x) => ({ pubkey: new PublicKey(x.pubkey), sig: hex(x.sig) }));

function check(t: V, body: Buffer): void {
  const kp = Keypair.fromSeed(hex(t.seed));
  const out = signTx(kp, { chainId: t.chain_id, from: kp.publicKey, nonce: BigInt(t.nonce), fee: BigInt(t.fee), kind: t.kind }, body);
  assert.equal(out.toString("hex"), t.tx, `${t.name} tx`);
  assert.equal(txHash(out), t.tx_hash, `${t.name} tx_hash`);
  assert.equal(decodeTx(out).kind, t.kind);
}

describe("bridge v2 vs chain/agent-types/vectors/bridge2.json", () => {
  const side = { domain: DOMAIN_RH, extChainId: RH_TESTNET_CHAIN_ID, bridgeAddr: hex(msg("deposit_v2").bridge_addr) };

  it("deposit messages and signatures", () => {
    const m = msg("deposit_v2");
    const out = depositMessageV2(side, BigInt(m.ext_nonce), new PublicKey(m.recipient).toBuffer(), BigInt(m.amount_ext));
    assert.equal(out.toString("hex"), m.message);
    assert.equal(ed25519Sign(Keypair.fromSeed(hex(m.attester_seed)), out).toString("hex"), m.sig);
    const o = msg("deposit_order_v2");
    const body = hex(o.post_job_body);
    assert.equal(sha256(body).toString("hex"), o.post_job_body_sha256);
    const om = depositOrderMessageV2(side, BigInt(o.ext_nonce), new PublicKey(o.recipient).toBuffer(), BigInt(o.amount_ext), sha256(body));
    assert.equal(om.toString("hex"), o.message);
    assert.equal(orderJobIdV2(DOMAIN_RH, BigInt(o.ext_nonce)).toString("hex"), o.job_id);
  });

  it("tx bodies", () => {
    let t = tx("bridge_unlock_v2");
    check(t, bridgeUnlockV2Body({ domain: t.body.domain, extNonce: BigInt(t.body.ext_nonce), recipient: new PublicKey(t.body.recipient),
      amountExt: BigInt(t.body.amount_ext), jobOrder: null, sigs: sigsFromJson(t.body.sigs) }));
    t = tx("bridge_unlock_v2_order");
    const job = encodePostJobBody(postJobFromJson(t.body.job_order));
    check(t, bridgeUnlockV2Body({ domain: t.body.domain, extNonce: BigInt(t.body.ext_nonce), recipient: new PublicKey(t.body.recipient),
      amountExt: BigInt(t.body.amount_ext), jobOrder: job, sigs: sigsFromJson(t.body.sigs) }));
    const d = decodeTx(hex(t.tx));
    assert.equal(d.body.kind, KIND.BridgeUnlockV2);
    if (d.body.kind === 40) assert.equal(d.body.extNonce, 6n);
    t = tx("bridge_lock_v2");
    check(t, bridgeLockV2Body(t.body.domain, hex(t.body.recipient), BigInt(t.body.amount)));
    assert.deepEqual(evmRecipient(v.withdrawal_v2.fields.recipient_evm), hex(t.body.recipient));
    t = tx("set_route_v2");
    check(t, setRouteV2Body({ domain: t.body.domain, recipient: hex(t.body.recipient), routeBps: t.body.route_bps,
      untilHeight: BigInt(t.body.until_height), cap: BigInt(t.body.cap), releaseKey: new PublicKey(t.body.release_key).toBuffer() }));
  });

  it("v2 withdrawal entry", () => {
    const e = hex(v.withdrawal_v2.entry);
    assert.equal(e.length, WITHDRAWAL_V2_LEN);
    assert.notEqual(e.length, 88, "the Solana bridge pays only 88-byte entries");
    const w = decodeWithdrawalV2(e);
    assert.equal(w.domain, DOMAIN_RH);
    assert.equal(w.amountExt.toString(), v.withdrawal_v2.fields.amount_ext);
    assert.equal(w.from.toBase58(), v.withdrawal_v2.fields.from);
    assert.deepEqual(encodeWithdrawalV2(w), e);
    assert.throws(() => decodeWithdrawalV2(e.subarray(0, 88)));
  });
});
