// Message signing rules for dApps.
//
// Agent chain (window.pearl.signMessage): Commonware framing under its own namespace,
//   sig = ed25519(varint(20) || "pearl-sol:message:v1" || message)
// so a signed message can never be replayed as a transaction (those use
// "pearl-sol:tx:v1"). Verifiers use commonware `verify(Some(ns), msg, sig)` or
// rebuild the framed bytes.
//
// Solana (Wallet Standard solana:signMessage): raw ed25519 over the message, as
// every Solana wallet does, but refused when the bytes parse as a Solana tx
// message or carry agent-chain tx framing.

import { equal, utf8 } from "./bytes";
import { TX_NAMESPACE, frame, signFramed, signRaw, verifyRaw } from "./crypto";
import { parseMessage } from "../solana/tx";
import { MESSAGE_NAMESPACE } from "../shared/constants";

export const MESSAGE_NS = utf8(MESSAGE_NAMESPACE);

export const signPearlMessage = (seed: Uint8Array, msg: Uint8Array) => signFramed(seed, MESSAGE_NS, msg);
export const verifyPearlMessage = (sig: Uint8Array, msg: Uint8Array, pub: Uint8Array) => verifyRaw(sig, frame(MESSAGE_NS, msg), pub);

export function looksLikeSolanaTx(msg: Uint8Array): boolean {
  try {
    const p = parseMessage(msg);
    return p.header.numRequiredSignatures > 0 && p.staticKeys.length > 0;
  } catch {
    return false;
  }
}

export function looksLikeChainTx(msg: Uint8Array): boolean {
  const prefix = frame(TX_NAMESPACE, new Uint8Array(0));
  if (msg.length >= prefix.length && equal(msg.subarray(0, prefix.length), prefix)) return true;
  // unframed signing bytes start with version 1 and are at least the fixed head (54 bytes)
  return false;
}

export function signSolanaMessage(seed: Uint8Array, msg: Uint8Array): Uint8Array {
  if (looksLikeSolanaTx(msg)) throw new Error("refused: this message is a Solana transaction; use signTransaction");
  if (looksLikeChainTx(msg)) throw new Error("refused: this message is an agent-chain transaction");
  return signRaw(seed, msg);
}
