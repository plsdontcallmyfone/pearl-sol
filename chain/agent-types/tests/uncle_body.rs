//! Block bodies with uncles (docs/SPEED.md 3.3): the 0x02 section after the coinbase and the
//! contracts section, uncle record items in tx_root between the contracts record and the
//! coinbase, context-free checks, and blocks without uncles encoding exactly as before.

use agent_types::speed::{Uncle, UncleRecord};
use agent_types::*;

fn uncle(height: u64, tag: u8) -> Uncle {
    let proof = vec![tag; 40];
    let header = Header { version: 1, height: height - 1, job_id: [tag; 32], miner: [tag; 32], proof_hash: block::proof_hash(&proof), ..Default::default() };
    let record = UncleRecord {
        height,
        uncle_hash: header.hash(),
        uncle_height: height - 1,
        miner: [tag; 32],
        job_id: [tag; 32],
        reward: 10,
        nephew: 0,
        mature_height: height + 60,
    };
    Uncle { header, proof, record }
}

fn base(height: u64) -> Block {
    let cb = Coinbase { height, miner: [1; 32], fees: 0, subsidy: 0, job_id: [0; 32], mature_height: height + 60 };
    let tx = Tx::signed(&[3; 32], 7, 0, 1000, TxBody::Transfer { to: [4; 32], amount: 1 });
    let header = Header { version: 1, height, miner: [1; 32], ..Default::default() };
    let mut b = Block { header, proof: vec![1; 8], txs: vec![tx], coinbase: Some(cb), contracts: None, uncles: Vec::new() };
    b.header.proof_hash = b.proof_hash();
    b.header.tx_root = b.tx_root();
    b
}

#[test]
fn no_uncles_encodes_as_before() {
    let b = base(10);
    let enc = b.encode();
    // header || proof || txs || has_coinbase 1 || coinbase: nothing after the receipt
    let tail = &enc[enc.len() - 98..];
    assert_eq!(tail[0], 1);
    assert_eq!(&tail[1..], &b.coinbase.unwrap().encode());
    assert_eq!(b.tx_root(), block::tx_root_full(&b.txs, None, b.coinbase.as_ref()));
    let d = Block::decode(&enc).unwrap();
    assert!(d.uncles.is_empty());
    assert_eq!(d.encode(), enc);
}

#[test]
fn uncles_roundtrip_commit_and_check() {
    let mut b = base(10);
    b.uncles = vec![uncle(10, 7), uncle(10, 8)];
    b.header.tx_root = b.tx_root();
    b.check_commitments().unwrap();
    let enc = b.encode();
    let d = Block::decode(&enc).unwrap();
    assert_eq!(d, b);
    assert_eq!(d.encode(), enc);
    // leaf order: txs, uncle records, coinbase last
    let items = b.leaf_items();
    assert_eq!(items.len(), 4);
    assert_eq!(items[1], b.uncles[0].record.hash());
    assert_eq!(items[2], b.uncles[1].record.hash());
    assert_eq!(items[3], b.coinbase.unwrap().hash());
    assert_eq!((b.uncle_index(1), b.coinbase_index()), (2, 3));
    // a record that does not match its header, or the wrong height, or a fallback uncle
    let mut x = b.clone();
    x.uncles[0].record.miner = [0; 32];
    x.header.tx_root = x.tx_root();
    assert!(x.check_commitments().is_err());
    let mut x = b.clone();
    x.uncles[0].record.height = 9;
    x.header.tx_root = x.tx_root();
    assert!(x.check_commitments().is_err());
    let mut x = b.clone();
    x.uncles[1] = x.uncles[0].clone();
    x.header.tx_root = x.tx_root();
    assert_eq!(x.check_commitments().unwrap_err(), "duplicate uncle");
    let mut x = b.clone();
    x.uncles[0].proof.push(0);
    assert!(x.check_commitments().is_err(), "proof_hash binds the uncle proof");
    // tx_root covers the records
    let mut x = b.clone();
    x.uncles[0].record.reward += 1;
    assert_eq!(x.check_commitments().unwrap_err(), "tx_root mismatch");
}

#[test]
fn malformed_uncle_sections_refused() {
    let mut b = base(10);
    b.uncles = vec![uncle(10, 7)];
    let enc = b.encode();
    // count 3
    let mut three = base(10);
    three.uncles = vec![uncle(10, 1), uncle(10, 2), uncle(10, 3)];
    assert!(Block::decode(&three.encode()).is_err());
    // count 0
    let mut zero = base(10).encode();
    zero.extend_from_slice(&[2, 0]);
    assert!(Block::decode(&zero).is_err());
    // uncle section before the contracts section (tags must increase), unknown tag, truncation
    let mut bad = base(10).encode();
    bad.extend_from_slice(&[3]);
    assert!(Block::decode(&bad).is_err());
    assert!(Block::decode(&enc[..enc.len() - 1]).is_err());
    let mut dup = enc.clone();
    dup.extend_from_slice(&enc[enc.len() - (1 + 1 + 256 + 4 + 40 + 137)..]);
    assert!(Block::decode(&dup).is_err(), "a second uncle section");
}
