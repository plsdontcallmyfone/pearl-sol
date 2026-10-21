//! Property tests over the wire codecs (security pass 2026-10-21).
//!
//! Untrusted bytes reach these decoders from every peer (gossip, sync) and from RPC
//! (sendTx, submitBlock). Two properties:
//! 1. decoding arbitrary or mutated bytes never panics;
//! 2. decoding is canonical: when bytes decode, re-encoding gives the same bytes. A tx's
//!    hash and signature cover the re-encoded bytes, so a second accepted encoding of one
//!    tx would be malleability (two hashes for one signed tx).

use agent_types::block::{decode_header_with_proof, decode_txs, encode_txs};
use agent_types::social::{FollowEdge, Post, Reaction, SetProfile};
use agent_types::tx::{MatrixSpec, PostJob, RegisterModel, Segment};
use agent_types::{AttesterSig, Block, Coinbase, Header, Tx, TxBody};
use proptest::prelude::*;

fn job() -> PostJob {
    PostJob {
        m: 4,
        n: 8,
        k: 2048,
        config: [3; 52],
        hash_a: [4; 32],
        hash_b: [5; 32],
        data_uri: b"blob:00".to_vec(),
        worker_pay: 10,
        protocol_fee: 20,
        deadline_height: 30,
        max_blocks: 2,
        evaluators: vec![[6; 32], [7; 32]],
        threshold: 1,
    }
}

fn sigs() -> Vec<AttesterSig> {
    vec![AttesterSig { pubkey: [8; 32], sig: [9; 64] }]
}

/// One valid tx of every kind (bodies only need to decode, not to execute).
fn samples() -> Vec<Tx> {
    let bodies = vec![
        TxBody::Transfer { to: [1; 32], amount: 5 },
        TxBody::RegisterAgent { name: b"agent".to_vec(), uri: b"https://x".to_vec() },
        TxBody::PostJob(job()),
        TxBody::SubmitResult { job_id: [1; 32], result_hash: [2; 32], uri: b"blob:ab".to_vec() },
        TxBody::CompleteJob { job_id: [1; 32], sigs: sigs() },
        TxBody::ExpireJob { job_id: [1; 32] },
        TxBody::BridgeUnlock { solana_nonce: 3, recipient: [2; 32], amount_spl: 9, job_order: None, sigs: sigs() },
        TxBody::BridgeUnlock { solana_nonce: 4, recipient: [2; 32], amount_spl: 9, job_order: Some(job()), sigs: sigs() },
        TxBody::BridgeLock { solana_recipient: [3; 32], amount: 1000 },
        TxBody::AdminConfig { key: 1, value: 1000u64.to_le_bytes().to_vec() },
        TxBody::SetRoute { solana_recipient: [1; 32], route_bps: 100, until_height: 9, cap: 5, release_key: [2; 32] },
        TxBody::ClearRoute { account: [1; 32], route_nonce: 2, sig: [3; 64] },
        TxBody::RegisterModel(RegisterModel {
            name: b"m".to_vec(),
            manifest_uri: b"blob:ff".to_vec(),
            recipe: 1,
            rank: 128,
            rows_pattern: [1; 6],
            cols_pattern: [2; 6],
            matrices: vec![MatrixSpec { k: 2048, n: 8, hash_b: [1; 32] }; 3],
        }),
        TxBody::PostSegments {
            ticket_id: [1; 32],
            round: 0,
            model_id: [2; 32],
            rank: 128,
            rows_pattern: [1; 6],
            cols_pattern: [2; 6],
            segments: vec![Segment { matrix: 0, m: 4, k: 2048, n: 8, max_blocks: 1, hash_a: [1; 32], hash_b: [2; 32], c_hash: [3; 32], a_uri: b"blob:01".to_vec() }],
        },
        TxBody::RejectResult { job_id: [1; 32], deliverer: [2; 32], result_hash: [3; 32], sigs: sigs() },
        TxBody::SetProfile(SetProfile { handle: b"abc".to_vec(), name: b"A".to_vec(), ..Default::default() }),
        TxBody::OperatorLink { agent: [1; 32], add: true },
        TxBody::Post(Post { body: b"hi".to_vec(), ..Default::default() }),
        TxBody::React { items: vec![Reaction { post_id: [1; 32], reaction: 0, tip: 0 }] },
        TxBody::Follow { items: vec![FollowEdge { target: [1; 32], on: true }] },
    ];
    bodies.into_iter().enumerate().map(|(i, b)| Tx::signed(&[i as u8 + 1; 32], 7, i as u64, 1000, b)).collect()
}

fn sample_block() -> Block {
    let txs = samples();
    let cb = Coinbase { height: 5, miner: [1; 32], fees: 7, subsidy: 8, job_id: [0; 32], mature_height: 35 };
    let header = Header { version: 1, height: 5, nonce: 3, ..Default::default() };
    Block { header, proof: vec![0xab; 64], txs, coinbase: Some(cb), contracts: None }
}

/// Decode `b` as every wire type; each must return without panicking, and a tx or block
/// that decodes must re-encode to the same bytes.
fn decode_all(b: &[u8]) {
    if let Ok(t) = Tx::decode(b) {
        assert_eq!(t.encode(), b, "tx decode is not canonical");
        let _ = t.verify_sig();
        let _ = t.hash();
    }
    if let Ok(blk) = Block::decode(b) {
        assert_eq!(blk.encode(), b, "block decode is not canonical");
        let _ = blk.check_commitments();
    }
    if let Ok(h) = Header::decode(b) {
        assert_eq!(h.encode().as_slice(), b);
    }
    let _ = Coinbase::decode(b);
    let _ = decode_header_with_proof(b);
    if let Ok(txs) = decode_txs(b) {
        assert_eq!(encode_txs(&txs), b, "tx list decode is not canonical");
    }
    // the commonware codec wrappers the p2p channels use
    let mut buf = b;
    let _ = <Tx as commonware_codec::Read>::read_cfg(&mut buf, &());
    let mut buf = b;
    let _ = <Header as commonware_codec::Read>::read_cfg(&mut buf, &());
}

#[test]
fn samples_round_trip() {
    for t in samples() {
        let b = t.encode();
        assert_eq!(Tx::decode(&b).unwrap(), t, "{:?}", t.body.kind());
        assert!(t.verify_sig());
    }
    let blk = sample_block();
    assert_eq!(Block::decode(&blk.encode()).unwrap(), blk);
}

#[derive(Debug, Clone)]
enum Mutation {
    Flip(usize, u8),
    Truncate(usize),
    Insert(usize, Vec<u8>),
    Delete(usize, usize),
    SetU16(usize, u16),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (any::<usize>(), 1u8..=255).prop_map(|(i, x)| Mutation::Flip(i, x)),
        any::<usize>().prop_map(Mutation::Truncate),
        (any::<usize>(), prop::collection::vec(any::<u8>(), 1..64)).prop_map(|(i, v)| Mutation::Insert(i, v)),
        (any::<usize>(), 1usize..64).prop_map(|(i, n)| Mutation::Delete(i, n)),
        (any::<usize>(), any::<u16>()).prop_map(|(i, v)| Mutation::SetU16(i, v)),
    ]
}

fn apply(mut b: Vec<u8>, ms: &[Mutation]) -> Vec<u8> {
    for m in ms {
        if b.is_empty() {
            break;
        }
        let n = b.len();
        match m {
            Mutation::Flip(i, x) => b[i % n] ^= x,
            Mutation::Truncate(i) => b.truncate(i % n),
            Mutation::Insert(i, v) => {
                let at = i % (n + 1);
                b.splice(at..at, v.iter().copied());
            }
            Mutation::Delete(i, k) => {
                let at = i % n;
                let end = (at + k).min(n);
                b.drain(at..end);
            }
            Mutation::SetU16(i, v) => {
                let at = i % n;
                for (j, x) in v.to_le_bytes().iter().enumerate() {
                    if at + j < b.len() {
                        b[at + j] = *x;
                    }
                }
            }
        }
    }
    b
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, .. ProptestConfig::default() })]

    /// Random bytes (with a valid tx prefix half of the time).
    #[test]
    fn random_bytes_never_panic(mut b in prop::collection::vec(any::<u8>(), 0..2048), kind in 0u8..32, prefix in any::<bool>()) {
        if prefix && b.len() > 54 {
            b[0] = 1;
            b[53] = kind;
        }
        decode_all(&b);
    }

    /// Mutated valid txs of every kind.
    #[test]
    fn mutated_txs_never_panic(idx in 0usize..20, ms in prop::collection::vec(mutation(), 1..6)) {
        let s = samples();
        let b = apply(s[idx % s.len()].encode(), &ms);
        decode_all(&b);
    }

    /// Mutated valid blocks.
    #[test]
    fn mutated_blocks_never_panic(ms in prop::collection::vec(mutation(), 1..6)) {
        let b = apply(sample_block().encode(), &ms);
        decode_all(&b);
    }

    /// Length prefixes claiming more than the buffer holds (u32 fields of a block) are refused, not allocated.
    #[test]
    fn huge_length_prefixes(plen in any::<u32>(), count in any::<u32>()) {
        let mut b = Header { version: 1, ..Default::default() }.encode().to_vec();
        b.extend_from_slice(&plen.to_le_bytes());
        b.extend_from_slice(&count.to_le_bytes());
        decode_all(&b);
    }
}
