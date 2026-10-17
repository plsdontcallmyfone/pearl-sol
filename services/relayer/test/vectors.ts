// Helpers for chain/agent-types/vectors JSON.

import { PublicKey } from "@solana/web3.js";
import type { PostJobBody } from "../src/codec.ts";

type V = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
const hex = (s: string) => Buffer.from(s, "hex");

export function postJobFromJson(j: V): PostJobBody {
  return {
    m: j.m, n: j.n, k: j.k, config: hex(j.config), hashA: hex(j.hash_a), hashB: hex(j.hash_b), dataUri: Buffer.from(j.data_uri),
    workerPay: BigInt(j.worker_pay), protocolFee: BigInt(j.protocol_fee), deadline: BigInt(j.deadline_height),
    maxBlocks: j.max_blocks, evaluators: j.evaluators.map((e: string) => new PublicKey(e)), threshold: j.threshold,
  };
}
