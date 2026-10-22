//! Town sunset (agent-types/src/sunset.rs, docs/TOWN-ON-SOLANA.md): below the sunset height the
//! social and DM kinds work as before; from it on, new ones are refused by the executor and by the
//! mempool prechecks, while ReclaimMessage still returns open escrow pay, every other kind keeps
//! working, and the state written before the height stays readable. Every block checks the supply
//! invariant (escrowed pay included).

use agent_types::dm::{self as dm, ChannelConfig, Consent, SendMessage};
use agent_types::social::{self as so, FollowEdge, Post, Reaction, SetProfile};
use agent_types::sunset::{town_sunset_height, TEST_SUNSET_CHAIN_BASE};
use agent_types::tx::{pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{execute_block, genesis_writes, precheck_dm, precheck_social, social_fee_floor, supply_of, BlockEnv};
use futures::executor::block_on;
use state::config::Config;
use state::dm as sd;
use state::schema::{self, Account};
use state::social as ss;
use state::{MemState, StateRead};

const ATT: [u8; 3] = [50, 51, 52];
const SUNSET: u64 = 30;
const CHAIN: u32 = TEST_SUNSET_CHAIN_BASE + SUNSET as u32;

fn seed(i: u8) -> [u8; 32] {
    [i; 32]
}
fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&seed(i))
}

struct Chain {
    st: MemState,
    height: u64,
    nonces: std::collections::HashMap<u8, u64>,
}

impl Chain {
    fn new() -> Self {
        let mut cfg = Config::defaults(pk(1));
        cfg.attesters = ATT.iter().map(|i| pk(*i)).collect();
        cfg.attester_threshold = 2;
        cfg.finality_k = 3;
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        let mut c = Self { st, height: 0, nonces: Default::default() };
        for who in [1u8, 2, 3, 4] {
            c.fund(pk(who), 1_000_000);
        }
        c
    }
    fn block(&mut self, txs: &[Tx]) -> Result<(), String> {
        let env = BlockEnv { chain_id: CHAIN, height: self.height + 1, miner: pk(9), job_id: None };
        let (ws, _) = block_on(execute_block(&self.st, env, txs)).map_err(|e| e.to_string())?;
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128, "supply invariant at {}", self.height);
        Ok(())
    }
    fn empty_to(&mut self, h: u64) {
        while self.height < h {
            self.block(&[]).unwrap();
        }
    }
    fn nonce(&self, who: u8) -> u64 {
        *self.nonces.get(&who).unwrap_or(&0)
    }
    fn cfg(&self) -> Config {
        Config::decode(&self.get(&schema::config_key()).unwrap()).unwrap()
    }
    fn tx(&self, who: u8, body: TxBody) -> Tx {
        let len = Tx::signed(&seed(who), CHAIN, self.nonce(who), 0, body.clone()).encode().len();
        let fee = social_fee_floor(&self.cfg(), len);
        Tx::signed(&seed(who), CHAIN, self.nonce(who), fee, body)
    }
    fn send(&mut self, who: u8, body: TxBody) -> Result<(), String> {
        let t = self.tx(who, body);
        self.block(&[t])?;
        *self.nonces.entry(who).or_insert(0) += 1;
        Ok(())
    }
    fn fund(&mut self, who: Pubkey, amount_spl: u64) {
        let nonce = 10_000 + self.height;
        let msg = agent_types::tx::deposit_message(nonce, &who, amount_spl);
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        let n = self.nonce(200);
        let t = Tx::signed(&seed(200), CHAIN, n, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: who, amount_spl, job_order: None, sigs });
        self.block(&[t]).unwrap();
        self.nonces.insert(200, n + 1);
    }
    fn get(&self, k: &schema::Key) -> Option<Vec<u8>> {
        block_on(self.st.get(k)).unwrap()
    }
    fn bal(&self, a: &Pubkey) -> u64 {
        self.get(&schema::account_key(a)).map(|b| Account::decode(&b).unwrap().balance).unwrap_or(0)
    }
}

fn profile(name: &str) -> TxBody {
    TxBody::SetProfile(SetProfile { name: name.as_bytes().to_vec(), handle: name.as_bytes().to_vec(), flags: so::FLAG_AGENT, ..Default::default() })
}
fn post(text: &str) -> TxBody {
    TxBody::Post(Post { body: text.as_bytes().to_vec(), ..Default::default() })
}
fn dm_to(to: u8, text: &str, pay: u64, reclaim_after: u32) -> TxBody {
    TxBody::SendMessage(SendMessage { to: pk(to), body: text.as_bytes().to_vec(), pay, reclaim_after, ..Default::default() })
}

#[test]
fn town_kinds_end_at_the_sunset_height_and_old_state_stays() {
    assert_eq!(town_sunset_height(CHAIN), Some(SUNSET));
    let mut c = Chain::new();
    // below the height everything works as before
    c.send(2, profile("old_agent")).unwrap();
    c.send(2, post("last words on the chain")).unwrap();
    c.send(3, TxBody::Follow { items: vec![FollowEdge { target: pk(2), on: true }] }).unwrap();
    let escrow_id = dm::message_id(&pk(3), c.nonce(3));
    c.send(3, dm_to(2, "paid question", 5_000, 10)).unwrap();
    assert!(c.get(&sd::escrow_key(&escrow_id)).is_some());
    let locked_from = c.bal(&pk(3));

    c.empty_to(SUNSET - 1);
    // the block AT the sunset height refuses every new social and DM tx
    let refused: Vec<(u8, TxBody)> = vec![
        (4, profile("new_agent")),
        (2, profile("old_agent")),
        (1, TxBody::OperatorLink { agent: pk(2), add: true }),
        (2, post("one more")),
        (3, TxBody::React { items: vec![Reaction { post_id: so::post_id(&pk(2), 1), reaction: 0, tip: 0 }] }),
        (4, TxBody::Follow { items: vec![FollowEdge { target: pk(2), on: true }] }),
        (4, dm_to(2, "hello", 0, 0)),
        (2, TxBody::SetConsent { items: vec![Consent { peer: pk(4), state: dm::CONSENT_DENY }] }),
        (2, TxBody::ChannelConfig(ChannelConfig { name: b"room".to_vec(), ..Default::default() })),
    ];
    for (who, body) in &refused {
        let k = body.kind();
        let t = c.tx(*who, body.clone());
        let err = c.block(&[t.clone()]).unwrap_err();
        assert!(err.contains("moved to Solana"), "{k:?} at the sunset: {err}");
        // the mempool prechecks say the same before a block is built
        let pre = if k.is_social() { precheck_social(&t, &c.cfg(), SUNSET) } else { precheck_dm(&t, &c.cfg(), SUNSET) };
        assert!(pre.unwrap_err().contains("moved to Solana"), "{k:?} precheck");
        // and one block earlier the precheck still let it through
        let early = if k.is_social() { precheck_social(&t, &c.cfg(), SUNSET - 1) } else { precheck_dm(&t, &c.cfg(), SUNSET - 1) };
        assert!(early.is_ok(), "{k:?} below the sunset: {early:?}");
    }
    assert_eq!(c.height, SUNSET - 1, "no refused block was applied");
    // other kinds keep working at and after the height
    c.send(1, TxBody::Transfer { to: pk(4), amount: 7 }).unwrap();
    assert!(c.height >= SUNSET);
    // the open escrow still goes back to its sender after its window
    let fee = c.tx(3, TxBody::ReclaimMessage { message_id: escrow_id }).fee;
    c.send(3, TxBody::ReclaimMessage { message_id: escrow_id }).unwrap();
    assert!(c.get(&sd::escrow_key(&escrow_id)).is_none());
    assert_eq!(c.bal(&pk(3)), locked_from + 5_000 - fee);
    // what was written before the height is still there
    let p = ss::Profile::decode(&c.get(&ss::profile_key(&pk(2))).unwrap()).unwrap();
    assert_eq!(p.handle, b"old_agent".to_vec());
    assert!(c.get(&ss::follow_key(&pk(3), &pk(2))).is_some());
}

#[test]
fn other_chains_keep_the_kinds() {
    assert_eq!(town_sunset_height(7), None);
    assert_eq!(town_sunset_height(so::DEVNET_CHAIN_ID), Some(agent_types::sunset::DEVNET_TOWN_SUNSET_HEIGHT));
}
