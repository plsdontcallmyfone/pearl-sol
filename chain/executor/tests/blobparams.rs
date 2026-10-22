//! AdminConfig keys 48-49 (blob data-lifecycle parameters, SPEC 4.5 "Segment data retention"):
//! own record, devnet activation height, admin only, range checks, a default value deletes
//! the record (state equal to a chain where nobody set it), and the config record's encoding
//! never changes because of them.

use agent_types::tx::{pubkey_from_seed, sign_plain};
use agent_types::*;
use executor::{execute_block, genesis_writes, supply_of, BlockEnv};
use futures::executor::block_on;
use state::blobparams::{self as bp, BlobParams, DEVNET_BLOB_PARAMS_HEIGHT, DEVNET_CHAIN_ID};
use state::config::Config;
use state::schema;
use state::{MemState, StateRead};

const ATT: [u8; 3] = [50, 51, 52];
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
        let st = MemState::default();
        st.apply(&genesis_writes(&cfg).unwrap());
        let mut c = Self { st, chain_id, height: 0, nonces: Default::default() };
        for who in [2u8, ADMIN] {
            c.fund(pk(who), 1_000_000);
        }
        c
    }

    fn block(&mut self, txs: &[Tx]) -> Result<(), String> {
        let env = BlockEnv { chain_id: self.chain_id, height: self.height + 1, miner: pk(9), job_id: None };
        let (ws, _) = block_on(execute_block(&self.st, env, txs)).map_err(|e| e.to_string())?;
        self.st.apply(&ws);
        self.height += 1;
        assert_eq!(supply_of(&self.st.values()).unwrap(), TOTAL_SUPPLY as u128);
        Ok(())
    }

    fn fund(&mut self, who: Pubkey, amount_spl: u64) {
        let nonce = 10_000 + self.height;
        let msg = agent_types::tx::deposit_message(nonce, &who, amount_spl);
        let sigs = ATT[..2].iter().map(|i| AttesterSig { pubkey: pk(*i), sig: sign_plain(&seed(*i), &msg) }).collect();
        let n = *self.nonces.get(&200).unwrap_or(&0);
        let t = Tx::signed(&seed(200), self.chain_id, n, 0, TxBody::BridgeUnlock { solana_nonce: nonce, recipient: who, amount_spl, job_order: None, sigs });
        self.block(&[t]).unwrap();
        self.nonces.insert(200, n + 1);
    }

    fn admin_as(&mut self, who: u8, key: u8, value: Vec<u8>) -> Result<(), String> {
        let n = *self.nonces.get(&who).unwrap_or(&0);
        let t = Tx::signed(&seed(who), self.chain_id, n, 1000, TxBody::AdminConfig { key, value });
        self.block(&[t])?;
        self.nonces.insert(who, n + 1);
        Ok(())
    }

    fn params(&self) -> Option<BlobParams> {
        block_on(self.st.get(&bp::blob_params_key())).unwrap().map(|b| BlobParams::decode(&b).unwrap())
    }

    fn config_bytes(&self) -> Vec<u8> {
        block_on(self.st.get(&schema::config_key())).unwrap().unwrap()
    }
}

#[test]
fn devnet_activation_height_gates_keys_48_and_49() {
    let mut c = Chain::new(DEVNET_CHAIN_ID);
    c.height = DEVNET_BLOB_PARAMS_HEIGHT - 2;
    // the block at DEVNET_BLOB_PARAMS_HEIGHT - 1 refuses it as an older node does
    let e = c.admin_as(ADMIN, 48, 1440u32.to_le_bytes().to_vec()).unwrap_err();
    assert!(e.contains("unknown config key"), "{e}");
    assert_eq!(c.params(), None);
    c.height = DEVNET_BLOB_PARAMS_HEIGHT - 1;
    let cfg_before = c.config_bytes();
    c.admin_as(ADMIN, 48, 1440u32.to_le_bytes().to_vec()).unwrap();
    assert_eq!(c.params(), Some(BlobParams { segment_replay_blocks: 1440 }));
    assert_eq!(c.config_bytes(), cfg_before, "the config record is untouched");
}

#[test]
fn admin_only_range_checked_reserved_key_and_default_deletes_the_record() {
    let mut c = Chain::new(11);
    let e = c.admin_as(2, 48, 10u32.to_le_bytes().to_vec()).unwrap_err();
    assert!(e.contains("not the admin"), "{e}");
    let e = c.admin_as(ADMIN, 48, (bp::MAX_SEGMENT_REPLAY_BLOCKS + 1).to_le_bytes().to_vec()).unwrap_err();
    assert!(e.contains("segment_replay_blocks"), "{e}");
    let e = c.admin_as(ADMIN, 48, vec![1, 2]).unwrap_err();
    assert!(e.contains("u32"), "{e}");
    let e = c.admin_as(ADMIN, 49, 5u32.to_le_bytes().to_vec()).unwrap_err();
    assert!(e.contains("unknown config key"), "{e}");
    c.admin_as(ADMIN, 48, 0u32.to_le_bytes().to_vec()).unwrap();
    assert_eq!(c.params(), Some(BlobParams { segment_replay_blocks: 0 }));
    c.admin_as(ADMIN, 48, bp::DEFAULT_SEGMENT_REPLAY_BLOCKS.to_le_bytes().to_vec()).unwrap();
    assert_eq!(c.params(), None, "back to the default: no record, the same state as never set");
    // keys 45-47 stay unknown here (another row's speed tier may claim 45)
    for k in [45u8, 46, 47, 50] {
        assert!(c.admin_as(ADMIN, k, vec![0; 8]).unwrap_err().contains("unknown config key"), "key {k}");
    }
}
