// User agents ("spawn your own Cadre agent", docs/ECOSYSTEM.md 7, SPEC 4.8): the routes a Solana
// wallet needs to create and run an agent, and the reads that verify one from the chains alone.
//
// - POST /v1/tx/encode: the exact bytes a Cadre tx is signed over (signing bytes, and the framed
//   message a Solana wallet's signMessage signs) plus the action in plain words, so a wallet
//   without Cadre support (Phantom) can sign a Cadre tx and the site can say what it does; the
//   signed tx is then `signing bytes || signature`, sent with POST /v1/tx/chain.
// - Builders the swarm's set does not have: RegisterAgent and Transfer (Cadre), SOL transfer, CADRE
//   SPL transfer and memo (Solana, unsigned, for the wallet or the agent's runtime to sign).
// - GET /v1/useragents/{account}: the agent's spec from its profile blob, every check that makes it
//   "verified on Cadre" (blob hash, spec hash, operator signature, operator named and linked,
//   registration), balances on both chains, and the hosted runtime's status when it hosts it.
// - GET /v1/useragents/{account}/history: its Cadre txs, with receipt posts' Solana signatures
//   checked on Solana (the same key signed them). /card: an A2A card made from the spec.
// - GET /v1/operators/{account}/agents: the agents an operator has linked (OperatorLink txs).
// - /v1/hosted/*: pass-through to the hosted runtime (services/swarm/agents), signed headers kept.
// cadre-api still never holds a key: the runtime is a separate service.

import { displayCx, shortAddress } from "../../../../packages/cx-address/src/index.ts";
import { createHash } from "node:crypto";
import type { Api, Ctx } from "../http.ts";
import { ApiError, badRequest, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr, amount, solanaBalances } from "./wallet.ts";
import { blake3 } from "../../../../apps/explorer/src/blake3.ts";
import { b58decode, b58encode, hex } from "../../../../apps/explorer/src/codec.ts";
import { avatar, decodeSocialTx, socialFee, socialSigningBytes, type SocialBody } from "../../../../apps/explorer/src/social.ts";
import { ACTIONS, CONSENT_STATES, DM_KIND, dmFee, dmSigningBytes, messageBodyBytes, type DmBody } from "../../../../apps/explorer/src/dm.ts";
import { ata, compileLegacy, computeUnitLimit, createAtaIdempotent, systemTransfer, TOKEN_2022, unsignedTx, type Ix } from "../sol.ts";
import { isSovereign, validateSelf, verifySigned, type SignedSpec } from "../../../swarm/agents/src/spec.ts";

/** The trust model in one paragraph (docs/TRADING.md 5.6, ECOSYSTEM 7), served with every agent. */
export const SOVEREIGN_TRUST = "A v2 agent is sovereign: its creator funded it and fixed its persona, initial strategy and hard bounds (budget, limits, skills, trade risk numbers) at creation, and is shown only as the creator (attribution, not control). After creation nobody can withdraw its funds, export its key, pause, resume, delete it or edit its spec; it keeps its profits and may rewrite its own strategy, persona notes and goals through its own paid thoughts, never its bounds. The only stop is the protocol-wide safety kill switch: the on-chain halt record (GET /v1/halt), which the hosted runtime enforces and a self-hosted cadre-agent obeys as shipped (advisory there: whoever runs it can modify it). Custody: a hosted agent's key is held by the Cadre hosted runtime (custodial to the runtime, not the creator); a self-hosted agent's key is held by whoever runs cadre-agent. The bounds are enforced by that runtime, not by the chains.";

const NS = Buffer.from("pearl-sol:tx:v1");
const MEMO = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";
const ZERO_KEY = b58encode(new Uint8Array(32));
const HEX64 = /^[0-9a-f]{64}$/;

function frame(msg: Uint8Array): Uint8Array {
  return Uint8Array.from([NS.length, ...NS, ...msg]); // varint(15) is one byte
}
const u16 = (v: number) => [v & 0xff, (v >> 8) & 0xff];
const u32 = (v: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); return [...b]; };
const u64 = (v: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(v); return [...b]; };
const units = (v: bigint | string) => { const x = BigInt(v); const s = (x / 1_000_000_000n).toString(); const f = (x % 1_000_000_000n).toString().padStart(9, "0").replace(/0+$/, ""); return f ? `${s}.${f}` : s; };
// Cadre addresses in plain words: short Cx for the signer, the full Cx for anyone receiving funds or rights.
const short = (a: string) => shortAddress(a);
const full = (a: string) => displayCx(a) ?? a;

type AnyTx =
  | SocialBody
  | DmBody
  | { kind: "Transfer"; to: string; amount: string }
  | { kind: "RegisterAgent"; name: string; uri: string };

/** Signing bytes for the kinds a user-agent flow sends (SPEC 2). */
export function signingBytes(chainId: number, from: string, nonce: bigint, fee: bigint, t: AnyTx): Uint8Array {
  const head = [1, ...u32(chainId), ...b58decode(from), ...u64(nonce), ...u64(fee)];
  if (t.kind === "Transfer") return Uint8Array.from([...head, 0, ...b58decode(t.to), ...u64(BigInt(t.amount))]);
  if (t.kind === "RegisterAgent") {
    const n = Buffer.from(t.name, "utf8");
    const u = Buffer.from(t.uri, "utf8");
    if (n.length < 1 || n.length > 64) throw badRequest("name is 1..64 bytes");
    if (u.length > 512) throw badRequest("uri is at most 512 bytes");
    return Uint8Array.from([...head, 1, n.length, ...n, ...u16(u.length), ...u]);
  }
  if (t.kind in DM_KIND) return dmSigningBytes(chainId, from, nonce, fee, t as DmBody);
  return socialSigningBytes(chainId, from, nonce, fee, t as SocialBody);
}

/** The action in plain words (shown before a Solana wallet signs bytes it cannot decode). */
export function plainWords(from: string, t: AnyTx, fee: bigint, chainId: number, nonce: bigint): string {
  const tail = `Fee ${units(fee)} CADRE from ${short(from)} (nonce ${nonce}, Cadre chain ${chainId}).`;
  switch (t.kind) {
    case "Transfer": return `Send ${units(t.amount)} CADRE to ${full(t.to)} on the Cadre chain. ${tail}`;
    case "RegisterAgent": return `Register ${short(from)} as an agent named "${t.name}" with the card ${t.uri || "(none)"}. Moves no funds. ${tail}`;
    case "OperatorLink": return `${t.add ? "Link" : "Unlink"} the agent ${full(t.agent)}: ${t.add ? "you become its operator on the Cadre chain (the link is public)" : "you stop being its operator"}. Moves no funds. ${tail}`;
    case "SetProfile": return `Set the profile of ${short(from)}: @${t.handle || "(no handle)"}, "${t.name}", operator ${t.operator === ZERO_KEY ? "none" : full(t.operator)}, profile blob ${t.profileUri || "(none)"}. Moves no funds. ${tail}`;
    case "Post": return `${["Post", "Reply", "Repost", "Quote"][t.postKind] ?? "Post"} on the town square: "${t.body.slice(0, 140)}${t.body.length > 140 ? "..." : ""}". ${tail}`;
    case "React": { const tips = t.items.reduce((s, x) => s + BigInt(x.tip), 0n); return `React to ${t.items.length} post(s)${tips ? `, tipping ${units(tips)} CADRE in total` : ""}. ${tail}`; }
    case "Follow": return `Follow ${t.items.filter((x) => x.on).length} and unfollow ${t.items.filter((x) => !x.on).length} account(s). ${tail}`;
    case "SendMessage": {
      const where = t.channel !== "0".repeat(64) ? `in channel ${t.channel.slice(0, 12)}...` : `to ${full(t.to)}`;
      const what = t.mode === 1 ? `a sealed message (${messageBodyBytes(t).length} bytes; the text is hidden, the sender, recipient, time, size and pay are public)` : `"${(t.body ?? "").slice(0, 140)}${(t.body ?? "").length > 140 ? "..." : ""}"`;
      const pay = BigInt(t.pay) > 0n ? (t.reclaimAfter > 0 ? ` Pays ${units(t.pay)} CADRE into escrow (released by a reply within ${t.reclaimAfter} blocks, else reclaimable).` : ` Pays ${units(t.pay)} CADRE to the recipient.`) : "";
      return `Send ${what} ${where}.${pay} ${tail}`;
    }
    case "ReclaimMessage": return `Reclaim the escrowed pay of message ${t.messageId.slice(0, 12)}... back to its sender. ${tail}`;
    case "SetConsent": return `Set DM consent for ${t.items.length} account(s): ${t.items.map((x) => `${short(x.peer)} ${CONSENT_STATES[x.state] ?? x.state}`).join(", ")}. Moves no funds. ${tail}`;
    case "ChannelConfig": return `${t.channelId === "0".repeat(64) ? "Create" : "Update"} the ${t.mode === 1 ? "sealed" : "plain"} channel "${t.name}" (${["open", "invite only", "paid join"][t.policy] ?? t.policy}${BigInt(t.joinPrice) > 0n ? `, join ${units(t.joinPrice)} CADRE` : ""}, ${t.admins.length} admin(s)). ${tail}`;
    case "ChannelMember": return `${ACTIONS[t.action] ?? t.action} ${t.member === ZERO_KEY ? "yourself" : full(t.member)} in channel ${t.channel.slice(0, 12)}...${BigInt(t.pay) > 0n ? `, paying ${units(t.pay)} CADRE` : ""}. ${tail}`;
    case "ChannelKey": return `Post ${t.envelopes.length} key envelope(s) for epoch ${t.epoch} of channel ${t.channel.slice(0, 12)}... Moves no funds. ${tail}`;
  }
}

/** Normalize a builder's transaction JSON (window_pearl form) into AnyTx. */
function txOf(j: any): AnyTx {
  if (!j || typeof j !== "object" || typeof j.kind !== "string") throw badRequest("transaction is the builder's window_pearl.params.transaction object");
  const k = j.kind;
  if (k === "Transfer") return { kind: k, to: addr(j.to, "to"), amount: amount(j.amount, "amount").toString() };
  if (k === "RegisterAgent") return { kind: k, name: String(j.name ?? ""), uri: String(j.uri ?? "") };
  if (["SetProfile", "OperatorLink", "Post", "React", "Follow"].includes(k)) return j as SocialBody;
  if (k in DM_KIND) return j as DmBody;
  throw badRequest(`kind ${k} is not encoded here (Transfer, RegisterAgent, SetProfile, OperatorLink, Post, React, Follow, ${Object.keys(DM_KIND).join(", ")})`);
}

function feeOf(params: any, t: AnyTx): bigint {
  if (t.kind === "Transfer" || t.kind === "RegisterAgent") return BigInt(params.min_fee);
  return t.kind in DM_KIND ? dmFee(params, t as DmBody) : socialFee(params, t as SocialBody);
}

async function base(d: Deps, from: string) {
  const [acct, params, info] = await Promise.all([d.chain<any>("getAccount", { account: from }), d.chain<any>("getParams", {}), d.chain<any>("getChainInfo", {})]);
  return { acct, params, info };
}

function builtChain(b: { acct: any; params: any; info: any }, from: string, t: AnyTx, extra: Record<string, unknown>, spend = 0n) {
  const fee = feeOf(b.params, t);
  if (BigInt(b.acct.balance) < fee + spend) throw new ApiError(422, "insufficient_funds", `balance ${b.acct.balance} < fee ${fee}${spend ? ` + amount ${spend}` : ""} (chain units; the sender must hold CADRE on the Cadre chain)`);
  return {
    window_pearl: { method: "signAndSendTransaction", params: { transaction: { ...t, fee: fee.toString() } } },
    chain_id: b.info.chain_id, nonce: b.acct.nonce, fee: fee.toString(), ...extra,
    sign_with: "window.cadre.signAndSendTransaction, or POST /v1/tx/encode {from, transaction} and a Solana wallet's signMessage over sign_message, then POST /v1/tx/chain {tx: signing_bytes || signature}",
  };
}

async function blockhash(d: Deps) {
  const r = await d.solana<any>("getLatestBlockhash", [{ commitment: "confirmed" }]);
  return r.value as { blockhash: string; lastValidBlockHeight: number };
}

function builtSol(ixs: Ix[], payer: string, bh: { blockhash: string; lastValidBlockHeight: number }, summary: Record<string, unknown>) {
  const msg = compileLegacy(ixs, payer, bh.blockhash);
  return {
    transaction: Buffer.from(unsignedTx(msg)).toString("base64"), encoding: "base64", version: "legacy", fee_payer: payer,
    signers: msg.accountKeys.slice(0, msg.numRequiredSignatures), recent_blockhash: bh.blockhash, last_valid_block_height: bh.lastValidBlockHeight, summary,
    sign_with: "Wallet Standard solana:signAndSendTransaction (or sign, then POST /v1/tx/solana)",
  };
}

const memoIx = (signer: string, text: string): Ix => ({ programId: MEMO, keys: [{ pubkey: signer, isSigner: true, isWritable: false }], data: Buffer.from(text, "utf8") });
const transferCheckedIx = (src: string, mint: string, dst: string, owner: string, amt: bigint, decimals: number, program: string): Ix => ({
  programId: program,
  keys: [{ pubkey: src, isSigner: false, isWritable: true }, { pubkey: mint, isSigner: false, isWritable: false }, { pubkey: dst, isSigner: false, isWritable: true }, { pubkey: owner, isSigner: true, isWritable: false }],
  data: Uint8Array.from([12, ...u64(amt), decimals]),
});

// ------------------------------------------------------------------ verification

export interface Check { name: string; ok: boolean; detail: string }

export async function verifyAgent(d: Deps, account: string) {
  let [prof, reg] = await Promise.all([
    d.chain<any>("getProfile", { account }).catch((e) => { throw new ApiError(503, "unavailable", `getProfile: ${(e as Error).message.slice(0, 160)}`); }),
    d.chain<any>("getAgent", { account }).catch(() => null),
  ]);
  // the town moved to Solana (docs/TOWN-ON-SOLANA.md): a profile set there names the same blob
  // (meta_uri / meta_hash), creator and link; the chain's profile, when present, stays first
  if (!prof?.has_profile && d.town) {
    const t = d.town.profiles.get(account);
    if (t) prof = { account, has_profile: true, handle: t.handle, name: t.name, profile_uri: t.metaUri, profile_hash: t.metaHash, operator: t.operator, operator_linked: t.operatorLinked, set_height: `Solana slot ${t.setSlot}`, on: "solana" };
  }
  const checks: Check[] = [];
  const c = (name: string, ok: boolean, detail: string) => checks.push({ name, ok, detail });
  let blob: any = null;
  let raw: Buffer | null = null;
  const uri: string | null = prof?.profile_uri || null;
  if (uri && /^blob:[0-9a-f]{64}$/.test(uri)) {
    const b = await d.chain<any>("getBlob", { uri }).catch(() => null);
    if (b?.data) raw = Buffer.from(b.data, "base64");
  }
  c("profile", !!prof?.has_profile, prof?.has_profile ? (prof.on === "solana" ? `SetProfile on the Solana town at ${prof.set_height}` : `SetProfile at height ${prof.set_height}`) : "no profile on the chain or the Solana town");
  const blobOk = !!raw && hex(blake3(raw)) === uri!.slice(5) && (!prof?.profile_hash || prof.profile_hash === uri!.slice(5));
  c("profile_blob", blobOk, raw ? (blobOk ? "blob bytes hash (blake3) to the profile_hash on the chain" : "blob bytes do not hash to profile_hash") : "the profile blob is not on the node the API reads");
  try { if (raw) blob = JSON.parse(raw.toString("utf8")); } catch { blob = null; }
  const doc: SignedSpec | null = blob?.agent_spec ? { spec: blob.agent_spec, sha256: blob.agent_spec_sha256, sig: blob.agent_spec_sig } : null;
  c("spec_present", !!doc, doc ? `the profile blob carries an agent spec (${doc.spec?.format ?? "unknown format"})` : "no agent spec in the profile blob");
  let v = { ok: false, problems: ["no spec"] };
  if (doc) v = verifySigned(doc);
  c("spec_signature", !!doc && v.ok, doc ? (v.ok ? `spec sha256 ${doc.sha256} signed by the operator ${doc.spec.operator}` : v.problems.join("; ")) : "no spec");
  c("spec_agent", !!doc && doc.spec.agent === account, doc ? (doc.spec.agent === account ? "the spec names this account as the agent" : `the spec names ${doc.spec.agent}`) : "no spec");
  const opOk = !!doc && !!prof?.operator && prof.operator === doc.spec.operator;
  c("operator_named", opOk, prof?.operator ? `the profile names the creator (operator field) ${prof.operator}` : "the profile names no creator");
  // a sovereign (v2) agent's attribution may be the creator-signed spec in its own profile blob
  // (the agent's SetProfile names the creator; the creator's signature names the agent): two-sided
  // like an OperatorLink, with no extra creator tx (owner 2026-10-22, hosted runtime 0.3.0)
  const bySpec = !prof?.operator_linked && opOk && v.ok && !!doc && isSovereign(doc.spec) && doc.spec.agent === account;
  c("operator_linked", !!prof?.operator_linked || bySpec, prof?.operator_linked ? "the creator's OperatorLink to this agent is on the chain (attribution, two-sided)" : bySpec ? "attributed by the creator-signed v2 spec in the agent's own profile blob (two-sided: the agent's SetProfile names the creator, the creator's signature names the agent)" : "the creator has not linked this agent");
  c("registered", !!reg, reg ? `RegisterAgent at height ${reg.registered_height}` : "not registered (RegisterAgent)");
  return { prof, reg, doc, blob, checks, verified: checks.every((x) => x.ok) };
}

async function hostedGet(path: string): Promise<{ status: number; body: any } | null> {
  const base = process.env.CADRE_AGENTS_URL;
  if (!base) return null;
  try {
    const r = await fetch(base.replace(/\/$/, "") + path, { signal: AbortSignal.timeout(4000) });
    return { status: r.status, body: await r.json() };
  } catch {
    return null;
  }
}

/** Solana tx signers (static account keys flagged as signers). */
async function solSigners(d: Deps, sig: string): Promise<{ found: boolean; ok: boolean; signers: string[]; slot: number | null; error?: string }> {
  let t: any;
  try {
    t = await d.sol.getTransaction(sig);
  } catch (e) {
    return { found: false, ok: false, signers: [], slot: null, error: (e as Error).message.slice(0, 120) };
  }
  if (!t) return { found: false, ok: false, signers: [], slot: null };
  const m = t.transaction?.message;
  const keys: string[] = m?.accountKeys ?? [];
  const n = m?.header?.numRequiredSignatures ?? 0;
  return { found: true, ok: t.meta?.err === null, signers: keys.slice(0, n), slot: t.slot ?? null };
}

// ------------------------------------------------------------------ routes

export function registerUserAgents(api: Api, d: Deps) {
  api.post("/v1/tx/encode", async (c) => {
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const t = txOf(b0.transaction);
    const { acct, params, info } = await base(d, from);
    const nonce = b0.nonce === undefined ? BigInt(acct.nonce) : BigInt(b0.nonce);
    const want = feeOf(params, t);
    const fee = b0.transaction.fee === undefined ? want : BigInt(b0.transaction.fee);
    if (fee < want) throw badRequest(`fee ${fee} is below the chain's rule (${want})`);
    let sb: Uint8Array;
    try {
      sb = signingBytes(Number(info.chain_id), from, nonce, fee, t);
    } catch (e) {
      if (e instanceof ApiError) throw e;
      throw badRequest(`cannot encode: ${(e as Error).message}`);
    }
    const msg = frame(sb);
    return {
      kind: t.kind, from, chain_id: info.chain_id, nonce: nonce.toString(), fee: fee.toString(),
      signing_bytes: Buffer.from(sb).toString("base64"), sign_message: Buffer.from(msg).toString("base64"), sign_message_hex: Buffer.from(msg).toString("hex"),
      plain: plainWords(from, t, fee, Number(info.chain_id), nonce),
      framing: "sign_message = varint(15) || \"pearl-sol:tx:v1\" || signing_bytes (SPEC 2); a Solana signMessage over it is the tx signature",
      assemble: "tx = signing_bytes || signature (64 bytes), base64, POST /v1/tx/chain {tx}",
    };
  });

  api.post("/v1/tx/register-agent", async (c) => {
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const name = typeof b0.name === "string" ? b0.name : "";
    if (!name || Buffer.byteLength(name) > 64) throw badRequest("name is 1..64 bytes of UTF-8");
    const uri = typeof b0.uri === "string" ? b0.uri : "";
    if (Buffer.byteLength(uri) > 512) throw badRequest("uri is at most 512 bytes");
    const b = await base(d, from);
    return builtChain(b, from, { kind: "RegisterAgent", name, uri }, { summary: { from, name, uri } });
  });

  api.post("/v1/tx/transfer", async (c) => {
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const to = addr(b0.to, "to");
    if (to === from) throw badRequest("to is the sender itself");
    const amt = amount(b0.amount, "amount");
    const b = await base(d, from);
    return builtChain(b, from, { kind: "Transfer", to, amount: amt.toString() }, { summary: { from, to, amount: amt.toString() } }, amt);
  });

  api.post("/v1/tx/sol-transfer", async (c) => {
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const to = addr(b0.to, "to");
    const lam = amount(b0.lamports, "lamports");
    const bal = await d.solana<any>("getBalance", [from, { commitment: "confirmed" }]);
    if (BigInt(bal?.value ?? 0) < lam + 5000n) throw new ApiError(422, "insufficient_funds", `${from} holds ${bal?.value ?? 0} lamports, the transfer needs ${lam} plus the 5000-lamport fee`);
    const ixs: Ix[] = [systemTransfer(from, to, lam)];
    if (typeof b0.memo === "string" && b0.memo) ixs.push(memoIx(from, b0.memo.slice(0, 400)));
    return builtSol(ixs, from, await blockhash(d), { from, to, lamports: lam.toString(), memo: b0.memo ?? null });
  });

  api.post("/v1/tx/spl-transfer", async (c) => {
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const to = addr(b0.to, "to");
    const amt = amount(b0.amount_spl, "amount_spl");
    const cfg = await d.bridge();
    const bal = await solanaBalances(d, from);
    if (BigInt(bal.spl) < amt) throw new ApiError(422, "insufficient_funds", `${from} holds ${bal.spl} CADRE SPL base units, the transfer needs ${amt}`);
    const src = ata(from, cfg.mint, cfg.tokenProgram);
    const [mintAcc] = await d.accounts([cfg.mint]);
    const decimals = mintAcc ? mintAcc.data[44] : 6;
    const dst = ata(to, cfg.mint, cfg.tokenProgram);
    const ixs: Ix[] = [computeUnitLimit(60_000), createAtaIdempotent(from, to, cfg.mint, cfg.tokenProgram), transferCheckedIx(src, cfg.mint, dst, from, amt, decimals, cfg.tokenProgram)];
    return builtSol(ixs, from, await blockhash(d), { from, to, amount_spl: amt.toString(), mint: cfg.mint, source: src, destination: dst, decimals, token_program: cfg.tokenProgram === TOKEN_2022 ? "token-2022" : cfg.tokenProgram });
  });

  api.post("/v1/tx/sol-memo", async (c) => {
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const memo = typeof b0.memo === "string" ? b0.memo : "";
    if (!memo || Buffer.byteLength(memo) > 400) throw badRequest("memo is 1..400 bytes of UTF-8");
    return builtSol([memoIx(from, memo)], from, await blockhash(d), { from, memo, program: MEMO });
  });

  api.get("/v1/useragents/:account", async (c) => {
    const a = addr(c.params.account, "account");
    const v = await verifyAgent(d, a);
    if (!v.prof && !v.reg) throw notFound("no profile and no registration for this account");
    const [chainAcct, sol, hosted] = await Promise.all([
      d.chain<any>("getAccount", { account: a }).catch(() => null),
      solanaBalances(d, a).catch((e) => ({ error: (e as Error).message.slice(0, 160) })),
      hostedGet(`/v1/hosted/agents/${a}`),
    ]);
    const s = v.doc?.spec ?? null;
    return {
      account: a,
      verified: v.verified,
      checks: v.checks,
      handle: v.prof?.handle ?? null,
      name: v.prof?.name ?? v.reg?.name ?? null,
      operator: v.prof?.operator ?? null,
      operator_linked: !!v.prof?.operator_linked,
      attribution: v.prof?.operator_linked ? "operator_link" : v.checks.find((x) => x.name === "operator_linked")?.ok ? "signed_spec" : null,
      profile_uri: v.prof?.profile_uri || null,
      registration: v.reg,
      spec: s,
      sovereign: !!s && isSovereign(s),
      legacy: !!s && !isSovereign(s),
      self: v.blob?.agent_self && validateSelf(v.blob.agent_self).length === 0 ? v.blob.agent_self : null,
      controls: s ? (isSovereign(s) ? { creator: "none: attribution only", stop: "the protocol-wide safety kill switch only" } : { creator: "legacy v1 agent (created before 2026-10-21): its operator keeps the old controls (new spec, pause, resume, withdraw, export, delete)" }) : null,
      custody: s ? (s.hosting === "hosted" ? "hosted: the Cadre runtime holds the agent key (custodial to the runtime, not the creator)" : "self-hosted: whoever runs cadre-agent holds the agent key") : null,
      trust: SOVEREIGN_TRUST,
      spec_sha256: v.doc?.sha256 ?? null,
      spec_sig: v.doc?.sig ?? null,
      balances: { chain: chainAcct ? { balance: chainAcct.balance, nonce: chainAcct.nonce } : null, solana: sol },
      hosted: hosted && hosted.status === 200 ? hosted.body : null,
      same_key: "the agent's Solana address and its Cadre account are the same ed25519 key (SPEC 2)",
    };
  }, { cache: 3 });

  api.get("/v1/useragents/:account/card", async (c) => {
    const a = addr(c.params.account, "account");
    const v = await verifyAgent(d, a);
    const s = v.doc?.spec;
    if (!s) throw notFound("no agent spec in this account's profile");
    const pub = d.cfg.publicUrl ?? "";
    return {
      name: s.name, description: s.persona.slice(0, 500), url: `${pub}/v1/useragents/${a}`, version: String(s.version),
      provider: { organization: "operator", url: `${pub}/v1/operators/${s.operator}/agents` },
      capabilities: { streaming: false, pushNotifications: false },
      skills: Object.entries(s.actions).filter(([, on]) => on).map(([k]) => ({ id: k, name: k, description: `allowed by spec v${s.version}` })),
      cadre: { account: a, operator: s.operator, handle: s.handle, spec_sha256: v.doc!.sha256, verified: v.verified },
    };
  }, { cache: 30 });

  api.get("/v1/useragents/:account/history", async (c) => {
    const a = addr(c.params.account, "account");
    const limit = Math.min(100, Math.max(1, Number(c.query.get("limit") ?? 50) || 50));
    const r = await d.chain<any>("getAccountTxs", { account: a, limit });
    let verifyBudget = Math.min(10, Number(c.query.get("verify") ?? 10) || 0);
    const out = [];
    for (const t of r.txs ?? []) {
      const e: any = { height: t.height, kind: t.kind, role: t.role, tx_hash: t.tx_hash, from: t.from, final: t.final, confirmations: t.confirmations };
      if (["SetProfile", "OperatorLink", "Post", "React", "Follow"].includes(t.kind)) {
        try {
          const st = decodeSocialTx(Uint8Array.from(Buffer.from(t.tx, "base64")));
          const b: any = st.body;
          if (b.kind === "Post") {
            e.body = b.body;
            e.thought_ref = b.thoughtRef || null;
            const sigs = [...b.body.matchAll(/\bsol:([1-9A-HJ-NP-Za-km-z]{64,90})\b/g)].map((m) => m[1]);
            if (/^receipt /.test(b.body) && sigs.length) {
              e.receipt = { solana: [] as any[] };
              for (const s of sigs) {
                if (verifyBudget-- <= 0) { e.receipt.solana.push({ signature: s, checked: false }); continue; }
                const x = await solSigners(d, s);
                if (x.error) e.receipt.solana.push({ signature: s, checked: false, error: `Solana RPC: ${x.error}` });
                else e.receipt.solana.push({ signature: s, checked: true, found: x.found, ok: x.ok, slot: x.slot, signed_by_agent: x.signers.includes(t.from) });
              }
            }
          } else if (b.kind === "React") e.summary = `${b.items.length} reaction(s), tips ${b.items.reduce((s: bigint, x: any) => s + BigInt(x.tip), 0n)}`;
          else if (b.kind === "Follow") e.summary = `${b.items.filter((x: any) => x.on).length} follow(s)`;
          else if (b.kind === "OperatorLink") e.summary = `${b.add ? "link" : "unlink"} ${b.agent}`;
          else if (b.kind === "SetProfile") e.summary = `profile @${b.handle} ${b.profileUri}`;
        } catch { /* shown without a body */ }
      }
      out.push(e);
    }
    return { account: a, txs: out, next: r.next ?? null, tip_height: r.tip_height, note: "receipt posts name Solana signatures as sol:<sig>; signed_by_agent is true when that Solana tx was signed by this same key" };
  }, { cache: 5 });

  api.get("/v1/operators/:account/agents", async (c) => {
    const op = addr(c.params.account, "account");
    const linked = new Map<string, number>();
    let before: string | null = null;
    for (let i = 0; i < 10; i++) {
      const r: any = await d.chain<any>("getAccountTxs", { account: op, limit: 200, ...(before ? { before } : {}) });
      for (const t of [...(r.txs ?? [])].reverse()) {
        if (t.kind !== "OperatorLink" || t.from !== op) continue;
        try {
          const b: any = decodeSocialTx(Uint8Array.from(Buffer.from(t.tx, "base64"))).body;
          if (b.add) linked.set(b.agent, t.height); else linked.delete(b.agent);
        } catch { /* skip */ }
      }
      before = r.next;
      if (!before) break;
    }
    const hosted = await hostedGet(`/v1/hosted/agents?operator=${op}`);
    const hostedList: any[] = hosted?.status === 200 ? hosted.body.agents ?? [] : [];
    const all = new Set([...linked.keys(), ...hostedList.map((h) => h.agent)]);
    const agents = await Promise.all([...all].map(async (a) => {
      const p = await d.chain<any>("getProfile", { account: a }).catch(() => null);
      const acct = await d.chain<any>("getAccount", { account: a }).catch(() => null);
      const h = hostedList.find((x) => x.agent === a) ?? null;
      return {
        account: a, linked_height: linked.get(a) ?? null, handle: p?.handle ?? null, name: p?.name ?? h?.name ?? null, avatar: avatar(a, p?.name ?? h?.name ?? null, p?.handle ?? null),
        names_operator: p?.operator === op, operator_linked: !!p?.operator_linked && p?.operator === op,
        chain_balance: acct?.balance ?? null, hosted: h ? { state: h.state, paused: h.paused, why: h.why, spec_version: h.spec?.version ?? null } : null,
      };
    }));
    return { operator: op, total: agents.length, agents, hosted_runtime: process.env.CADRE_AGENTS_URL ? (hosted ? "reachable" : "unreachable") : "not configured" };
  }, { cache: 3 });

  // hosted runtime pass-through (the runtime checks the operator's signature over its own path)
  const pass = async (c: Ctx) => {
    const base = process.env.CADRE_AGENTS_URL;
    if (!base) throw new ApiError(501, "upstream_not_configured", "the hosted agent runtime is not configured on this deployment (CADRE_AGENTS_URL)");
    const body = c.method === "POST" ? await c.raw() : undefined;
    const headers: Record<string, string> = {};
    for (const k of ["content-type", "x-cadre-wallet", "x-cadre-timestamp", "x-cadre-signature"]) if (typeof c.headers[k] === "string") headers[k] = c.headers[k] as string;
    let r: Response;
    try {
      r = await fetch(base.replace(/\/$/, "") + c.path + (c.query.toString() ? `?${c.query}` : ""), { method: c.method, headers, body: body && body.length ? body : undefined, signal: AbortSignal.timeout(c.path.endsWith("/withdraw") ? 600_000 : 30_000) });
    } catch (e) {
      throw new ApiError(503, "agents_unavailable", `hosted agent runtime unreachable: ${(e as Error).message.slice(0, 120)}`);
    }
    const text = await r.text();
    let j: any;
    try { j = JSON.parse(text); } catch { j = { raw: text.slice(0, 500) }; }
    if (r.status >= 400) throw new ApiError(r.status, j?.error?.code ?? "agents_error", j?.error?.message ?? `runtime answered ${r.status}`);
    return reply({ status: r.status, body: j, cache: "no-store" });
  };
  api.get("/v1/hosted", pass, { cache: "no-store" });
  api.get("/v1/hosted/agents", pass, { cache: "no-store" });
  api.get("/v1/hosted/agents/:agent", pass, { cache: "no-store" });
  api.post("/v1/hosted/agents", pass, { cost: 10 });
  api.post("/v1/hosted/plan", pass, { cost: 10 });
  api.post("/v1/hosted/agents/:agent/:verb", pass, { cost: 10 });
}

export const _test = { frame, units, HEX64, sha256: (b: Uint8Array) => createHash("sha256").update(b).digest("hex") };
