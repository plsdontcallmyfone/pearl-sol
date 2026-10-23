//! tx_root leaves of a block with an uncle (regression: public devnet block 6299).
//!
//! The relay rebuilt tx_root paths from the txs, the contracts record and the coinbase
//! receipt, but not the uncle records the chain puts between them (docs/RPC.md getBlock,
//! chain/agent-types/tests/uncle_body.rs). Every job posted in a block that credits an
//! uncle then failed begin_verify_job on Solana with TxPath (6016), and the light client
//! stopped at the header that mined the first of those jobs (6300) for good.

use header_relay::chain::{block_body_of, tx_root_paths};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/devnet-block-6299-uncle.json")).unwrap()
}

#[test]
fn uncle_record_is_a_leaf_and_root_matches_header() {
    let v = fixture();
    let (txs, cb) = block_body_of(&v).expect("leaves rebuild the header's tx_root");
    let n_txs = v["txs"].as_array().unwrap().len();
    assert_eq!(txs.len(), n_txs + 1, "66 txs + 1 uncle record");
    assert_eq!(v["coinbase"]["leaf_index"].as_u64().unwrap() as usize, txs.len(), "coinbase is the last leaf");
    let (root, paths) = tx_root_paths(&txs, cb.as_deref());
    assert_eq!(hex::encode(root), v["header_fields"]["tx_root"].as_str().unwrap());
    // every path the relay sends equals the node's own (33-byte entries, bottom-up)
    for (i, t) in v["txs"].as_array().unwrap().iter().enumerate() {
        assert_eq!(hex::encode(&paths[i]), t["path"].as_str().unwrap(), "tx {i} path");
    }
    assert_eq!(hex::encode(&paths[n_txs]), v["uncles"][0]["path"].as_str().unwrap(), "uncle record path");
    assert_eq!(hex::encode(&paths[txs.len()]), v["coinbase"]["path"].as_str().unwrap(), "coinbase path");
}

#[test]
fn leaving_the_uncle_out_is_refused_not_sent() {
    let mut v = fixture();
    v.as_object_mut().unwrap().remove("uncles");
    let e = block_body_of(&v).unwrap_err().to_string();
    assert!(e.contains("tx_root mismatch"), "{e}");
}

#[test]
fn reply_without_header_fields_is_not_checked() {
    let mut v = fixture();
    v.as_object_mut().unwrap().remove("header_fields");
    v.as_object_mut().unwrap().remove("uncles");
    assert!(block_body_of(&v).is_ok());
}
