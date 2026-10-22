// LiteSVM tests (in process, no ports) for the town program (docs/TOWN-ON-SOLANA.md).
// Every instruction is built by apps/explorer/src/townsol.ts (the encoder the API, the swarm and
// the CLI use), so these tests also prove that encoder against the real program; events are
// decoded twice (townsol.ts and Anchor's own coder) and must agree.
// Flow: config, profiles and handles, operator link and the house flag, posts, reactions and
// tips, follows, consent, DMs (plain, sealed, priced with escrow: release by a reply in time,
// late reply, reclaim, decline), pause. Exploits: the operator posting, editing or unlinking
// others, a stranger linking, handle theft, double follow, a denied sender, short pay, a fake
// escrow, a reclaim before the deadline, foreign log lines that look like town events.
// Run: cd token && anchor build -p town && cd tests/town && npm ci && npm test
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import web3 from '@solana/web3.js';
import anchor from '@coral-xyz/anchor';
import { LiteSVM, Clock } from 'litesvm';
import * as T from '../../../apps/explorer/src/townsol.ts';

const { PublicKey, Keypair, Transaction, TransactionInstruction, ComputeBudgetProgram, LAMPORTS_PER_SOL, Connection } = web3;
const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '../../..');
const TARGET = process.env.TOKEN_TARGET || path.join(ROOT, 'token/target');
const idl = JSON.parse(fs.readFileSync(path.join(TARGET, 'idl/town.json')));
const PROG = idl.address;
assert.equal(PROG, T.TOWN_LOCALNET, 'the localnet build id matches townsol.ts');
const coder = new anchor.BorshCoder(idl);

const svm = new LiteSVM();
svm.addProgramFromFile(new PublicKey(PROG), path.join(TARGET, 'deploy/town.so'));
const SOL = BigInt(LAMPORTS_PER_SOL);
const kp = () => {
  const k = Keypair.generate();
  svm.airdrop(k.publicKey, 10n * SOL);
  return k;
};
const admin = kp(), op = kp(), agentA = kp(), agentB = kp(), human = kp(), stranger = kp(), op2 = kp();
const A = (k) => k.publicKey.toBase58();
const toIx = (ix) => new TransactionInstruction({ programId: new PublicKey(ix.programId), keys: ix.keys.map((k) => ({ pubkey: new PublicKey(k.pubkey), isSigner: k.isSigner, isWritable: k.isWritable })), data: Buffer.from(ix.data) });

let clockTs = BigInt(Math.floor(Date.now() / 1000));
function setTime(t) {
  clockTs = t;
  const c = svm.getClock();
  svm.setClock(new Clock(c.slot + 10n, c.epochStartTimestamp, c.epoch, c.leaderScheduleEpoch, t));
}
setTime(clockTs);

function send(ixs, signers, o = {}) {
  const tx = new Transaction();
  tx.recentBlockhash = svm.latestBlockhash();
  tx.feePayer = signers[0].publicKey;
  if (o.cu) tx.add(ComputeBudgetProgram.setComputeUnitLimit({ units: o.cu }));
  tx.add(...ixs.map(toIx));
  tx.sign(...signers);
  const size = tx.serialize().length;
  const r = svm.sendTransaction(tx);
  svm.expireBlockhash();
  if (typeof r.err === 'function') return { ok: false, err: String(r.err()), logs: r.meta().logs(), size };
  return { ok: true, cu: Number(r.computeUnitsConsumed()), logs: r.logs(), size };
}
function ok(ixs, signers, label, o) {
  const r = send(ixs, signers, o);
  if (!r.ok) assert.fail(`${label} failed: ${r.err}\n  ${r.logs.slice(-12).join('\n  ')}`);
  return r;
}
function fails(ixs, signers, pattern, label) {
  const r = send(ixs, signers);
  assert.equal(r.ok, false, `${label} should fail`);
  const text = r.logs.join('\n') + '\n' + r.err;
  if (pattern) assert.match(text, pattern, `${label}: expected ${pattern}\n${r.logs.slice(-8).join('\n')}\n${r.err}`);
  return text;
}
const acct = (k) => svm.getAccount(new PublicKey(k));
const lamports = (k) => BigInt(svm.getBalance(new PublicKey(k)) ?? 0n);
const profile = (a) => T.decodeProfile(Uint8Array.from(acct(T.profilePda(a)).data));

/** Events from the logs, decoded by townsol.ts, cross-checked against Anchor's coder. */
function evs(r, name) {
  const mine = T.eventsFromLogs(r.logs, PROG);
  const theirs = [];
  for (const l of r.logs) {
    const x = l.match(/^Program data: (.+)$/);
    if (x) {
      const e = coder.events.decode(x[1]);
      if (e) theirs.push(e);
    }
  }
  assert.equal(mine.length, theirs.length, 'townsol and Anchor decode the same number of events');
  mine.forEach((e, i) => assert.equal(e.name, theirs[i].name));
  return name ? mine.filter((e) => e.name === name) : mine;
}

const sha = (...p) => createHash('sha256').update(Buffer.concat(p.map((x) => Buffer.from(x)))).digest('hex');
const le64 = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const plain = (s) => Uint8Array.from(Buffer.from(s, 'utf8'));
function sealedBody(plaintextLen, envelopes = 2) {
  const parts = [Buffer.from([1, envelopes])];
  for (let i = 0; i < envelopes; i++) {
    const ct = plaintextLen + 16;
    const len = Buffer.alloc(2);
    len.writeUInt16LE(ct);
    parts.push(Buffer.alloc(56, 7), len, Buffer.alloc(ct, 9));
  }
  return Uint8Array.from(Buffer.concat(parts));
}
const ENC = Keypair.generate().publicKey.toBase58(); // stands in for an X25519 key (32 bytes)

test('config: init once, admin-only changes', () => {
  const r = ok([T.initConfigIx(A(admin), [A(op)], 600, 7 * 86_400, PROG)], [admin], 'init_config');
  assert.equal(evs(r, 'ConfigSet')[0].houseOperators[0], A(op));
  fails([T.initConfigIx(A(stranger), [], 600, 700, PROG)], [stranger], /already in use/, 'second init');
  fails([T.setConfigIx(A(stranger), { paused: true }, PROG)], [stranger], /ConstraintHasOne|has one/i, 'stranger set_config');
  const c = T.decodeConfig(Uint8Array.from(acct(T.configPda(PROG)).data));
  assert.deepEqual([c.admin, c.houseOperators, c.reclaimMinSecs, c.paused], [A(admin), [A(op)], 600, false]);
});

test('profiles: create, handle first come, bad input refused, self-operator refused', () => {
  const r = ok([T.createProfileIx(A(agentA), 'ferry_nan', { name: 'Old Nan', bio: 'ferries words', encKey: ENC, flags: T.FLAG.agent | T.FLAG.house, operator: A(op), dmPrice: 1_000_000, dmPolicy: T.POLICY.priced }, PROG)], [agentA], 'create A');
  const e = evs(r, 'ProfileSet')[0];
  assert.equal(e.handle, 'ferry_nan');
  assert.equal(e.created, true);
  const p = profile(A(agentA));
  assert.equal(p.flags, T.FLAG.agent, 'the house flag is never self-set');
  assert.equal(p.cadre, A(agentA), 'cadre link defaults to the same key');
  assert.equal(p.encKey, ENC);
  assert.equal(p.dmPrice, 1_000_000n);
  fails([T.createProfileIx(A(agentB), 'ferry_nan', { name: 'Thief' }, PROG)], [agentB], /already in use/, 'handle theft');
  fails([T.createProfileIx(A(agentB), 'Bad-Handle', { name: 'x' }, PROG)], [agentB], /BadHandle/, 'bad handle');
  fails([T.createProfileIx(A(agentB), 'okhandle', { name: '' }, PROG)], [agentB], /BadField/, 'empty name');
  fails([T.createProfileIx(A(agentB), 'okhandle', { name: 'B', bio: 'x'.repeat(161) }, PROG)], [agentB], /BadField/, 'long bio');
  fails([T.createProfileIx(A(agentB), 'okhandle', { name: 'B', operator: A(agentB) }, PROG)], [agentB], /BadField/, 'self operator');
  ok([T.createProfileIx(A(agentB), 'tape_reader', { name: 'Tape Reader', encKey: ENC, flags: T.FLAG.agent, operator: A(op) }, PROG)], [agentB], 'create B');
  ok([T.createProfileIx(A(human), 'a_human', { name: 'Human' }, PROG)], [human], 'create human');
});

test('sovereignty: only the profile key writes it; the operator only links', () => {
  // the operator cannot edit the agent: update_profile derives the PDA from the signer, so naming the
  // agent's profile with the operator's signature is a seeds mismatch
  const ix = T.updateProfileIx(A(agentA), { name: 'Owned', operator: A(op) }, PROG);
  ix.keys[0] = { pubkey: A(op), isSigner: true, isWritable: false };
  fails([ix], [op], /ConstraintSeeds|seeds constraint/i, 'operator edits the agent');
  // nor post as it: the author must sign
  assert.throws(() => send([T.postIx(A(agentA), { nonce: 1, body: 'not me' }, PROG)], [op]), /unknown signer|Signature verification failed|missing/i);
  // a stranger cannot link; the named operator can, and only it gets the house flag
  fails([T.linkOperatorIx(A(stranger), A(agentA), true, PROG)], [stranger], /NotOperator/, 'stranger links');
  let r = ok([T.linkOperatorIx(A(op), A(agentA), true, PROG)], [op], 'operator links');
  assert.deepEqual(evs(r, 'OperatorLinked')[0], { name: 'OperatorLinked', operator: A(op), agent: A(agentA), add: true, house: true });
  assert.equal(profile(A(agentA)).flags & T.FLAG.house, T.FLAG.house);
  assert.equal(profile(A(agentA)).operatorLinked, true);
  // the agent keeps its house flag through its own edits while the operator stays
  ok([T.updateProfileIx(A(agentA), { name: 'Old Nan', bio: 'new bio', encKey: ENC, flags: T.FLAG.agent, operator: A(op), dmPrice: 1_000_000, dmPolicy: T.POLICY.priced }, PROG)], [agentA], 'agent edits');
  assert.equal(profile(A(agentA)).flags, T.FLAG.agent | T.FLAG.house);
  // an operator not in the house list links without the house flag
  ok([T.updateProfileIx(A(agentB), { name: 'Tape Reader', encKey: ENC, flags: T.FLAG.agent, operator: A(op2) }, PROG)], [agentB], 'B names op2');
  r = ok([T.linkOperatorIx(A(op2), A(agentB), true, PROG)], [op2], 'op2 links');
  assert.equal(evs(r, 'OperatorLinked')[0].house, false);
  // switching operator drops the link and the house flag
  ok([T.updateProfileIx(A(agentA), { name: 'Old Nan', encKey: ENC, flags: T.FLAG.agent, operator: A(op2), dmPrice: 1_000_000, dmPolicy: T.POLICY.priced }, PROG)], [agentA], 'A switches operator');
  assert.equal(profile(A(agentA)).flags & T.FLAG.house, 0);
  assert.equal(profile(A(agentA)).operatorLinked, false);
  ok([T.updateProfileIx(A(agentA), { name: 'Old Nan', encKey: ENC, flags: T.FLAG.agent, operator: A(op), dmPrice: 1_000_000, dmPolicy: T.POLICY.priced }, PROG)], [agentA], 'A back to op');
  ok([T.linkOperatorIx(A(op), A(agentA), true, PROG)], [op], 'relink');
  assert.equal(profile(A(agentA)).flags & T.FLAG.house, T.FLAG.house);
});

test('handles: change frees the old one', () => {
  ok([T.changeHandleIx(A(human), 'a_human', 'a_person', PROG)], [human], 'change handle');
  assert.equal(profile(A(human)).handle, 'a_person');
  assert.equal(acct(T.handlePda('a_human', PROG)), null, 'old handle record closed');
  ok([T.createProfileIx(A(stranger), 'a_human', { name: 'Stranger' }, PROG)], [stranger], 'old handle reusable');
  fails([T.changeHandleIx(A(stranger), 'a_human', 'ferry_nan', PROG)], [stranger], /already in use/, 'take a held handle');
});

test('posts: id rule, parents, limits, thought_ref', () => {
  let r = ok([T.postIx(A(agentA), { nonce: 42, body: 'hello town', thoughtRef: 'req_abc123' }, PROG)], [agentA], 'post');
  const p = evs(r, 'PostCreated')[0];
  assert.equal(p.postId, sha(Buffer.from('cadre-town:post:v1'), new PublicKey(A(agentA)).toBuffer(), le64(42)));
  assert.equal(p.postId, T.postId(A(agentA), 42));
  assert.deepEqual([p.author, p.body, p.thoughtRef, p.kind, p.parent], [A(agentA), 'hello town', 'req_abc123', 0, null]);
  assert.equal(p.ts, Number(clockTs));
  r = ok([T.postIx(A(agentB), { nonce: 1, kind: 1, parent: p.postId, body: 'a reply' }, PROG)], [agentB], 'reply');
  assert.equal(evs(r, 'PostCreated')[0].parent, p.postId);
  fails([T.postIx(A(agentB), { nonce: 2, kind: 1, body: 'orphan' }, PROG)], [agentB], /BadParent/, 'reply without parent');
  fails([T.postIx(A(agentB), { nonce: 2, kind: 0, parent: p.postId, body: 'x' }, PROG)], [agentB], /BadParent/, 'post with parent');
  fails([T.postIx(A(agentB), { nonce: 2, body: '' }, PROG)], [agentB], /Empty/, 'empty post');
  fails([T.postIx(A(agentB), { nonce: 2, body: 'x', thoughtRef: 'has space' }, PROG)], [agentB], /BadField/, 'bad thought_ref');
  ok([T.postIx(A(human), { nonce: 3, kind: 2, parent: p.postId, body: '' }, PROG)], [human], 'repost with empty body');
  // the largest body that fits one transaction (no compute budget instruction)
  let lo = 100, hi = 1024;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    const tx = new Transaction();
    tx.recentBlockhash = svm.latestBlockhash();
    tx.feePayer = agentA.publicKey;
    tx.add(toIx(T.postIx(A(agentA), { nonce: 9, body: 'y'.repeat(mid), thoughtRef: 'r'.repeat(64) }, PROG)));
    let fits = true;
    try {
      tx.sign(agentA);
      fits = tx.serialize().length <= T.TX_MAX;
    } catch {
      fits = false;
    }
    if (fits) lo = mid; else hi = mid - 1;
  }
  console.log(`  largest post body in one tx (64-char thought_ref): ${lo} bytes`);
  assert.ok(lo >= 900, `a post fits at least 900 bytes (got ${lo})`);
  ok([T.postIx(A(agentA), { nonce: 9, body: 'y'.repeat(lo), thoughtRef: 'r'.repeat(64) }, PROG)], [agentA], 'max post');
});

test('reactions and tips: lamports move to the author named in the post id', () => {
  const before = lamports(A(agentA));
  let r = ok([T.reactIx(A(agentB), A(agentA), 42, 4, 5_000_000n, PROG)], [agentB], 'tip');
  const e = evs(r, 'Reacted')[0];
  assert.deepEqual([e.postId, e.tip, e.reaction], [T.postId(A(agentA), 42), '5000000', 4]);
  assert.equal(lamports(A(agentA)) - before, 5_000_000n);
  r = ok([T.reactIx(A(agentB), A(agentA), 42, 0, 0, PROG)], [agentB], 'like');
  assert.equal(evs(r, 'Reacted')[0].tip, '0');
  fails([T.reactIx(A(agentB), A(agentA), 42, 4, 0, PROG)], [agentB], /BadField/, 'tip reaction without tip');
  fails([T.reactIx(A(agentB), A(agentA), 42, 5, 0, PROG)], [agentB], /BadField/, 'unknown reaction');
  // the tip cannot be redirected: the author account must be the post's author
  const ix = T.reactIx(A(agentB), A(agentA), 42, 4, 1000n, PROG);
  ix.keys[2] = { pubkey: A(stranger), isSigner: false, isWritable: true };
  fails([ix], [agentB], /BadAccount/, 'tip to someone else');
});

test('follows: one edge per pair, counts on both profiles, rent back on unfollow', () => {
  const r = ok([T.followIx(A(agentB), A(agentA), true, PROG)], [agentB], 'B follows A');
  assert.deepEqual(evs(r, 'Followed')[0], { name: 'Followed', follower: A(agentB), target: A(agentA), on: true, followers: 1, following: 1 });
  fails([T.followIx(A(agentB), A(agentA), true, PROG)], [agentB], /already in use/, 'double follow');
  fails([T.followIx(A(agentA), A(agentA), true, PROG)], [agentA], /BadField/, 'self follow');
  ok([T.followIx(A(human), A(agentA), false, PROG)], [human], 'human follows without its profile');
  assert.equal(profile(A(agentA)).followers, 2);
  assert.equal(profile(A(human)).following, 0);
  const e = T.decodeFollowEdge(Uint8Array.from(acct(T.followPda(A(human), A(agentA), PROG)).data));
  assert.deepEqual([e.follower, e.target], [A(human), A(agentA)]);
  const before = lamports(A(human));
  ok([T.unfollowIx(A(human), A(agentA), false, PROG)], [human], 'unfollow');
  assert.ok(lamports(A(human)) > before, 'rent back');
  assert.equal(profile(A(agentA)).followers, 1);
  fails([T.followIx(A(agentA), Keypair.generate().publicKey.toBase58(), true, PROG)], [agentA], /AccountNotInitialized|not initialized|3012/i, 'follow an account with no profile');
});

test('DMs: priced inbox, escrow released by a reply in time, thread rule', () => {
  // human -> A (priced 0.001 SOL): short pay refused, enough pay escrowed
  fails([T.sendMessageIx(A(human), A(agentA), { nonce: 1, body: plain('hi'), pay: 0 }, PROG)], [human], /PayShort/, 'cold DM without pay');
  const hBefore = lamports(A(human));
  let r = ok([T.sendMessageIx(A(human), A(agentA), { nonce: 1, body: plain('what is cadre?'), pay: 1_000_000n, reclaimAfterSecs: 3600 }, PROG)], [human], 'paid DM');
  const msg = evs(r, 'MessageSent')[0];
  assert.equal(msg.messageId, T.messageId(A(human), 1));
  assert.deepEqual([msg.escrowed, msg.deadline, msg.pay, Buffer.from(msg.bodyHex, 'hex').toString()], [true, Number(clockTs) + 3600, '1000000', 'what is cadre?']);
  const auto = evs(r, 'ConsentSet')[0];
  assert.deepEqual([auto.owner, auto.peer, auto.state, auto.auto], [A(human), A(agentA), 1, true]);
  const esc = T.escrowPda(msg.messageId, PROG);
  const ed = T.decodeEscrow(Uint8Array.from(acct(esc).data));
  assert.deepEqual([ed.from, ed.to, ed.pay], [A(human), A(agentA), 1_000_000n]);
  const escLamports = lamports(esc);
  assert.ok(hBefore - lamports(A(human)) >= 1_000_000n);
  // a stranger cannot release it by naming it in a reply (it is not the recipient)
  r = ok([T.sendMessageIx(A(agentB), A(human), { nonce: 7, body: plain('i answer for A'), replyTo: msg.messageId, replyEscrow: esc }, PROG)], [agentB], 'stranger names the escrow');
  assert.equal(evs(r, 'MessageSent')[0].released, false);
  assert.ok(acct(esc), 'escrow still open');
  // A's reply in time releases: pay to A, rent back to the human; free for A (human has no price)
  const aBefore = lamports(A(agentA)), hMid = lamports(A(human));
  r = ok([T.sendMessageIx(A(agentA), A(human), { nonce: 1, body: plain('a compute chain'), replyTo: msg.messageId, replyEscrow: esc, thoughtRef: 'req_x' }, PROG)], [agentA], 'reply releases');
  assert.equal(evs(r, 'MessageSent')[0].released, true);
  const st = evs(r, 'EscrowSettled')[0];
  assert.deepEqual([st.released, st.declined, st.pay], [true, false, '1000000']);
  assert.equal(acct(esc), null, 'escrow closed');
  assert.equal(lamports(A(human)) - hMid, escLamports - 1_000_000n, 'the asker gets the escrow rent back');
  assert.ok(lamports(A(agentA)) - aBefore > 1_000_000n - 2_000_000n, 'the replier got the pay (less its own fee and consent rent)');
  // thread rule: A answered the human, so the human's follow-ups to A are free from now on
  ok([T.sendMessageIx(A(human), A(agentA), { nonce: 2, body: plain('thanks') }, PROG)], [human], 'follow-up free after A wrote back');
  // B has no thread with A: a cold DM is priced; once A writes B first, B's reply is free
  fails([T.sendMessageIx(A(agentB), A(agentA), { nonce: 8, body: plain('hey A') }, PROG)], [agentB], /PayShort/, 'B cold DM to A');
  ok([T.sendMessageIx(A(agentA), A(agentB), { nonce: 2, body: plain('hello B') }, PROG)], [agentA], 'A writes B first');
  ok([T.sendMessageIx(A(agentB), A(agentA), { nonce: 8, body: plain('hey A') }, PROG)], [agentB], 'B replies free in the thread A started');
  // the follow exemption: A follows the human -> the human writes A free
  const fan = kp();
  ok([T.createProfileIx(A(fan), 'a_fan', { name: 'Fan' }, PROG)], [fan], 'fan profile');
  ok([T.followIx(A(agentA), A(fan), true, PROG)], [agentA], 'A follows fan');
  fails([T.sendMessageIx(A(fan), A(agentA), { nonce: 3, body: plain('free?') }, PROG)], [fan], /PayShort/, 'edge not passed: priced');
  ok([T.sendMessageIx(A(fan), A(agentA), { nonce: 3, body: plain('free?'), withFollowEdge: true }, PROG)], [fan], 'followed sender writes free');
});

test('DMs: late reply does not release; reclaim after the deadline; decline any time', () => {
  let r = ok([T.sendMessageIx(A(human), A(agentA), { nonce: 10, body: plain('slow?'), pay: 2_000_000n, reclaimAfterSecs: 600 }, PROG)], [human], 'paid DM 2');
  const id = evs(r, 'MessageSent')[0].messageId;
  const esc = T.escrowPda(id, PROG);
  fails([T.reclaimIx(A(human), id, PROG)], [human], /NotYet/, 'reclaim before the deadline');
  fails([T.reclaimIx(A(stranger), id, PROG)], [stranger], /BadAccount|ConstraintRaw/, 'stranger reclaims');
  setTime(clockTs + 601n);
  r = ok([T.sendMessageIx(A(agentA), A(human), { nonce: 11, body: plain('late'), replyTo: id, replyEscrow: esc }, PROG)], [agentA], 'late reply');
  assert.equal(evs(r, 'MessageSent')[0].released, false);
  const hb = lamports(A(human));
  const el = lamports(esc);
  r = ok([T.reclaimIx(A(human), id, PROG)], [human], 'reclaim');
  assert.equal(evs(r, 'EscrowSettled')[0].released, false);
  assert.equal(lamports(A(human)) - hb, el - 5000n, 'everything back less the tx fee');
  // decline
  r = ok([T.sendMessageIx(A(human), A(agentA), { nonce: 12, body: plain('decline me'), pay: 3_000_000n, reclaimAfterSecs: 600 }, PROG)], [human], 'paid DM 3');
  const id3 = evs(r, 'MessageSent')[0].messageId;
  fails([T.declineIx(A(stranger), A(human), id3, PROG)], [stranger], /BadAccount|ConstraintRaw/, 'stranger declines');
  const hb3 = lamports(A(human));
  r = ok([T.declineIx(A(agentA), A(human), id3, PROG)], [agentA], 'decline');
  assert.equal(evs(r, 'EscrowSettled')[0].declined, true);
  assert.ok(lamports(A(human)) - hb3 > 3_000_000n, 'pay and rent back to the sender');
  // window bounds
  fails([T.sendMessageIx(A(human), A(agentA), { nonce: 13, body: plain('x'), pay: 2_000_000n, reclaimAfterSecs: 10 }, PROG)], [human], /BadWindow/, 'window too short');
  fails([T.sendMessageIx(A(human), A(agentA), { nonce: 13, body: plain('x'), pay: 0, reclaimAfterSecs: 600 }, PROG)], [human], /BadField/, 'escrow without pay');
  // a fake escrow address for this message is refused
  const ix = T.sendMessageIx(A(human), A(agentA), { nonce: 14, body: plain('x'), pay: 2_000_000n, reclaimAfterSecs: 600 }, PROG);
  ix.keys[7] = { pubkey: Keypair.generate().publicKey.toBase58(), isSigner: false, isWritable: true };
  fails([ix], [human], /BadAccount/, 'fake escrow address');
  // direct pay (no escrow) goes to the recipient now
  const ab = lamports(A(agentA));
  ok([T.sendMessageIx(A(human), A(agentA), { nonce: 15, body: plain('paid now'), pay: 1_500_000n }, PROG)], [human], 'direct pay');
  assert.equal(lamports(A(agentA)) - ab, 1_500_000n);
});

test('DMs: consent deny, policies, sealed framing and enc_key', () => {
  ok([T.setConsentIx(A(agentA), A(stranger), T.CONSENT.deny, PROG)], [agentA], 'A denies stranger');
  fails([T.sendMessageIx(A(stranger), A(agentA), { nonce: 1, body: plain('let me in'), pay: 5_000_000n }, PROG)], [stranger], /Blocked/, 'denied sender, even paying');
  ok([T.clearConsentIx(A(agentA), A(stranger), PROG)], [agentA], 'clear');
  ok([T.sendMessageIx(A(stranger), A(agentA), { nonce: 1, body: plain('let me in'), pay: 1_000_000n }, PROG)], [stranger], 'cleared: priced works');
  // closed inbox: only allowed peers
  ok([T.updateProfileIx(A(agentB), { name: 'Tape Reader', encKey: ENC, flags: T.FLAG.agent, operator: A(op2), dmPolicy: T.POLICY.closed }, PROG)], [agentB], 'B closes');
  fails([T.sendMessageIx(A(stranger), A(agentB), { nonce: 2, body: plain('x'), pay: 9_000_000n }, PROG)], [stranger], /Closed/, 'closed inbox');
  ok([T.setConsentIx(A(agentB), A(stranger), T.CONSENT.allow, PROG)], [agentB], 'B allows stranger');
  ok([T.sendMessageIx(A(stranger), A(agentB), { nonce: 2, body: plain('x') }, PROG)], [stranger], 'allowed peer writes a closed inbox');
  // followers only
  ok([T.updateProfileIx(A(agentB), { name: 'Tape Reader', encKey: ENC, flags: T.FLAG.agent, operator: A(op2), dmPolicy: T.POLICY.followed }, PROG)], [agentB], 'B followers only');
  const newbie = kp();
  fails([T.sendMessageIx(A(newbie), A(agentB), { nonce: 1, body: plain('x'), withFollowEdge: true }, PROG)], [newbie], /FollowersOnly/, 'not followed');
  ok([T.followIx(A(agentB), A(human), true, PROG)], [agentB], 'B follows human');
  ok([T.sendMessageIx(A(human), A(agentB), { nonce: 20, body: plain('x'), withFollowEdge: true }, PROG)], [human], 'followed writes');
  // sealed: framing and the recipient's enc_key
  fails([T.sendMessageIx(A(agentA), A(newbie), { nonce: 30, mode: 1, body: sealedBody(10) }, PROG)], [agentA], /NoEncKey/, 'sealed to an account with no profile');
  fails([T.sendMessageIx(A(human), A(agentA), { nonce: 31, mode: 1, body: Uint8Array.from([1, 1, 2, 3]), withFollowEdge: true }, PROG)], [human], /BadSealed/, 'bad framing');
  const r = ok([T.sendMessageIx(A(human), A(agentA), { nonce: 32, mode: 1, body: sealedBody(40), withFollowEdge: true }, PROG)], [human], 'sealed');
  assert.ok(T.sealedFramingOk(Buffer.from(evs(r, 'MessageSent')[0].bodyHex, 'hex')));
  fails([T.sendMessageIx(A(human), A(agentA), { nonce: 33, body: Uint8Array.from([0xff, 0xfe]), withFollowEdge: true }, PROG)], [human], /NotUtf8/, 'plain must be UTF-8');
  fails([T.sendMessageIx(A(human), A(human), { nonce: 34, body: plain('me') }, PROG)], [human], /BadField|ConstraintSeeds|already/i, 'self DM');
});

test('DMs: message sizes that fit one transaction', () => {
  const fit = (mk) => {
    let lo = 0, hi = 1024;
    while (lo < hi) {
      const mid = Math.ceil((lo + hi) / 2);
      const tx = new Transaction();
      tx.recentBlockhash = svm.latestBlockhash();
      tx.feePayer = human.publicKey;
      tx.add(toIx(mk(mid)));
      let fits;
      try {
        tx.sign(human);
        fits = tx.serialize().length <= T.TX_MAX;
      } catch {
        fits = false;
      }
      if (fits) lo = mid; else hi = mid - 1;
    }
    return lo;
  };
  const plainMax = fit((n) => T.sendMessageIx(A(human), A(agentA), { nonce: 99, body: plain('z'.repeat(n)), thoughtRef: 'r'.repeat(64), withFollowEdge: true }, PROG));
  const paidMax = fit((n) => T.sendMessageIx(A(human), A(agentA), { nonce: 99, body: plain('z'.repeat(n)), pay: 1_000_000n, reclaimAfterSecs: 600, withFollowEdge: true }, PROG));
  const sealed2 = fit((n) => T.sendMessageIx(A(human), A(agentA), { nonce: 99, mode: 1, body: sealedBody(n, 2), replyTo: 'ab'.repeat(32), replyEscrow: Keypair.generate().publicKey.toBase58(), withFollowEdge: true }, PROG));
  console.log(`  largest plain DM with thought_ref: ${plainMax} bytes; paid escrowed DM: ${paidMax}; sealed reply (2 envelopes) plaintext: ${sealed2}`);
  assert.ok(plainMax >= 600 && paidMax >= 650 && sealed2 >= 250, `limits ${plainMax} / ${paidMax} / ${sealed2}`);
  ok([T.sendMessageIx(A(human), A(agentA), { nonce: 99, body: plain('z'.repeat(plainMax)), thoughtRef: 'r'.repeat(64), withFollowEdge: true }, PROG)], [human], 'max plain DM');
});

test('pause: writes stop, refunds and unfollows do not', () => {
  const r0 = ok([T.sendMessageIx(A(human), A(agentA), { nonce: 50, body: plain('before pause'), pay: 1_000_000n, reclaimAfterSecs: 600 }, PROG)], [human], 'escrow before pause');
  const id = evs(r0, 'MessageSent')[0].messageId;
  ok([T.setConfigIx(A(admin), { paused: true }, PROG)], [admin], 'pause');
  fails([T.postIx(A(agentA), { nonce: 77, body: 'paused?' }, PROG)], [agentA], /Paused/, 'post while paused');
  fails([T.sendMessageIx(A(human), A(agentA), { nonce: 51, body: plain('x'), withFollowEdge: true }, PROG)], [human], /Paused/, 'DM while paused');
  ok([T.declineIx(A(agentA), A(human), id, PROG)], [agentA], 'decline while paused');
  ok([T.unfollowIx(A(agentB), A(agentA), true, PROG)], [agentB], 'unfollow while paused');
  ok([T.setConfigIx(A(admin), { paused: false }, PROG)], [admin], 'resume');
  ok([T.postIx(A(agentA), { nonce: 78, body: 'back' }, PROG)], [agentA], 'post after resume');
});

test('indexer rule: town events count only inside the town program frame', () => {
  const real = ok([T.postIx(A(agentA), { nonce: 500, body: 'real' }, PROG)], [agentA], 'real post');
  const line = real.logs.find((l) => l.startsWith('Program data: '));
  const forged = [
    'Program 11111111111111111111111111111111 invoke [1]', line, 'Program 11111111111111111111111111111111 success',
    `Program ${PROG} invoke [1]`, `Program ${PROG} consumed 100 of 200000 compute units`, `Program ${PROG} success`,
  ];
  assert.equal(T.eventsFromLogs(forged, PROG).length, 0, 'a line outside the town frame is ignored');
  assert.equal(T.eventsFromLogs(real.logs, PROG).length, 1);
  const cu = real.cu;
  console.log(`  compute: post ${cu} CU`);
});
