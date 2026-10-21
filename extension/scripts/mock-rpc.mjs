// Mock Solana + agent-chain RPC for headless UI checks when the real localnet is
// not running. Ports (owned block 10670-10679): Solana 10671, chain 10672, dApp page 10673.
// Data is synthetic and only for screenshots/tests; it is never shipped in the extension.
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { blake3 } from "@noble/hashes/blake3";
import { base58, base64 } from "@scure/base";

const SOL_PORT = Number(process.env.MOCK_SOL_PORT ?? 10671);
const CHAIN_PORT = Number(process.env.MOCK_CHAIN_PORT ?? 10672);
const PAGE_PORT = Number(process.env.MOCK_PAGE_PORT ?? 10673);
const ME = process.env.MOCK_ACCOUNT ?? "";

const BRIDGE = base58.decode("7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8");
const TOKEN_2022 = base58.decode("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const ATA = base58.decode("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const MINT = base58.decode("8eJrnVjzdEYGYMF8hx1bTfxi5wSANT41fV5c3bCKpump");
const sha = (...p) => createHash("sha256").update(Buffer.concat(p.map((x) => Buffer.from(x)))).digest();
const u64 = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const hex = (b) => Buffer.from(b).toString("hex");

// --- PDA helpers (same rule as the wallet; off-curve check skipped: we only need the
// addresses the wallet derives, so we accept the first bump the wallet would)
import { ed25519 } from "@noble/curves/ed25519";
const onCurve = (b) => { try { ed25519.ExtendedPoint.fromHex(b); return true; } catch { return false; } };
function pda(seeds, prog) {
  for (let bump = 255; bump >= 0; bump--) {
    const h = sha(...seeds, Buffer.from([bump]), prog, Buffer.from("ProgramDerivedAddress"));
    if (!onCurve(h)) return h;
  }
}
const bridgeConfig = pda([Buffer.from("config")], BRIDGE);
const vault = pda([bridgeConfig, TOKEN_2022, MINT], ATA);
const STOREFRONT = base58.decode("DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr");
const PUMP = base58.decode("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
const storeConfig = pda([Buffer.from("config")], STOREFRONT);
const curve = pda([Buffer.from("bonding-curve"), MINT], PUMP);
const pumpGlobal = pda([Buffer.from("global")], PUMP);
const ataOf = (owner) => pda([owner, TOKEN_2022, MINT], ATA);

const state = {
  depositNonce: 41,
  deposits: new Map(), // nonce -> { at, recipient, amount }
  sigs: new Map(), // sig -> { at, kind }
  sol: 4_215_000_000n,
  token: 182_450_000_000n, // 182,450 tokens (6 dp)
  height: 1180,
  native: 25_731_400_000_000n, // 25,731.4 (9 dp)
  nonce: 7,
  agent: null,
  txs: new Map(),
  withdrawals: [],
  processed: new Set(),
};
setInterval(() => { state.height += 1; }, 1500);

function accountData(addr) {
  const a = Buffer.from(base58.decode(addr));
  if (a.equals(bridgeConfig)) {
    const d = Buffer.alloc(8 + 32 * 4 + 8 + 2 + 4 + 32);
    sha(Buffer.from("account:Config")).copy(d, 0, 0, 8);
    Buffer.alloc(32, 1).copy(d, 8);
    Buffer.from(MINT).copy(d, 40);
    vault.copy(d, 72);
    Buffer.from(TOKEN_2022).copy(d, 104);
    d.writeBigUInt64LE(BigInt(state.depositNonce), 136);
    d[144] = 1;
    return { owner: base58.encode(BRIDGE), data: d };
  }
  if (a.equals(Buffer.from(MINT))) {
    const d = Buffer.alloc(82);
    d.writeBigUInt64LE(1_000_000_000_000_000n, 36);
    d[44] = 6;
    d[45] = 1;
    return { owner: base58.encode(TOKEN_2022), data: d };
  }
  if (ME && a.equals(ataOf(Buffer.from(base58.decode(ME))))) {
    const d = Buffer.alloc(165);
    Buffer.from(MINT).copy(d, 0);
    d.writeBigUInt64LE(state.token, 64);
    return { owner: base58.encode(TOKEN_2022), data: d };
  }
  if (a.equals(storeConfig)) {
    const d = Buffer.alloc(146 + 32);
    sha(Buffer.from("account:Config")).copy(d, 0, 0, 8);
    Buffer.from(MINT).copy(d, 40);
    Buffer.from(BRIDGE).copy(d, 72);
    d.writeUInt16LE(2000, 105);
    d.writeUInt16LE(300, 107);
    d.writeBigUInt64LE(10_000_000n, 109);
    d.writeBigUInt64LE(50_000_000_000n, 117);
    d.writeUInt32LE(4096, 125);
    d.writeUInt32LE(1000, 129);
    d.writeBigUInt64LE(10_000n, 133);
    d[141] = 1;
    d.writeUInt32LE(1, 142);
    return { owner: base58.encode(STOREFRONT), data: d };
  }
  if (a.equals(curve)) {
    const d = Buffer.alloc(150);
    d.writeBigUInt64LE(912_000_000_000_000n, 8);
    d.writeBigUInt64LE(38_500_000_000n, 16);
    d[48] = 0;
    return { owner: base58.encode(PUMP), data: d };
  }
  if (a.equals(pumpGlobal)) return { owner: base58.encode(PUMP), data: Buffer.alloc(1000, 3) };
  for (const n of state.processed) if (a.equals(pda([Buffer.from("processed"), Buffer.from([1]), u64(n)], BRIDGE))) return { owner: base58.encode(BRIDGE), data: Buffer.alloc(64) };
  return null;
}

function solana(method, params) {
  switch (method) {
    case "getHealth": return "ok";
    case "getBalance": return { context: { slot: 1 }, value: Number(params[0] === ME ? state.sol : 0n) };
    case "getAccountInfo": {
      const a = accountData(params[0]);
      return { context: { slot: 1 }, value: a ? { lamports: 2_039_280, owner: a.owner, data: [a.data.toString("base64"), "base64"], executable: false, rentEpoch: 0 } : null };
    }
    case "getMultipleAccounts":
      return { context: { slot: 1 }, value: params[0].map((x) => { const a = accountData(x); return a ? { lamports: 1, owner: a.owner, data: [a.data.toString("base64"), "base64"], executable: false } : null; }) };
    case "getLatestBlockhash": return { context: { slot: 1 }, value: { blockhash: base58.encode(sha(Buffer.from(String(Math.floor(Date.now() / 20000))))), lastValidBlockHeight: 999 } };
    case "sendTransaction": {
      const tx = Buffer.from(params[0], "base64");
      const sig = base58.encode(tx.subarray(1, 65));
      const isDeposit = tx.includes(Buffer.from(BRIDGE)) && tx.includes(sha(Buffer.from("global:deposit")).subarray(0, 8));
      if (isDeposit) {
        state.deposits.set(state.depositNonce, { at: Date.now() });
        state.depositNonce += 1;
      }
      state.sigs.set(sig, { at: Date.now(), kind: isDeposit ? "deposit" : "tx" });
      return sig;
    }
    case "getSignatureStatuses":
      return { context: { slot: 1 }, value: params[0].map((s) => (state.sigs.has(s) ? { slot: 1, confirmations: 1, err: null, confirmationStatus: Date.now() - state.sigs.get(s).at > 1200 ? "confirmed" : "processed" } : null)) };
    case "getSignaturesForAddress":
      return [...state.sigs.entries()].map(([s, v]) => ({ signature: s, slot: 1, err: null, blockTime: Math.floor(v.at / 1000), memo: null }));
  }
  throw Object.assign(new Error(`mock solana: ${method} not implemented`), { code: -32601 });
}

const MINER_BLOCKS = new Set([1100, 1121, 1135, 1160, 1166, 1172, 1177]);
function job(i) {
  const id = hex(blake3(Buffer.from(`job-${i}`)));
  const statuses = ["open", "open", "delivered", "completed", "expired", "open"];
  const st = statuses[i % statuses.length];
  const poster = i === 1 && ME ? ME : base58.encode(blake3(Buffer.from(`poster-${i % 3}`)));
  const config = "00080000800000000003000000000007" + "0".repeat(72);
  // every third job is a storefront order: Pearl header id = the poster (docs/RPC.md)
  const terms = blake3(Buffer.from(`t${i}`));
  const headerId = i % 3 === 2 ? Buffer.from(base58.decode(poster)) : Buffer.from(id, "hex");
  const header = Buffer.concat([headerId, terms, Buffer.alloc(12)]);
  return {
    job_id: id, poster, status: st, mineable: st === "open",
    m: 64 * (1 + (i % 3)), n: 64, k: 2048, config, hash_a: hex(blake3(Buffer.from(`a${i}`))), hash_b: hex(blake3(Buffer.from(`b${i}`))),
    data_uri: i % 2 === 0 ? `blob:${hex(blake3(Buffer.from(`data-${i}`)))}` : `ipfs://bafy-mock-${i}`, worker_pay: String(BigInt(1 + i * 3) * 1_000_000_000n), protocol_fee: String(BigInt(1 + i) * 250_000_000n),
    deadline_height: 1400 + i * 20, max_blocks: 10 + i, blocks_mined: Math.min(10 + i, i * 2), cap_remaining: String(BigInt(1 + i) * 150_000_000n), subsidy_paid: String(BigInt(i) * 7_000_000n),
    evaluators: [base58.encode(blake3(Buffer.from("attester")))], threshold: 1, terms_hash: hex(terms), job_key: hex(blake3(Buffer.concat([header, Buffer.from(config, "hex")]))), job_header: hex(header), header_id: hex(headerId),
    created_height: 0, share_target: 0, posted_height: 1000 + i * 9, deliverer: st === "delivered" || st === "completed" ? base58.encode(blake3(Buffer.from("worker"))) : null,
    result_hash: st === "delivered" || st === "completed" ? hex(blake3(Buffer.from(`r${i}`))) : null, result_uri: "", closed_height: st === "expired" ? 1150 : 0, index: i,
  };
}
const JOBS = Array.from({ length: 9 }, (_, i) => job(i));

function chain(method, p) {
  switch (method) {
    case "getChainInfo": return { chain_id: 1347616769, genesis_hash: "00".repeat(32), height: state.height, tip: hex(blake3(u64(state.height))), cumulative_work: "0x1", finalized_height: state.height - 30, finalized_hash: "00".repeat(32), target_bits: 545259519, next_target_bits: 545259519, mempool_size: 0, peers: 2, finality_k: 30, syncing: false };
    case "getParams": return { admin: "", min_fee: "1000", finality_k: 30, emission_ppm: 7, alpha_ppm: 800000, attesters: [], attester_threshold: 1, bridge_paused: false, max_job_blocks: 100000, min_protocol_fee: "0", target_block_time_ms: 60000,
      // contract fee rule: chain/state/src/config.rs CONTRACT_DEFAULTS
      gas_price: "10000", gas_price_unit: "1000000", contract_base_fee: "1000", contract_byte_fee: "10", storage_byte_deposit: "100000", tx_gas_limit: "50000000" };
    // the contracts.json vector contract (address rule: creator 6DDS..., nonce 7)
    case "getContract": return p.address === "Ad49vfymLfb1oY4veT8yudCHGL6DM6aUpTMZuPbM7wcZ" ? { address: p.address, code_hash: "0d66d411a21e80d93afa1487b002a1867432c04ba1d08dc790d259639f6a69c4", admin: null, upgradable: false, creator: "6DDSqnFUxWMu66T4pgQJpaUxEmfYdrJLJdcJmUtL6HW7", label: "counter", created_height: 1 } : null;
    case "getAccount": return p.account === ME ? { account: ME, balance: String(state.native), nonce: state.nonce } : { account: p.account, balance: "0", nonce: 0 };
    case "getAgent": return p.account === ME ? state.agent : null;
    case "listJobs": {
      let js = JOBS;
      if (p.status === "mineable") js = js.filter((j) => j.mineable);
      else if (p.status) js = js.filter((j) => j.status === p.status);
      return { total: js.length, jobs: js.slice(p.offset ?? 0, (p.offset ?? 0) + (p.limit ?? 50)) };
    }
    case "getJob": return JOBS.find((j) => j.job_id === p.job_id) ?? null;
    case "sendTx": {
      const tx = Buffer.from(p.tx, "base64");
      const hash = hex(blake3(tx));
      const from = base58.encode(tx.subarray(5, 37));
      const nonce = Number(tx.readBigUInt64LE(37));
      const kind = tx[53];
      if (from === ME) {
        if (nonce !== state.nonce) throw Object.assign(new Error(`bad nonce ${nonce}, expected ${state.nonce}`), { code: -32000 });
        state.nonce += 1;
      }
      const at = state.height + 1;
      state.txs.set(hash, { kind, from, nonce, at });
      if (kind === 1) state.agent = { account: from, name: tx.subarray(55, 55 + tx[54]).toString(), uri: "", registered_height: at, updated_height: at };
      if (kind === 7) {
        const amount = tx.readBigUInt64LE(54 + 32);
        state.native -= amount;
        state.withdrawals.push({ nonce: state.withdrawals.length, from, solana_recipient: base58.encode(tx.subarray(54, 86)), amount_spl: String(amount / 1000n), amount: String(amount), height: at, block_hash: hash });
      }
      return { tx_hash: hash };
    }
    case "getTx": {
      const t = state.txs.get(p.tx_hash);
      if (!t) return null;
      const inc = state.height >= t.at;
      return { tx_hash: p.tx_hash, status: inc ? "included" : "pending", height: inc ? t.at : undefined, confirmations: inc ? state.height - t.at : undefined, kind: "x", from: t.from, nonce: t.nonce, fee: "1000" };
    }
    case "getBridgeEvents": {
      const ws = state.withdrawals.map((w) => {
        const c = state.height - w.height;
        if (c >= 30) state.processed.add(w.nonce);
        return { ...w, confirmations: c, final: c >= 30 };
      });
      return { withdrawal_count: ws.length, deposit_count: state.deposits.size, withdrawals: ws.filter((w) => w.nonce >= (p.from_nonce ?? 0)).slice(0, p.limit ?? 100) };
    }
    case "getDeposit": {
      const d = state.deposits.get(p.solana_nonce);
      if (!d || Date.now() - d.at < 5000) return { processed: false };
      return { processed: true, height: state.height - 2, confirmations: 2, final: false };
    }
    case "getBlock": {
      const h = p.height;
      if (h > state.height) return null;
      const mine = MINER_BLOCKS.has(h) || (h > 1180 && h % 9 === 0);
      return {
        hash: hex(blake3(u64(h))), header: "", header_fields: { height: h, miner: mine && ME ? ME : base58.encode(blake3(Buffer.from("other-miner"))), job_id: h % 2 ? hex(blake3(Buffer.from("job-0"))) : "00".repeat(32), timestamp_ms: Date.now() },
        proof_len: 12600, txs: mine ? [{ tx_hash: "aa", kind: "Transfer", from: "x", nonce: 0, fee: "1000" }, { tx_hash: "bb", kind: "Transfer", from: "y", nonce: 0, fee: "2500" }] : [], canonical: true, confirmations: state.height - h, final: state.height - h >= 30,
        fees: mine ? "3500" : "0", subsidy: h % 2 ? "7000000" : "0", reward: String((mine ? 3500n : 0n) + (h % 2 ? 7000000n : 0n)), matured: "0",
      };
    }
    case "getEscrow": return { escrow: "", escrow_balance: "0", pool: "", pool_balance: "0", total_supply: "1000000000000000000", outside_escrow: "0", outside_escrow_spl: "0", height: state.height };
  }
  throw Object.assign(new Error(`mock chain: ${method} not implemented`), { code: -32601 });
}

function serve(port, handler, label) {
  createServer((req, res) => {
    res.setHeader("access-control-allow-origin", "*");
    res.setHeader("access-control-allow-headers", "content-type");
    if (req.method === "OPTIONS") return res.end();
    if (req.method === "GET") return res.end("ok");
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      const j = JSON.parse(body);
      let out;
      try {
        out = { jsonrpc: "2.0", id: j.id, result: handler(j.method, j.params) };
      } catch (e) {
        out = { jsonrpc: "2.0", id: j.id, error: { code: e.code ?? -32000, message: e.message } };
      }
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(out));
    });
  }).listen(port, "127.0.0.1", () => console.log(`${label} mock on ${port}`));
}

serve(SOL_PORT, solana, "solana");
serve(CHAIN_PORT, chain, "chain");

// A tiny dApp page that exercises both providers.
createServer((req, res) => {
  res.setHeader("content-type", "text/html");
  res.end(`<!doctype html><html><head><title>Test dApp</title></head><body style="font-family:system-ui;padding:20px">
<h3>Test dApp</h3><pre id="out"></pre>
<script>
window.__wallets = [];
window.addEventListener("wallet-standard:register-wallet", (e) => e.detail({ register: (w) => window.__wallets.push(w) }));
window.dispatchEvent(new CustomEvent("wallet-standard:app-ready", { detail: { register: (w) => window.__wallets.push(w) } }));
</script></body></html>`);
}).listen(PAGE_PORT, "127.0.0.1", () => console.log(`dapp page on ${PAGE_PORT}`));
