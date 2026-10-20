//! Social kinds (SPEC 4.6) and large balance tickets (ECOSYSTEM M1.5) in the executor:
//! activation height, the social fee rule, profiles and handles, house flags, posts,
//! reactions with tips, follows, atomic batches, and balance tickets above 4096 rows with
//! exact pro-rata pay and the alpha x P subsidy bound.

use agent_types::social::{self as so, social_upgrade_height, DEVNET_CHAIN_ID, DEVNET_SOCIAL_HEIGHT};
use agent_types::tx::{pubkey_from_seed, result_message, sign_plain};
use agent_types::*;
use executor::{execute_block, genesis_writes, social_fee_floor, supply_of, BlockEnv};
use futures::executor::block_on;
use pearl_job::standing;
use state::config::{keys, Config};
use state::schema::{self, Account, JobRecord};
use state::social::{self as ss, PostRef, Profile, SocialStats};
use state::{MemState, StateRead};

const ATT: [u8; 3] = [50, 51, 52];
const MINER: u8 = 9;
const ADMIN: u8 = 1;
const EVAL: u8 = 60;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}
fn blob_uri(id: &Hash) -> Vec<u8> {
    format!("blob:{}", hex::encode(id)).into_bytes()
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
        cfg.emission_ppm = 100_000;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        let mut c = Self { st, chain_id, height: 0, nonces: Default::default() };
        for who in [2u8, 3, 4, 5, 6, 7, ADMIN] {
            c.fund(pk(who), 1_000_000);
        }
        c
    }

    fn block(&mut self, job: Option<Hash>, txs: &[Tx]) -> Result<executor::BlockSummary, String> {
        let env = BlockEnv { chain_id: self.chain_id, height: self.height + 1, miner: pk(MINER), job_id: job };
        let (ws, sum) = block_on(execute_block(&self.st, env, txs)).map_err(|e| e.to_string())?;
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
        Ok(sum)
    }

    fn tx(&self, who: u8, fee: u64, body: TxBody) -> Tx {
        Tx::signed(&seed(who), self.chain_id, *self.nonces.get(&who).unwrap_or(&0), fee, body)
    }

    /// One tx in its own block with fee `fee`; the nonce advances only when it lands.
    fn send_fee(&mut self, who: u8, fee: u64, body: TxBody) -> Result<executor::BlockSummary, String> {
        let t = self.tx(who, fee, body);
        let r = self.block(None, &[t]);
        if r.is_ok() {
            *self.nonces.entry(who).or_insert(0) += 1;
        }
        r
    }

    fn send(&mut self, who: u8, body: TxBody) -> Result<executor::BlockSummary, String> {
        let fee = self.floor(who, &body);
        self.send_fee(who, fee, body)
    }

    /// The social fee floor of `body` from `who` under the current config.
    fn floor(&self, who: u8, body: &TxBody) -> u64 {
        let len = self.tx(who, 0, body.clone()).encode().len();
        social_fee_floor(&self.cfg(), len)
    }

    fn cfg(&self) -> Config {
        Config::decode(&self.get(&schema::config_key()).unwrap()).unwrap()
    }

    fn fund(&mut self, who: Pubkey, amount_spl: u64) {
        let nonce = 10_000 + self.height;
        let msg = agent_types::tx::deposit_message(nonce, &who, amount_spl);
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        let n = *self.nonces.get(&200).unwrap_or(&0);
        let t = Tx::signed(&seed(200), self.chain_id, n, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: who, amount_spl, job_order: None, sigs });
        self.block(None, &[t]).unwrap();
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
    fn profile(&self, a: &Pubkey) -> Option<Profile> {
        self.get(&ss::profile_key(a)).map(|b| Profile::decode(&b).unwrap())
    }
    fn stats(&self, a: &Pubkey) -> SocialStats {
        self.get(&ss::social_key(a)).map(|b| SocialStats::decode(&b).unwrap()).unwrap_or_default()
    }
    fn handle_owner(&self, h: &str) -> Option<Pubkey> {
        self.get(&ss::handle_key(h.as_bytes())).map(|b| ss::decode_key32(schema::tag::HANDLE, &b).unwrap())
    }
    fn job(&self, id: &Hash) -> JobRecord {
        JobRecord::decode(&self.get(&schema::job_key(id)).unwrap()).unwrap()
    }
    /// The id the next Post from `who` gets.
    fn next_post_id(&self, who: u8) -> Hash {
        so::post_id(&pk(who), *self.nonces.get(&who).unwrap_or(&0))
    }
}

fn profile(handle: &str, name: &str) -> SetProfile {
    SetProfile { handle: handle.as_bytes().to_vec(), name: name.as_bytes().to_vec(), flags: so::FLAG_AGENT, ..Default::default() }
}

fn post(kind: u8, parent: Hash, body: &str) -> TxBody {
    TxBody::Post(Post { kind, parent, body: body.as_bytes().to_vec(), ..Default::default() })
}

#[test]
fn activation_height_gates_social_kinds_and_new_config_keys() {
    assert_eq!(social_upgrade_height(99), 0);
    let mut c = Chain::new(DEVNET_CHAIN_ID);
    // jump to just below the devnet activation height (empty blocks are cheap in MemState)
    c.height = DEVNET_SOCIAL_HEIGHT - 2;
    let e = c.send(2, TxBody::SetProfile(profile("early_bird", "Early"))).unwrap_err();
    assert!(e.contains("activate at height"), "{e}");
    let e = c.admin(keys::SOCIAL_BYTE_FEE, 1u64.to_le_bytes().to_vec()).unwrap_err();
    assert!(e.contains("activates at height"), "{e}");
    // old keys keep working below the height, and the config record keeps its old bytes
    let before = c.get(&schema::config_key()).unwrap();
    c.admin(keys::MIN_FEE, 1000u64.to_le_bytes().to_vec()).unwrap();
    assert_eq!(c.get(&schema::config_key()).unwrap(), before, "same 19-value encoding as an old node writes");
    assert_eq!(c.height, DEVNET_SOCIAL_HEIGHT - 1);
    // from the height: both valid
    c.send(2, TxBody::SetProfile(profile("early_bird", "Early"))).unwrap();
    c.admin(keys::SOCIAL_BYTE_FEE, 1u64.to_le_bytes().to_vec()).unwrap();
    assert!(c.get(&schema::config_key()).unwrap().len() > before.len());
    // a block mixing a social tx below the height is refused whole (all-or-nothing)
    let mut d = Chain::new(DEVNET_CHAIN_ID);
    d.height = DEVNET_SOCIAL_HEIGHT - 10;
    let t1 = d.tx(2, 1000, TxBody::Transfer { to: pk(3), amount: 5 });
    let t2 = d.tx(3, 1000, post(so::POST, [0; 32], "too soon"));
    assert!(d.block(None, &[t1, t2]).unwrap_err().contains("tx 1"));
}

#[test]
fn social_fee_rule_and_spam_limits() {
    let mut c = Chain::new(99);
    let body = post(so::POST, [0; 32], "hello town");
    // defaults: floor = social_min_fee 1000 = min_fee
    assert_eq!(c.floor(2, &body), 1000);
    assert!(c.send_fee(2, 999, body.clone()).unwrap_err().contains("below min_fee"));
    c.send_fee(2, 1000, body.clone()).unwrap();
    // a per-byte fee: the floor grows with the tx size
    c.admin(keys::SOCIAL_BYTE_FEE, 10u64.to_le_bytes().to_vec()).unwrap();
    c.admin(keys::SOCIAL_MIN_FEE, 500u64.to_le_bytes().to_vec()).unwrap();
    let small = post(so::POST, [0; 32], "hi");
    let big = post(so::POST, [0; 32], &"x".repeat(900));
    let (fs, fb) = (c.floor(2, &small), c.floor(2, &big));
    let len = c.tx(2, 0, big.clone()).encode().len() as u64;
    assert_eq!(fb, 500 + 10 * len);
    assert_eq!(fb - fs, 10 * 898, "898 more body bytes");
    let e = c.send_fee(2, fb - 1, big.clone()).unwrap_err();
    assert!(e.contains("social fee"), "{e}");
    let bal = c.bal(&pk(2));
    c.send_fee(2, fb, big).unwrap();
    assert_eq!(c.bal(&pk(2)), bal - fb, "the fee is all a post costs");
    // the floor never drops below min_fee
    c.admin(keys::SOCIAL_BYTE_FEE, 0u64.to_le_bytes().to_vec()).unwrap();
    c.admin(keys::SOCIAL_MIN_FEE, 0u64.to_le_bytes().to_vec()).unwrap();
    assert_eq!(c.floor(2, &small), 1000);
    // body cap: post_inline_max (default 1024), admin-editable
    assert!(c.send(2, post(so::POST, [0; 32], &"y".repeat(1025))).unwrap_err().contains("post_inline_max"));
    c.admin(keys::POST_INLINE_MAX, 2048u32.to_le_bytes().to_vec()).unwrap();
    c.send(2, post(so::POST, [0; 32], &"y".repeat(1025))).unwrap();
    assert!(c.admin(keys::POST_INLINE_MAX, 9000u32.to_le_bytes().to_vec()).unwrap_err().contains("post_inline_max"));
    // batch caps are codec rules: 0 or 65 entries do not decode
    let mut w = agent_types::codec::W::new();
    agent_types::social::write_follows(&mut w, &vec![FollowEdge { target: pk(3), on: true }; 65]);
    let b = w.into_inner();
    assert!(agent_types::social::read_follows(&mut agent_types::codec::R::new(&b)).is_err());
    assert!(agent_types::social::read_follows(&mut agent_types::codec::R::new(&[0])).is_err());
}

#[test]
fn profiles_handles_and_house_flags() {
    let mut c = Chain::new(99);
    c.send(2, TxBody::SetProfile(profile("lamp_keeper", "Lamp Keeper"))).unwrap();
    let p = c.profile(&pk(2)).unwrap();
    assert_eq!((p.index, p.created_height), (0, c.height));
    assert_eq!(c.handle_owner("lamp_keeper"), Some(pk(2)));
    // first come: another account cannot take it
    assert!(c.send(3, TxBody::SetProfile(profile("lamp_keeper", "Thief"))).unwrap_err().contains("taken"));
    for bad in ["ab", "Lamp", "lamp-keeper", "a_very_long_handle_over_24c"] {
        assert!(c.send(3, TxBody::SetProfile(profile(bad, "x"))).unwrap_err().contains("handle"), "{bad}");
    }
    assert!(c.send(3, TxBody::SetProfile(profile("", ""))).unwrap_err().contains("name"));
    let mut flags = profile("ok_name", "x");
    flags.flags = 0x10;
    assert!(c.send(3, TxBody::SetProfile(flags)).unwrap_err().contains("flag"));
    // a new handle releases the old one
    let created = c.profile(&pk(2)).unwrap().created_height;
    c.send(2, TxBody::SetProfile(profile("lighthouse", "Lamp Keeper"))).unwrap();
    assert_eq!(c.handle_owner("lamp_keeper"), None);
    assert_eq!(c.handle_owner("lighthouse"), Some(pk(2)));
    let p = c.profile(&pk(2)).unwrap();
    assert_eq!((p.index, p.created_height), (0, created), "an update keeps the index and creation height");
    c.send(3, TxBody::SetProfile(profile("lamp_keeper", "Second"))).unwrap();
    assert_eq!(c.profile(&pk(3)).unwrap().index, 1);
    // blob URI: profile_hash must be the blob id
    let id = [0x5au8; 32];
    let mut b = profile("blobby", "Blobby");
    b.profile_uri = blob_uri(&id);
    b.profile_hash = [0x5b; 32];
    assert!(c.send(4, TxBody::SetProfile(b.clone())).unwrap_err().contains("blob id"));
    b.profile_hash = id;
    c.send(4, TxBody::SetProfile(b)).unwrap();
    // house: needs a house operator AND the operator's link
    let mut h = profile("house_poet", "Poet");
    h.flags |= so::FLAG_HOUSE;
    h.operator = pk(7);
    assert!(c.send(5, TxBody::SetProfile(h.clone())).unwrap_err().contains("house_operators"));
    c.admin(keys::HOUSE_OPERATORS, [&[1u8][..], &pk(7)].concat()).unwrap();
    assert!(c.send(5, TxBody::SetProfile(h.clone())).unwrap_err().contains("OperatorLink"));
    c.send(7, TxBody::OperatorLink { agent: pk(5), add: true }).unwrap();
    assert!(c.send(7, TxBody::OperatorLink { agent: pk(5), add: true }).unwrap_err().contains("exists"));
    c.send(5, TxBody::SetProfile(h)).unwrap();
    assert_ne!(c.profile(&pk(5)).unwrap().flags & so::FLAG_HOUSE, 0);
    // dropping the link drops the house flag
    c.send(7, TxBody::OperatorLink { agent: pk(5), add: false }).unwrap();
    assert_eq!(c.profile(&pk(5)).unwrap().flags & so::FLAG_HOUSE, 0);
    assert!(c.send(7, TxBody::OperatorLink { agent: pk(5), add: false }).unwrap_err().contains("no such link"));
}

#[test]
fn posts_reactions_tips_and_follows() {
    let mut c = Chain::new(99);
    let p0 = c.next_post_id(2);
    c.send(2, TxBody::Post(Post { kind: so::POST, body: b"The fog came in.".to_vec(), thought_ref: b"req_0123abcd".to_vec(), ..Default::default() })).unwrap();
    let r = PostRef::decode(&c.get(&ss::post_key(&p0)).unwrap()).unwrap();
    assert_eq!((r.author, r.height, r.kind), (pk(2), c.height, so::POST));
    // shape rules
    assert!(c.send(3, post(so::REPLY, [9; 32], "to nothing")).unwrap_err().contains("parent"));
    assert!(c.send(3, post(so::POST, p0, "a post with a parent")).unwrap_err().contains("no parent"));
    assert!(c.send(3, post(7, [0; 32], "x")).unwrap_err().contains("kind"));
    assert!(c.send(3, post(so::POST, [0; 32], "")).unwrap_err().contains("empty"));
    assert!(c.send(3, TxBody::Post(Post { body: vec![0xff, 0xfe], ..Default::default() })).unwrap_err().contains("UTF-8"));
    assert!(c.send(3, TxBody::Post(Post { body: b"x".to_vec(), channel: [1; 32], ..Default::default() })).unwrap_err().contains("channel"));
    assert!(c.send(3, TxBody::Post(Post { body: b"x".to_vec(), thought_ref: b"req 1".to_vec(), ..Default::default() })).unwrap_err().contains("thought_ref"));
    assert!(c.send(3, TxBody::Post(Post { body: b"x".to_vec(), blob_uri: blob_uri(&[1; 32]), blob_hash: [2; 32], ..Default::default() })).unwrap_err().contains("blob"));
    c.send(3, TxBody::Post(Post { body: b"with a blob".to_vec(), blob_uri: blob_uri(&[1; 32]), blob_hash: [1; 32], ..Default::default() })).unwrap();
    let p1 = c.next_post_id(3);
    c.send(3, post(so::REPLY, p0, "Saw it from the harbour.")).unwrap();
    c.send(4, TxBody::Post(Post { kind: so::REPOST, parent: p1, ..Default::default() })).unwrap();
    assert_eq!((c.stats(&pk(2)).posts, c.stats(&pk(3)).posts, c.stats(&pk(3)).replies, c.stats(&pk(4)).replies), (1, 1, 1, 1));
    // reactions + a tip, batched; a tip moves CADRE to the author
    let (a0, r0) = (c.bal(&pk(2)), c.bal(&pk(4)));
    let body = TxBody::React { items: vec![Reaction { post_id: p0, reaction: 0, tip: 0 }, Reaction { post_id: p0, reaction: so::REACTION_TIP, tip: 250_000 }] };
    let fee = c.floor(4, &body);
    c.send(4, body).unwrap();
    assert_eq!(c.bal(&pk(2)), a0 + 250_000);
    assert_eq!(c.bal(&pk(4)), r0 - 250_000 - fee);
    let (sa, sr) = (c.stats(&pk(2)), c.stats(&pk(4)));
    assert_eq!((sa.reactions_received, sa.tips_received, sr.reactions_given, sr.tips_sent), (2, 250_000, 2, 250_000));
    // repeats, own-post tips, tip reaction without tip, unknown post: refused, and atomically
    let bal = c.bal(&pk(4));
    let e = c.send(4, TxBody::React { items: vec![Reaction { post_id: p1, reaction: 2, tip: 10 }, Reaction { post_id: p0, reaction: 0, tip: 0 }] }).unwrap_err();
    assert!(e.contains("already"), "{e}");
    assert_eq!(c.bal(&pk(4)), bal, "the first entry's tip was rolled back with the tx");
    assert_eq!(c.stats(&pk(3)).tips_received, 0);
    assert!(c.send(2, TxBody::React { items: vec![Reaction { post_id: p0, reaction: 1, tip: 5 }] }).unwrap_err().contains("own post"));
    assert!(c.send(2, TxBody::React { items: vec![Reaction { post_id: p1, reaction: so::REACTION_TIP, tip: 0 }] }).unwrap_err().contains("tip > 0"));
    assert!(c.send(2, TxBody::React { items: vec![Reaction { post_id: [3; 32], reaction: 0, tip: 0 }] }).unwrap_err().contains("unknown post"));
    assert!(c.send(2, TxBody::React { items: vec![Reaction { post_id: p1, reaction: 5, tip: 0 }] }).unwrap_err().contains("unknown reaction"));
    assert!(c.send(6, TxBody::React { items: vec![Reaction { post_id: p0, reaction: so::REACTION_TIP, tip: 10_000_000_000 }] }).unwrap_err().contains("insufficient"));
    // follows: batched, counters both ways, exact unfollow
    c.send(4, TxBody::Follow { items: vec![FollowEdge { target: pk(2), on: true }, FollowEdge { target: pk(3), on: true }] }).unwrap();
    c.send(5, TxBody::Follow { items: vec![FollowEdge { target: pk(2), on: true }] }).unwrap();
    assert_eq!((c.stats(&pk(2)).followers, c.stats(&pk(4)).following), (2, 2));
    assert!(c.send(4, TxBody::Follow { items: vec![FollowEdge { target: pk(2), on: true }] }).unwrap_err().contains("already following"));
    assert!(c.send(4, TxBody::Follow { items: vec![FollowEdge { target: pk(4), on: true }] }).unwrap_err().contains("yourself"));
    assert!(c.send(4, TxBody::Follow { items: vec![FollowEdge { target: pk(6), on: false }] }).unwrap_err().contains("not following"));
    assert!(c.send(4, TxBody::Follow { items: vec![FollowEdge { target: pool_address(), on: true }] }).unwrap_err().contains("followable"));
    c.send(4, TxBody::Follow { items: vec![FollowEdge { target: pk(2), on: false }] }).unwrap();
    assert_eq!((c.stats(&pk(2)).followers, c.stats(&pk(4)).following, c.stats(&pk(3)).followers), (1, 1, 1));
    assert!(c.get(&ss::follow_key(&pk(4), &pk(2))).is_none());
}

// ------------------------------------------------------------------ M1.5

fn register_body(name: &[u8]) -> RegisterModel {
    let cfg = pearl_job::MiningConfig::localnet_default();
    RegisterModel {
        name: name.to_vec(),
        manifest_uri: blob_uri(&[0x44; 32]),
        recipe: 1,
        rank: cfg.rank,
        rows_pattern: cfg.rows_pattern.to_bytes(),
        cols_pattern: cfg.cols_pattern.to_bytes(),
        matrices: vec![MatrixSpec { k: 2048, n: 3072, hash_b: [0x61; 32] }, MatrixSpec { k: 2048, n: 2048, hash_b: [0x62; 32] }],
    }
}

fn ticket(model_id: &Hash, m: u32, n: u32, server: Pubkey, w: u64, p: u64, deadline: u64, max_blocks: u32) -> PostJob {
    PostJob {
        m,
        n,
        k: 1,
        config: standing::ticket_config(model_id),
        hash_a: [0x5e; 32],
        hash_b: server,
        data_uri: blob_uri(&[0x51; 32]),
        worker_pay: w,
        protocol_fee: p,
        deadline_height: deadline,
        max_blocks,
        evaluators: vec![pk(EVAL)],
        threshold: 1,
    }
}

fn seg(matrix: u16, m: u32) -> Segment {
    let (k, n, hb) = if matrix == 1 { (2048, 2048, [0x62; 32]) } else { (2048, 3072, [0x61; 32]) };
    Segment { matrix, m, k, n, max_blocks: 1, hash_a: [matrix as u8 + 1; 32], hash_b: hb, c_hash: [matrix as u8 + 0x80; 32], a_uri: blob_uri(&[0x90 + matrix as u8; 32]) }
}

fn post_segments(ticket_id: Hash, round: u16, model: &Hash, segments: Vec<Segment>) -> TxBody {
    let cfg = pearl_job::MiningConfig::localnet_default();
    TxBody::PostSegments { ticket_id, round, model_id: *model, rank: cfg.rank, rows_pattern: cfg.rows_pattern.to_bytes(), cols_pattern: cfg.cols_pattern.to_bytes(), segments }
}

/// A balance ticket above 4096 rows needs `max_balance_rows`; only the balance form (n = 1)
/// gets it; a full-size round settles W and P pro rata exactly and the segment caps stay
/// under alpha x P x rows_used / R.
#[test]
fn large_balance_tickets_pay_pro_rata_and_keep_the_subsidy_bound() {
    let (registrant, buyer, server) = (2u8, 3u8, 4u8);
    let mut c = Chain::new(99);
    c.fund(pk(buyer), 100_000_000);
    c.fund(pool_address(), 1_000_000_000);
    c.send_fee(registrant, 1000, TxBody::RegisterModel(register_body(b"llama-big"))).unwrap();
    let model = standing::model_id(&pk(registrant), b"llama-big");
    let (w, p) = (64_000_000_000u64, 16_000_000_000u64);
    let big = 65_536u32;
    let d = c.height + 500;
    // default: 4096 is the most
    let e = c.send_fee(buyer, 1000, TxBody::PostJob(ticket(&model, big - 1, 1, pk(server), w, p, d, 200))).unwrap_err();
    assert!(e.contains("above 4096"), "{e}");
    assert!(c.admin(keys::MAX_BALANCE_ROWS, 2048u32.to_le_bytes().to_vec()).unwrap_err().contains("max_balance_rows"));
    c.admin(keys::MAX_BALANCE_ROWS, big.to_le_bytes().to_vec()).unwrap();
    // a one-request ticket (n > 1) keeps the 4096 bound
    let e = c.send_fee(buyer, 1000, TxBody::PostJob(ticket(&model, 8000, 100, pk(server), w, p, d, 200))).unwrap_err();
    assert!(e.contains("above 4096"), "{e}");
    assert!(c.send_fee(buyer, 1000, TxBody::PostJob(ticket(&model, big, 1, pk(server), w, p, d, 200))).unwrap_err().contains("above 65536"));
    let n = *c.nonces.get(&buyer).unwrap_or(&0);
    c.send_fee(buyer, 1000, TxBody::PostJob(ticket(&model, big - 1, 1, pk(server), w, p, d, 200))).unwrap();
    let tid = schema::job_id_for_post(&pk(buyer), n);
    let te = *c.job(&tid).ticket().unwrap();
    assert_eq!((te.rows, te.cap_total), (big as u64, p * 800_000 / 1_000_000));
    // three full rounds of 2044 rows (one 4 MiB A blob at k 2048) and one odd round
    let rounds = [2044u32, 2044, 2044, 1000];
    for (r, m) in rounds.iter().enumerate() {
        c.send_fee(server, 1000, post_segments(tid, r as u16, &model, vec![seg(0, *m), seg(1, *m)])).unwrap();
    }
    let used: u64 = rounds.iter().map(|m| *m as u64).sum();
    let te = *c.job(&tid).ticket().unwrap();
    assert_eq!((te.rounds, te.rows_used, te.segments), (4, used, 8));
    // subsidy bound: every segment cap, summed, stays under alpha x P x used / R
    let caps: u64 = (0..4u16).flat_map(|r| (0..2u16).map(move |m| (m, r))).map(|(m, r)| c.job(&standing::segment_id(&tid, m, r)).cap_remaining).sum();
    assert!(caps <= te.cap_total * used / big as u64, "{caps} > {}", te.cap_total * used / big as u64);
    assert!(caps + c.job(&tid).cap_remaining <= te.cap_total);
    // mine two segments: the subsidy each block pays stays under that segment's cap
    for (m, r) in [(0u16, 0u16), (1, 3)] {
        let sid = standing::segment_id(&tid, m, r);
        let cap = c.job(&sid).cap_remaining;
        let s = c.block(Some(sid), &[]).unwrap();
        assert!(s.subsidy <= cap && s.subsidy > 0);
    }
    // complete: exact pro rata in u128 (W x used / R to the server, the rest of W and
    // P x (R - used) / R back to the buyer)
    let rh = [0x77; 32];
    c.send_fee(server, 1000, TxBody::SubmitResult { job_id: tid, result_hash: rh, uri: blob_uri(&[0x52; 32]) }).unwrap();
    let msg = result_message(&tid, &pk(server), &rh);
    let (srv0, buy0) = (c.bal(&pk(server)), c.bal(&pk(buyer)));
    c.send_fee(server, 1000, TxBody::CompleteJob { job_id: tid, sigs: vec![AttesterSig { pubkey: pk(EVAL), sig: sign_plain(&seed(EVAL), &msg) }] }).unwrap();
    let pay = (w as u128 * used as u128 / big as u128) as u64;
    let p_back = (p as u128 * (big as u64 - used) as u128 / big as u128) as u64;
    assert_eq!(c.bal(&pk(server)), srv0 + pay - 1000);
    assert_eq!(c.bal(&pk(buyer)), buy0 + (w - pay) + p_back);
}
