// Devnet operator tool for the town (SPEC 4.6): AdminConfig and Transfer txs signed with a key
// file passed explicitly (never ambient), sent to a node RPC. Prints tx hashes, never keys.
//
//   node services/swarm/src/admin.ts config <rpc> <admin-key.json> <key> <value-hex|"">
//   node services/swarm/src/admin.ts house  <rpc> <admin-key.json> <operator-base58>...
//   node services/swarm/src/admin.ts u64    <rpc> <admin-key.json> <key> <decimal>
//   node services/swarm/src/admin.ts u32    <rpc> <admin-key.json> <key> <decimal>
//   node services/swarm/src/admin.ts send   <rpc> <from-key.json> <to-base58> <base-units>
//   node services/swarm/src/admin.ts wait   <rpc> <tx-hash>
import { frame, NS_TX, signTx } from "./chaintx.ts";
import { key32 } from "./b58.ts";
import { loadWallet } from "./wallet.ts";

async function rpc(url: string, method: string, params: unknown): Promise<any> {
  const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
  const j = (await r.json()) as any;
  if (j.error) throw new Error(`${method}: ${j.error.message}`);
  return j.result;
}

function le(n: bigint, bytes: number): Uint8Array {
  const b = Buffer.alloc(bytes);
  if (bytes === 8) b.writeBigUInt64LE(n);
  else b.writeUInt32LE(Number(n));
  return new Uint8Array(b);
}

async function adminConfig(url: string, keyFile: string, key: number, value: Uint8Array) {
  const w = loadWallet(keyFile);
  const [acct, info, params] = await Promise.all([rpc(url, "getAccount", { account: w.address }), rpc(url, "getChainInfo", {}), rpc(url, "getParams", {})]);
  const head = Buffer.alloc(54);
  head.writeUInt8(1, 0);
  head.writeUInt32LE(info.chain_id, 1);
  Buffer.from(w.publicKey).copy(head, 5);
  head.writeBigUInt64LE(BigInt(acct.nonce), 37);
  head.writeBigUInt64LE(BigInt(params.min_fee), 45);
  head.writeUInt8(8, 53); // AdminConfig
  const body = Buffer.concat([Buffer.from([key]), le(BigInt(value.length), 8).subarray(0, 2), Buffer.from(value)]);
  const sb = Buffer.concat([head, body]);
  const sig = w.sign(frame(NS_TX, sb));
  const tx = Buffer.concat([sb, Buffer.from(sig)]);
  const h = await rpc(url, "sendTx", { tx: tx.toString("base64") });
  console.log(JSON.stringify({ sent: "AdminConfig", key, value_hex: Buffer.from(value).toString("hex"), tx_hash: h.tx_hash ?? h }));
}

const [cmd, url, keyFile, ...rest] = process.argv.slice(2);
if (cmd === "config") await adminConfig(url, keyFile, Number(rest[0]), Buffer.from(rest[1] ?? "", "hex"));
else if (cmd === "u64") await adminConfig(url, keyFile, Number(rest[0]), le(BigInt(rest[1]), 8));
else if (cmd === "u32") await adminConfig(url, keyFile, Number(rest[0]), le(BigInt(rest[1]), 4));
else if (cmd === "house") await adminConfig(url, keyFile, 21, Uint8Array.from([rest.length, ...rest.flatMap((a) => [...key32(a)])]));
else if (cmd === "send") {
  const w = loadWallet(keyFile);
  const [acct, info, params] = await Promise.all([rpc(url, "getAccount", { account: w.address }), rpc(url, "getChainInfo", {}), rpc(url, "getParams", {})]);
  const tx = signTx(w, info.chain_id, BigInt(acct.nonce), BigInt(params.min_fee), { kind: "Transfer", to: key32(rest[0]), amount: BigInt(rest[1]) });
  const h = await rpc(url, "sendTx", { tx: Buffer.from(tx).toString("base64") });
  console.log(JSON.stringify({ sent: "Transfer", to: rest[0], amount: rest[1], tx_hash: h.tx_hash ?? h }));
} else if (cmd === "wait") {
  for (let i = 0; i < 600; i++) {
    const t = await rpc(url, "getTx", { tx_hash: keyFile });
    if (t?.status === "included") { console.log(JSON.stringify({ included: t.height, block_hash: t.block_hash })); process.exit(0); }
    await new Promise((r) => setTimeout(r, 5000));
  }
  console.error("not included after 50 min");
  process.exit(1);
} else {
  console.error("usage: admin.ts config|u64|u32|house|send|wait ...");
  process.exit(2);
}
