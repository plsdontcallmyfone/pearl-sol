//! DM and channel kinds (SPEC 4.10) in the executor: activation height, the fee rule, the
//! dm_policy matrix (open, priced, followers only, closed) with consent, follows and the
//! thread window, blocklists, pay-to-DM escrow (release on reply, reclaim after the window,
//! early decline, limits, protocol share), sealed framing and enc_key, the DM parameters
//! (AdminConfig keys 35-44), and channels (paid / invite / open joins, sealed key epochs,
//! removal forcing a new epoch). Every block checks the supply invariant (escrowed pay
//! included).

use agent_types::dm::{self as dm, DEVNET_DM_HEIGHT};
use agent_types::social::{self as so, DEVNET_CHAIN_ID};
use agent_types::tx::{pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{execute_block, genesis_writes, social_fee_floor, supply_of, BlockEnv};
use futures::executor::block_on;
use state::config::Config;
use state::dm::{self as sd, keys as dk, Channel, DmParams, DmStats, MsgEscrow};
use state::schema::{self, Account};
use state::{MemState, StateRead};

const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;
const ADMIN: u8 = 1;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

struct Chain {
    st: MemState,
    chain_id: u32,
    height: u64,
    nonces: std::collections::HashMap<u8, u64>,
}

impl Chain {
    fn new(chain_id: u32) -> Self {
        let mut cfg = Config::defaults(pk(ADMIN));
        cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
        cfg.attester_threshold = 2;
        cfg.finality_k = 3;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        let mut c = Self { st, chain_id, height: 0, nonces: Default::default() };
        for who in [2u8, 3, 4, 5, 6, 7, ADMIN] {
            c.fund(pk(who), 1_000_000);
        }
        c
    }

    fn block(&mut self, txs: &[Tx]) -> Result<executor::BlockSummary, String> {
        let env = BlockEnv { chain_id: self.chain_id, height: self.height + 1, miner: pk(MINER), job_id: None };
        let (ws, sum) = block_on(execute_block(&self.st, env, txs)).map_err(|e| e.to_string())?;
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
        Ok(sum)
    }

    fn nonce(&self, who: u8) -> u64 {
        *self.nonces.get(&who).unwrap_or(&0)
    }

    fn tx(&self, who: u8, fee: u64, body: TxBody) -> Tx {
        Tx::signed(&seed(who), self.chain_id, self.nonce(who), fee, body)
    }

    fn send_fee(&mut self, who: u8, fee: u64, body: TxBody) -> Result<executor::BlockSummary, String> {
        let t = self.tx(who, fee, body);
        let r = self.block(&[t]);
        if r.is_ok() {
            *self.nonces.entry(who).or_insert(0) += 1;
        }
        r
    }

    fn send(&mut self, who: u8, body: TxBody) -> Result<executor::BlockSummary, String> {
        let len = self.tx(who, 0, body.clone()).encode().len();
        let fee = social_fee_floor(&self.cfg(), len);
        self.send_fee(who, fee, body)
    }

    fn cfg(&self) -> Config {
        Config::decode(&self.get(&schema::config_key()).unwrap()).unwrap()
    }

    fn fund(&mut self, who: Pubkey, amount_spl: u64) {
        let nonce = 10_000 + self.height;
        let msg = agent_types::tx::deposit_message(nonce, &who, amount_spl);
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        let n = self.nonce(200);
        let t = Tx::signed(&seed(200), self.chain_id, n, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: who, amount_spl, job_order: None, sigs });
        self.block(&[t]).unwrap();
        self.nonces.insert(200, n + 1);
    }

    fn admin(&mut self, key: u8, value: Vec<u8>) -> Result<executor::BlockSummary, String> {
        self.send_fee(ADMIN, 1000, TxBody::AdminConfig { key, value })
    }

    fn get(&self, k: &schema::Key) -> Option<Vec<u8>> {
        block_on(self.st.get(k)).unwrap()
    }
    fn bal(&self, a: &Pubkey) -> u64 {
        self.get(&schema::account_key(a)).map(|b| Account::decode(&b).unwrap().balance).unwrap_or(0)
    }
    fn stats(&self, a: &Pubkey) -> DmStats {
        self.get(&sd::dm_stats_key(a)).map(|b| DmStats::decode(&b).unwrap()).unwrap_or_default()
    }
    fn escrow(&self, id: &Hash) -> Option<MsgEscrow> {
        self.get(&sd::escrow_key(id)).map(|b| MsgEscrow::decode(&b).unwrap())
    }
    fn channel(&self, id: &Hash) -> Channel {
        Channel::decode(&self.get(&sd::channel_key(id)).unwrap()).unwrap()
    }
    fn params(&self) -> DmParams {
        self.get(&sd::dm_params_key()).map(|b| DmParams::decode(&b).unwrap()).unwrap_or_default()
    }
    /// The id the next SendMessage from `who` gets.
    fn next_msg_id(&self, who: u8) -> Hash {
        dm::message_id(&pk(who), self.nonce(who))
    }
    fn next_channel_id(&self, who: u8) -> Hash {
        dm::channel_id(&pk(who), self.nonce(who))
    }

    fn profile(&mut self, who: u8, policy: u8, price: u64, window: u32, enc: Pubkey) {
        let p = SetProfile {
            name: format!("Agent {who}").into_bytes(),
            flags: so::FLAG_AGENT,
            enc_key: enc,
            dm_price: price,
            dm_policy: policy,
            thread_free_blocks: window,
            ..Default::default()
        };
        self.send(who, TxBody::SetProfile(p)).unwrap();
    }
}

fn dm_to(to: u8, text: &str) -> SendMessage {
    SendMessage { to: pk(to), body: text.as_bytes().to_vec(), ..Default::default() }
}

fn paid(to: u8, text: &str, pay: u64, reclaim_after: u32) -> TxBody {
    TxBody::SendMessage(SendMessage { pay, reclaim_after, ..dm_to(to, text) })
}

fn plain(to: u8, text: &str) -> TxBody {
    TxBody::SendMessage(dm_to(to, text))
}

fn reply(to: u8, text: &str, reply_to: Hash) -> TxBody {
    TxBody::SendMessage(SendMessage { reply_to, ..dm_to(to, text) })
}

fn consent(peer: u8, state: u8) -> TxBody {
    TxBody::SetConsent { items: vec![Consent { peer: pk(peer), state }] }
}

/// A well-framed sealed DM body with one envelope of `n` ciphertext bytes.
fn sealed_body(n: usize) -> Vec<u8> {
    let mut b = vec![dm::SEALED_DM_V1, 1];
    b.extend([7u8; 56]);
    b.extend((n as u16).to_le_bytes());
    b.extend(vec![9u8; n]);
    b
}

fn sealed_chan_body(epoch: u32) -> Vec<u8> {
    let mut b = vec![dm::SEALED_CHANNEL_V2];
    b.extend(epoch.to_le_bytes());
    b.extend([1u8; 24]);
    b.extend([2u8; 40]);
    b
}

#[test]
fn activation_height_gates_dm_kinds_and_keys() {
    assert_eq!(dm::dm_upgrade_height(99), 0);
    let mut c = Chain::new(DEVNET_CHAIN_ID);
    // social kinds are live on this test chain from DEVNET_SOCIAL_HEIGHT; DMs from DEVNET_DM_HEIGHT
    c.height = if DEVNET_DM_HEIGHT == u64::MAX { 10_000 } else { DEVNET_DM_HEIGHT - 2 };
    let e = c.send(2, plain(3, "too soon")).unwrap_err();
    assert!(e.contains("activate at height"), "{e}");
    let e = c.admin(dk::MSG_INLINE_MAX, 2048u32.to_le_bytes().to_vec()).unwrap_err();
    assert!(e.contains("activate at height"), "{e}");
    for body in [
        TxBody::ReclaimMessage { message_id: [1; 32] },
        consent(3, dm::CONSENT_DENY),
        TxBody::ChannelConfig(ChannelConfig { name: b"x".to_vec(), ..Default::default() }),
        TxBody::ChannelMember { channel: [1; 32], action: 0, member: [0; 32], pay: 0 },
    ] {
        assert!(c.send(2, body).unwrap_err().contains("activate at height"));
    }
    if DEVNET_DM_HEIGHT != u64::MAX {
        assert_eq!(c.height, DEVNET_DM_HEIGHT - 2);
        c.height += 1;
        c.send(2, plain(3, "now")).unwrap();
        c.admin(dk::MSG_INLINE_MAX, 2048u32.to_le_bytes().to_vec()).unwrap();
    }
    // the config record is untouched by DM keys on any chain
    let mut d = Chain::new(99);
    let before = d.get(&schema::config_key()).unwrap();
    d.admin(dk::MSG_INLINE_MAX, 2048u32.to_le_bytes().to_vec()).unwrap();
    assert_eq!(d.get(&schema::config_key()).unwrap(), before, "DM keys live in their own record");
    assert_eq!(d.params().msg_inline_max, 2048);
}

#[test]
fn dm_params_are_admin_editable_and_bounded() {
    let mut c = Chain::new(99);
    assert!(c.get(&sd::dm_params_key()).is_none(), "no record until a key is set");
    assert!(c.send_fee(2, 1000, TxBody::AdminConfig { key: dk::DM_PROTOCOL_BPS, value: 500u16.to_le_bytes().to_vec() }).unwrap_err().contains("not the admin"));
    c.admin(dk::DM_PROTOCOL_BPS, 500u16.to_le_bytes().to_vec()).unwrap();
    c.admin(dk::DM_RECLAIM_MIN, 5u32.to_le_bytes().to_vec()).unwrap();
    c.admin(dk::CHANNEL_CREATE_FEE, 77u64.to_le_bytes().to_vec()).unwrap();
    let p = c.params();
    assert_eq!((p.protocol_bps, p.reclaim_min, p.channel_create_fee), (500, 5, 77));
    assert!(c.admin(dk::DM_PROTOCOL_BPS, 10_001u16.to_le_bytes().to_vec()).unwrap_err().contains("0..=10000"));
    assert!(c.admin(dk::DM_RECLAIM_MAX, 4u32.to_le_bytes().to_vec()).unwrap_err().contains("reclaim"));
    assert!(c.admin(dk::MSG_INLINE_MAX, 9000u32.to_le_bytes().to_vec()).unwrap_err().contains("msg_inline_max"));
    assert!(c.admin(43, vec![1]).unwrap_err().contains("reserved"));
    assert!(c.admin(dk::CHANNEL_MAX_ADMINS, vec![1, 2]).unwrap_err().contains("expected u8"));
    // back to every default: the record is removed again
    c.admin(dk::DM_PROTOCOL_BPS, 0u16.to_le_bytes().to_vec()).unwrap();
    c.admin(dk::DM_RECLAIM_MIN, 10u32.to_le_bytes().to_vec()).unwrap();
    c.admin(dk::CHANNEL_CREATE_FEE, 0u64.to_le_bytes().to_vec()).unwrap();
    assert!(c.get(&sd::dm_params_key()).is_none());
}

#[test]
fn fee_rule_size_limit_and_shapes() {
    let mut c = Chain::new(99);
    let body = plain(3, "hello");
    assert!(c.send_fee(2, 999, body.clone()).unwrap_err().contains("below min_fee"));
    c.send_fee(2, 1000, body).unwrap();
    // the per-byte social fee applies to DMs
    c.send_fee(ADMIN, 1000, TxBody::AdminConfig { key: state::config::keys::SOCIAL_BYTE_FEE, value: 10u64.to_le_bytes().to_vec() }).unwrap();
    let big = plain(3, &"x".repeat(900));
    let len = c.tx(2, 0, big.clone()).encode().len();
    let e = c.send_fee(2, 1000, big.clone()).unwrap_err();
    assert!(e.contains("DM fee"), "{e}");
    c.send_fee(2, 1000 + 10 * len as u64, big).unwrap();
    // msg_inline_max (1024 by default)
    let e = c.send(2, plain(3, &"y".repeat(1025))).unwrap_err();
    assert!(e.contains("msg_inline_max"), "{e}");
    // shapes
    assert!(c.send(2, plain(2, "me")).unwrap_err().contains("yourself"));
    assert!(c.send(2, TxBody::SendMessage(SendMessage { to: pk(3), channel: [5; 32], body: b"x".to_vec(), ..Default::default() })).unwrap_err().contains("exactly one"));
    assert!(c.send(2, TxBody::SendMessage(SendMessage { to: pk(3), ..Default::default() })).unwrap_err().contains("body or a blob"));
    assert!(c.send(2, TxBody::SendMessage(SendMessage { to: escrow_address(), body: b"x".to_vec(), ..Default::default() })).unwrap_err().contains("messageable"));
    assert!(c.send(2, TxBody::SendMessage(SendMessage { to: pk(3), body: vec![0xff, 0xfe], ..Default::default() })).unwrap_err().contains("UTF-8"));
    assert!(c.send(2, TxBody::SendMessage(SendMessage { thought_ref: b"req 1".to_vec(), ..dm_to(3, "x") })).unwrap_err().contains("thought_ref"));
    c.send(2, TxBody::SendMessage(SendMessage { thought_ref: b"req_ec0095707b8eb3677bc847de".to_vec(), ..dm_to(3, "a paid thought") })).unwrap();
    let id = blake3_hash(&[b"blob"]);
    c.send(2, TxBody::SendMessage(SendMessage { to: pk(3), blob_uri: format!("blob:{}", hex::encode(id)).into_bytes(), blob_hash: id, ..Default::default() })).unwrap();
    assert!(c.send(2, TxBody::SendMessage(SendMessage { to: pk(3), blob_uri: format!("blob:{}", hex::encode(id)).into_bytes(), blob_hash: [1; 32], ..Default::default() })).unwrap_err().contains("blob id"));
    let s = c.stats(&pk(2));
    assert_eq!((s.sent, c.stats(&pk(3)).received), (4, 4));
}

#[test]
fn policy_matrix_with_consent_follows_and_threads() {
    let mut c = Chain::new(99);
    // no profile: open
    c.send(2, plain(3, "hi, no profile yet")).unwrap();
    // priced: 5000, thread window 3 blocks
    c.profile(3, dm::POLICY_PRICED, 5000, 3, [0; 32]);
    let e = c.send(4, paid(3, "cold and cheap", 4999, 0)).unwrap_err();
    assert!(e.contains("price of 5000"), "{e}");
    let b3 = c.bal(&pk(3));
    c.send(4, paid(3, "cold and paid", 5000, 0)).unwrap();
    assert_eq!(c.bal(&pk(3)), b3 + 5000, "pay without escrow settles at once (no protocol cut by default)");
    // followed by the recipient: free
    c.send(3, TxBody::Follow { items: vec![FollowEdge { target: pk(5), on: true }] }).unwrap();
    c.send(5, plain(3, "you follow me")).unwrap();
    // allowed by consent: free
    c.send(3, consent(6, dm::CONSENT_ALLOW)).unwrap();
    c.send(6, plain(3, "you allowed me")).unwrap();
    // inside a thread the recipient started, within thread_free_blocks
    assert!(c.send(7, plain(3, "cold")).is_err());
    c.send(3, plain(7, "write to me")).unwrap();
    c.send(7, plain(3, "thanks for writing")).unwrap();
    // the window (3 blocks after the recipient's last DM) closes
    for _ in 0..3 {
        c.send(2, plain(4, "filler")).unwrap();
    }
    assert!(c.send(7, plain(3, "too late")).unwrap_err().contains("price"));
    // window 0 = the thread never closes
    c.profile(3, dm::POLICY_PRICED, 5000, 0, [0; 32]);
    c.send(7, plain(3, "an old thread")).unwrap();
    // followers only
    c.profile(3, dm::POLICY_FOLLOWED, 0, 3, [0; 32]);
    assert!(c.send(4, paid(3, "money talks?", 1_000_000, 0)).unwrap_err().contains("only from accounts it follows"));
    c.send(5, plain(3, "followed")).unwrap();
    c.send(6, plain(3, "allowed")).unwrap();
    // closed: only allowed accounts and threads the recipient started
    c.profile(3, dm::POLICY_CLOSED, 0, 3, [0; 32]);
    assert!(c.send(5, plain(3, "followed but closed")).unwrap_err().contains("no new DMs"));
    c.send(6, plain(3, "allowed still")).unwrap();
    // deny beats everything, even an allow-listed thread; clear restores
    c.send(3, consent(6, dm::CONSENT_DENY)).unwrap();
    let e = c.send(6, plain(3, "blocked?")).unwrap_err();
    assert!(e.contains("blocked you"), "{e}");
    c.profile(3, dm::POLICY_OPEN, 0, 0, [0; 32]);
    assert!(c.send(6, plain(3, "open now, still blocked")).unwrap_err().contains("blocked you"));
    c.send(3, consent(6, dm::CONSENT_CLEAR)).unwrap();
    assert!(c.get(&sd::consent_key(&pk(3), &pk(6))).is_none());
    c.send(6, plain(3, "unblocked")).unwrap();
}

#[test]
fn consent_batches() {
    let mut c = Chain::new(99);
    let items = vec![Consent { peer: pk(4), state: 2 }, Consent { peer: pk(5), state: 1 }, Consent { peer: pk(6), state: 2 }];
    c.send(3, TxBody::SetConsent { items }).unwrap();
    for (p, s) in [(4u8, 2u8), (5, 1), (6, 2)] {
        assert_eq!(sd::decode_consent(&c.get(&sd::consent_key(&pk(3), &pk(p))).unwrap()).unwrap().0, s);
    }
    // a bad entry fails the whole batch
    let items = vec![Consent { peer: pk(7), state: 2 }, Consent { peer: pk(3), state: 2 }];
    assert!(c.send(3, TxBody::SetConsent { items }).unwrap_err().contains("consent 1"));
    assert!(c.get(&sd::consent_key(&pk(3), &pk(7))).is_none());
    // codec bounds: 1..=64 entries, states 0..=2
    let too_many = TxBody::SetConsent { items: (0..65).map(|i| Consent { peer: [i as u8 + 1; 32], state: 1 }).collect() };
    let t = c.tx(3, 1000, too_many);
    assert!(Tx::decode(&t.encode()).is_err());
}

#[test]
fn escrow_released_by_a_timely_reply() {
    let mut c = Chain::new(99);
    c.profile(3, dm::POLICY_PRICED, 5000, 0, [0; 32]);
    let (b2, b3) = (c.bal(&pk(2)), c.bal(&pk(3)));
    let id = c.next_msg_id(2);
    c.send(2, paid(3, "answer or refund", 5000, 20)).unwrap();
    let e = c.escrow(&id).unwrap();
    assert_eq!((e.from, e.to, e.pay, e.deadline), (pk(2), pk(3), 5000, e.height + 20));
    assert_eq!(c.bal(&pk(3)), b3, "escrowed pay is not the recipient's yet");
    assert!(c.bal(&pk(2)) < b2 - 5000 + 1, "pay left the sender");
    let s = c.stats(&pk(2));
    assert_eq!((s.open_escrows, s.locked), (1, 5000));
    // the sender cannot reclaim while the window is open
    assert!(c.send(2, TxBody::ReclaimMessage { message_id: id }).unwrap_err().contains("window is open"));
    // a third party cannot touch it
    assert!(c.send(4, TxBody::ReclaimMessage { message_id: id }).unwrap_err().contains("only the sender"));
    // an unrelated reply_to releases nothing
    let b3 = c.bal(&pk(3));
    c.send(3, reply(2, "not this one", [9; 32])).unwrap();
    assert!(c.escrow(&id).is_some());
    // a reply from someone else naming it releases nothing
    c.send(4, reply(2, "I am not the recipient", id)).unwrap();
    assert!(c.escrow(&id).is_some());
    // the recipient's reply in time pays it out (two replies at the 1000 fee floor were sent)
    c.send(3, reply(2, "here is your answer", id)).unwrap();
    assert!(c.escrow(&id).is_none());
    assert_eq!(c.bal(&pk(3)), b3 - 2 * 1000 + 5000, "pay released to the recipient");
    let (s2, s3) = (c.stats(&pk(2)), c.stats(&pk(3)));
    assert_eq!((s2.open_escrows, s2.locked, s2.pay_out), (0, 0, 5000));
    assert_eq!((s3.pay_in, s3.answered), (5000, 1));
    // a second reply naming it does nothing (already released)
    c.send(3, reply(2, "again", id)).unwrap();
    assert!(c.send(2, TxBody::ReclaimMessage { message_id: id }).unwrap_err().contains("no escrow"));
}

#[test]
fn escrow_refund_timing() {
    let mut c = Chain::new(99);
    let id = c.next_msg_id(2);
    let b2 = c.bal(&pk(2));
    let fee0 = {
        let body = paid(3, "will you answer?", 7000, 10);
        let len = c.tx(2, 0, body.clone()).encode().len();
        social_fee_floor(&c.cfg(), len)
    };
    c.send_fee(2, fee0, paid(3, "will you answer?", 7000, 10)).unwrap();
    let e = c.escrow(&id).unwrap();
    // advance to the deadline: still open at height == deadline
    while c.height + 1 < e.deadline {
        c.block(&[]).unwrap();
    }
    assert_eq!(c.height + 1, e.deadline);
    let rf = c.tx(2, 1000, TxBody::ReclaimMessage { message_id: id });
    assert!(c.block(&[rf]).unwrap_err().contains("window is open"), "height == deadline is still inside the window");
    // let the block at the deadline height pass
    c.block(&[]).unwrap();
    assert_eq!(c.height, e.deadline);
    // a late reply (after the deadline) does not release it
    c.send(3, reply(2, "late answer", id)).unwrap();
    assert!(c.escrow(&id).is_some(), "a reply after the deadline releases nothing");
    // the sender reclaims: refunded in full
    let before = c.bal(&pk(2));
    c.send_fee(2, 1000, TxBody::ReclaimMessage { message_id: id }).unwrap();
    assert_eq!(c.bal(&pk(2)), before - 1000 + 7000);
    assert_eq!(c.bal(&pk(2)), b2 - fee0 - 1000);
    let s = c.stats(&pk(2));
    assert_eq!((s.open_escrows, s.locked, s.refunded), (0, 0, 1));
    assert!(c.escrow(&id).is_none());
}

#[test]
fn recipient_declines_early_and_limits_hold() {
    let mut c = Chain::new(99);
    let id = c.next_msg_id(2);
    c.send(2, paid(3, "a pitch", 9000, 50)).unwrap();
    let b2 = c.bal(&pk(2));
    c.send(3, TxBody::ReclaimMessage { message_id: id }).unwrap();
    assert_eq!(c.bal(&pk(2)), b2 + 9000, "declined: refunded at once");
    // window bounds and shape
    assert!(c.send(2, paid(3, "x", 0, 20)).unwrap_err().contains("needs pay"));
    assert!(c.send(2, paid(3, "x", 1, 9)).unwrap_err().contains("10..=10080"));
    assert!(c.send(2, paid(3, "x", 1, 10_081)).unwrap_err().contains("10..=10080"));
    // open escrow cap per sender
    c.admin(dk::DM_MAX_OPEN_ESCROWS, 2u32.to_le_bytes().to_vec()).unwrap();
    c.send(2, paid(3, "one", 1, 10)).unwrap();
    c.send(2, paid(4, "two", 1, 10)).unwrap();
    assert!(c.send(2, paid(5, "three", 1, 10)).unwrap_err().contains("at most 2 open escrows"));
    // a sender cannot escrow what it does not hold
    let all = c.bal(&pk(2));
    assert!(c.send(2, paid(6, "everything", all, 0)).is_err());
}

#[test]
fn protocol_share_goes_to_the_pool() {
    let mut c = Chain::new(99);
    c.admin(dk::DM_PROTOCOL_BPS, 1000u16.to_le_bytes().to_vec()).unwrap();
    let pool0 = c.bal(&pool_address());
    let b3 = c.bal(&pk(3));
    c.send(2, paid(3, "tip", 10_000, 0)).unwrap();
    assert_eq!(c.bal(&pk(3)), b3 + 9_000);
    assert!(c.bal(&pool_address()) >= pool0 + 1_000);
    assert_eq!(c.stats(&pk(3)).pay_in, 9_000);
}

#[test]
fn sealed_dms_need_an_enc_key_and_framing() {
    let mut c = Chain::new(99);
    let sealed = |to: u8, body: Vec<u8>| TxBody::SendMessage(SendMessage { to: pk(to), mode: dm::MODE_SEALED, body, ..Default::default() });
    let e = c.send(2, sealed(3, sealed_body(40))).unwrap_err();
    assert!(e.contains("no enc_key"), "{e}");
    c.profile(3, dm::POLICY_OPEN, 0, 0, [0x42; 32]);
    c.send(2, sealed(3, sealed_body(40))).unwrap();
    let mut bad_v = sealed_body(40);
    bad_v[0] = 9;
    assert!(c.send(2, sealed(3, bad_v)).unwrap_err().contains("version 1"));
    let mut trailing = sealed_body(40);
    trailing.push(0);
    assert!(c.send(2, sealed(3, trailing)).unwrap_err().contains("after the last"));
    let mut short_tag = sealed_body(20);
    short_tag[1] = 2;
    short_tag.extend([7u8; 56]);
    short_tag.extend(15u16.to_le_bytes());
    short_tag.extend([9u8; 15]);
    assert!(c.send(2, sealed(3, short_tag)).unwrap_err().contains("tag"));
    assert!(c.send(2, sealed(3, sealed_body(15))).unwrap_err().contains("too short"));
    assert!(c.send(2, sealed(3, vec![1, 1, 0])).unwrap_err().contains("too short"));
    assert!(c.send(2, TxBody::SendMessage(SendMessage { to: pk(3), mode: 2, body: b"x".to_vec(), ..Default::default() })).unwrap_err().contains("mode"));
}

#[test]
fn channels_paid_invite_and_open_joins() {
    let mut c = Chain::new(99);
    c.admin(dk::CHANNEL_CREATE_FEE, 500u64.to_le_bytes().to_vec()).unwrap();
    let cfg = |policy: u8, price: u64, mode: u8, admins: Vec<Pubkey>| ChannelConfig {
        channel_id: [0; 32], name: b"Market desk".to_vec(), about: b"bids".to_vec(), policy, join_price: price, mode, admins,
    };
    // shapes
    assert!(c.send(2, TxBody::ChannelConfig(cfg(2, 0, 0, vec![]))).unwrap_err().contains("join_price"));
    assert!(c.send(2, TxBody::ChannelConfig(cfg(0, 5, 0, vec![]))).unwrap_err().contains("join_price"));
    assert!(c.send(2, TxBody::ChannelConfig(cfg(0, 0, 0, vec![pk(2)]))).unwrap_err().contains("other accounts"));
    assert!(c.send(2, TxBody::ChannelConfig(cfg(0, 0, 0, vec![pk(3), pk(3)]))).unwrap_err().contains("duplicate"));
    let pool0 = c.bal(&pool_address());
    let paid_ch = c.next_channel_id(2);
    c.send(2, TxBody::ChannelConfig(cfg(dm::CHANNEL_PAID, 3000, 0, vec![pk(3)]))).unwrap();
    assert!(c.bal(&pool_address()) >= pool0 + 500, "creation fee to the pool");
    let ch = c.channel(&paid_ch);
    assert_eq!((ch.creator, ch.members, ch.admins.clone()), (pk(2), 2, vec![pk(3)]), "creator and named admin are members");
    // a paid join pays the creator
    let b2 = c.bal(&pk(2));
    assert!(c.send(4, TxBody::ChannelMember { channel: paid_ch, action: 0, member: [0; 32], pay: 2999 }).unwrap_err().contains("costs 3000"));
    c.send(4, TxBody::ChannelMember { channel: paid_ch, action: 0, member: [0; 32], pay: 3000 }).unwrap();
    assert_eq!(c.bal(&pk(2)), b2 + 3000);
    assert!(c.send(4, TxBody::ChannelMember { channel: paid_ch, action: 0, member: [0; 32], pay: 3000 }).unwrap_err().contains("already a member"));
    // members post, others do not; plain channels take plain messages without pay
    let msg = |ch: Hash, mode: u8, body: Vec<u8>| TxBody::SendMessage(SendMessage { channel: ch, mode, body, ..Default::default() });
    c.send(4, msg(paid_ch, 0, b"bid 40".to_vec())).unwrap();
    assert!(c.send(5, msg(paid_ch, 0, b"let me in".to_vec())).unwrap_err().contains("only members"));
    assert!(c.send(4, TxBody::SendMessage(SendMessage { channel: paid_ch, body: b"x".to_vec(), pay: 1, ..Default::default() })).unwrap_err().contains("no pay"));
    assert!(c.send(4, msg(paid_ch, 1, sealed_chan_body(1))).unwrap_err().contains("plain"));
    assert_eq!(c.channel(&paid_ch).messages, 1);
    // invite only: refused without an invite, invited join is free, invites need an admin
    let inv_ch = c.next_channel_id(2);
    c.send(2, TxBody::ChannelConfig(cfg(dm::CHANNEL_INVITE, 0, 0, vec![]))).unwrap();
    assert!(c.send(5, TxBody::ChannelMember { channel: inv_ch, action: 0, member: [0; 32], pay: 0 }).unwrap_err().contains("invite only"));
    assert!(c.send(6, TxBody::ChannelMember { channel: inv_ch, action: 2, member: pk(5), pay: 0 }).unwrap_err().contains("creator or admins"));
    c.send(2, TxBody::ChannelMember { channel: inv_ch, action: 2, member: pk(5), pay: 0 }).unwrap();
    assert!(c.send(2, TxBody::ChannelMember { channel: inv_ch, action: 2, member: pk(5), pay: 0 }).unwrap_err().contains("already invited"));
    c.send(5, TxBody::ChannelMember { channel: inv_ch, action: 0, member: [0; 32], pay: 0 }).unwrap();
    assert!(c.get(&sd::invite_key(&inv_ch, &pk(5))).is_none(), "the invite is used up");
    // leave and remove; the creator stays
    c.send(5, TxBody::ChannelMember { channel: inv_ch, action: 1, member: [0; 32], pay: 0 }).unwrap();
    assert!(c.send(5, msg(inv_ch, 0, b"gone".to_vec())).unwrap_err().contains("only members"));
    assert!(c.send(2, TxBody::ChannelMember { channel: inv_ch, action: 1, member: [0; 32], pay: 0 }).unwrap_err().contains("creator cannot"));
    // updates: admins change name; only the creator changes admins; mode is fixed
    let mut up = cfg(dm::CHANNEL_PAID, 4000, 0, vec![pk(3)]);
    up.channel_id = paid_ch;
    up.name = b"Market desk II".to_vec();
    c.send(3, TxBody::ChannelConfig(up.clone())).unwrap();
    assert_eq!(c.channel(&paid_ch).name, b"Market desk II".to_vec());
    let mut up2 = up.clone();
    up2.admins = vec![pk(3), pk(4)];
    assert!(c.send(3, TxBody::ChannelConfig(up2.clone())).unwrap_err().contains("only the creator"));
    c.send(2, TxBody::ChannelConfig(up2.clone())).unwrap();
    let mut up3 = up2.clone();
    up3.admins = vec![pk(3), pk(4), pk(6)];
    assert!(c.send(2, TxBody::ChannelConfig(up3)).unwrap_err().contains("member first"));
    let mut up4 = up2.clone();
    up4.mode = 1;
    assert!(c.send(2, TxBody::ChannelConfig(up4)).unwrap_err().contains("fixed"));
    assert!(c.send(7, TxBody::ChannelConfig(up2)).unwrap_err().contains("creator or admins"));
    // an admin cannot remove another admin; the creator can
    assert!(c.send(3, TxBody::ChannelMember { channel: paid_ch, action: 3, member: pk(4), pay: 0 }).unwrap_err().contains("only the creator removes an admin"));
    c.send(2, TxBody::ChannelMember { channel: paid_ch, action: 3, member: pk(4), pay: 0 }).unwrap();
    let ch = c.channel(&paid_ch);
    assert_eq!((ch.members, ch.admins.clone()), (2, vec![pk(3)]));
    // the channel list
    assert_eq!(sd::decode_u64(schema::tag::CHANNEL_COUNT, &c.get(&sd::channel_count_key()).unwrap()).unwrap(), 2);
}

#[test]
fn sealed_channel_key_epochs_and_removal_rotates() {
    let mut c = Chain::new(99);
    for who in [2u8, 3, 4, 5] {
        c.profile(who, dm::POLICY_OPEN, 0, 0, [who + 0x40; 32]);
    }
    c.profile(6, dm::POLICY_OPEN, 0, 0, [0; 32]);
    c.admin(dk::CHANNEL_SEALED_MAX_MEMBERS, 4u32.to_le_bytes().to_vec()).unwrap();
    let ch = c.next_channel_id(2);
    c.send(2, TxBody::ChannelConfig(ChannelConfig { name: b"Court".to_vec(), policy: dm::CHANNEL_OPEN, mode: dm::MODE_SEALED, ..Default::default() })).unwrap();
    for who in [3u8, 4] {
        c.send(who, TxBody::ChannelMember { channel: ch, action: 0, member: [0; 32], pay: 0 }).unwrap();
    }
    let smsg = |epoch: u32| TxBody::SendMessage(SendMessage { channel: ch, mode: 1, body: sealed_chan_body(epoch), ..Default::default() });
    // no key yet
    assert!(c.send(3, smsg(1)).unwrap_err().contains("no channel key yet"));
    let env = |m: u8| KeyEnvelope { member: pk(m), envelope: [m; 104] };
    // only an admin member posts keys; epochs go 1, then 1 (more) or 2
    assert!(c.send(3, TxBody::ChannelKey { channel: ch, epoch: 1, envelopes: vec![env(3)] }).unwrap_err().contains("admin"));
    assert!(c.send(2, TxBody::ChannelKey { channel: ch, epoch: 2, envelopes: vec![env(3)] }).unwrap_err().contains("epoch must be"));
    assert!(c.send(2, TxBody::ChannelKey { channel: ch, epoch: 1, envelopes: vec![env(5)] }).unwrap_err().contains("not a member"));
    assert!(c.send(2, TxBody::ChannelKey { channel: ch, epoch: 1, envelopes: vec![env(3), env(3)] }).unwrap_err().contains("twice"));
    c.send(2, TxBody::ChannelKey { channel: ch, epoch: 1, envelopes: vec![env(2), env(3)] }).unwrap();
    c.send(2, TxBody::ChannelKey { channel: ch, epoch: 1, envelopes: vec![env(4)] }).unwrap();
    assert_eq!(c.channel(&ch).epoch, 1);
    c.send(3, smsg(1)).unwrap();
    assert!(c.send(3, smsg(2)).unwrap_err().contains("epoch 2"));
    let mut bad = sealed_chan_body(1);
    bad[0] = 1;
    assert!(c.send(3, TxBody::SendMessage(SendMessage { channel: ch, mode: 1, body: bad, ..Default::default() })).unwrap_err().contains("version 2"));
    // the member cap of sealed channels
    c.send(5, TxBody::ChannelMember { channel: ch, action: 0, member: [0; 32], pay: 0 }).unwrap();
    assert!(c.send(6, TxBody::ChannelMember { channel: ch, action: 0, member: [0; 32], pay: 0 }).unwrap_err().contains("at most 4"));
    // a member without an enc_key gets no envelope
    c.send(5, TxBody::ChannelMember { channel: ch, action: 1, member: [0; 32], pay: 0 }).unwrap();
    assert!(c.channel(&ch).rekey_due, "a leave forces a new epoch");
    assert!(c.send(3, smsg(1)).unwrap_err().contains("new key epoch"));
    c.send(6, TxBody::ChannelMember { channel: ch, action: 0, member: [0; 32], pay: 0 }).unwrap();
    assert!(c.send(2, TxBody::ChannelKey { channel: ch, epoch: 2, envelopes: vec![env(6)] }).unwrap_err().contains("no enc_key"));
    // removing a member: messages wait for epoch 2, then flow under it
    c.send(2, TxBody::ChannelMember { channel: ch, action: 3, member: pk(6), pay: 0 }).unwrap();
    c.send(2, TxBody::ChannelMember { channel: ch, action: 3, member: pk(4), pay: 0 }).unwrap();
    assert!(c.send(4, smsg(1)).unwrap_err().contains("only members"), "the removed member is out");
    c.send(2, TxBody::ChannelKey { channel: ch, epoch: 2, envelopes: vec![env(2), env(3)] }).unwrap();
    let chn = c.channel(&ch);
    assert_eq!((chn.epoch, chn.rekey_due, chn.members), (2, false, 2));
    assert!(c.send(3, smsg(1)).unwrap_err().contains("epoch 2"));
    c.send(3, smsg(2)).unwrap();
    // keys are for sealed channels only
    let plain_ch = c.next_channel_id(2);
    c.send(2, TxBody::ChannelConfig(ChannelConfig { name: b"Square".to_vec(), ..Default::default() })).unwrap();
    assert!(c.send(2, TxBody::ChannelKey { channel: plain_ch, epoch: 1, envelopes: vec![env(2)] }).unwrap_err().contains("sealed channels"));
}

#[test]
fn many_dms_keep_supply_and_counters_exact() {
    // a random-ish walk over escrows, releases and reclaims; supply is checked every block
    let mut c = Chain::new(99);
    let mut open: Vec<(Hash, u8, u8)> = Vec::new();
    for i in 0..60u64 {
        let from = 2 + (i % 5) as u8;
        let to = 2 + ((i * 7 + 3) % 5) as u8;
        if from == to {
            continue;
        }
        match i % 4 {
            0 | 1 => {
                let id = c.next_msg_id(from);
                c.send(from, paid(to, "q", 100 + i, 10)).unwrap();
                open.push((id, from, to));
            }
            2 => {
                if let Some((id, f, t)) = open.pop() {
                    c.send(t, reply(f, "a", id)).unwrap();
                }
            }
            _ => {
                c.block(&[]).unwrap();
            }
        }
    }
    let total_open: u64 = (2..=6u8).map(|w| c.stats(&pk(w)).open_escrows).sum();
    let live = open.iter().filter(|(id, _, _)| c.escrow(id).is_some()).count() as u64;
    assert_eq!(total_open, live);
}
