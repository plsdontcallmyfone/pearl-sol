//! Weak blocks (docs/SPEED.md 2): message codec and the receive checks, with MockVerifier
//! (weak proof = "mockweak" || pow_digest) and with real Pearl proofs at an easy target.

use agent_types::tx::pubkey_from_seed;
use agent_types::*;
use commonware_runtime::{deterministic, Runner as _, Supervisor as _};
use consensus_pow::{is_weak_msg, Chain, Genesis, JobChoice, MockVerifier, PearlVerifier, Template, WeakBlock, WeakReject};
use state::config::Config;

const CHAIN: u32 = 7;
const EASY: u32 = 0x207fffff;
const T0: u64 = 1_760_000_000_000;

fn pk(i: u8) -> Pubkey {
    pubkey_from_seed(&[i; 32])
}

fn genesis(block_ms: u64, bits: u32) -> Genesis {
    let mut c = Config::defaults(pk(1));
    c.target_block_time_ms = block_ms;
    c.min_fee = 10;
    c.attesters = vec![pk(50)];
    c.attester_threshold = 1;
    Genesis { chain_id: CHAIN, timestamp_ms: T0, target_bits: bits, config: c }
}

type C<V> = Chain<deterministic::Context, V>;

fn weak_of(t: &Template) -> WeakBlock {
    let mut h = t.header;
    let proof = MockVerifier::weak_proof_for(&h);
    h.proof_hash = block::proof_hash(&proof);
    WeakBlock::from_template(t, h, proof)
}

fn seal(t: &Template) -> Block {
    let mut h = t.header;
    let proof = MockVerifier::proof_for(&h);
    h.proof_hash = block::proof_hash(&proof);
    Block { header: h, proof, txs: t.txs.clone(), coinbase: Some(t.summary.coinbase), contracts: t.summary.contracts, uncles: Vec::new() }
}

#[test]
fn codec_roundtrip_and_malformed() {
    deterministic::Runner::default().start(|ctx| async move {
        let c = C::open(ctx.child("a"), "n", genesis(60_000, EASY), MockVerifier).await.unwrap();
        let t = c.build_template(pk(3), JobChoice::Fallback, &[], T0 + 1000).await.unwrap();
        let w = weak_of(&t);
        let b = w.encode();
        assert!(is_weak_msg(&b));
        assert!(!is_weak_msg(&[0u8; 40]), "a 40-byte tip is never a weak message");
        let mut tip = b"CWB1".to_vec();
        tip.resize(40, 0);
        assert!(!is_weak_msg(&tip), "even with the magic, 40 bytes is a tip");
        assert_eq!(WeakBlock::decode(&b).unwrap(), w);
        assert_eq!(w.tx_hashes().len(), 0);
        assert_eq!(*w.items.last().unwrap(), t.summary.coinbase.hash());
        // truncations, trailing bytes, bad magic, bad flag, zero items
        for n in [0, 3, 4, 100, b.len() - 1] {
            assert!(WeakBlock::decode(&b[..n]).is_err(), "truncated at {n}");
        }
        let mut x = b.clone();
        x.push(0);
        assert!(WeakBlock::decode(&x).is_err());
        let mut x = b.clone();
        x[0] = b'X';
        assert!(WeakBlock::decode(&x).is_err());
        let mut x = b.clone();
        *x.last_mut().unwrap() = 2;
        assert!(WeakBlock::decode(&x).is_err());
        let mut z = w.clone();
        z.items.clear();
        assert!(WeakBlock::decode(&z.encode()).is_err());
    });
}

#[test]
fn checks_accept_and_reject() {
    deterministic::Runner::default().start(|ctx| async move {
        let mut c = C::open(ctx.child("a"), "n", genesis(60_000, EASY), MockVerifier).await.unwrap();
        let g = c.genesis_hash();
        let now = T0 + 1000;
        assert_eq!(c.weak_shift_for(&g), 5);
        let t = c.build_template(pk(3), JobChoice::Fallback, &[], now).await.unwrap();
        let w = weak_of(&t);
        c.check_weak(&w, now).await.unwrap();
        // a strong proof also passes the weak check
        let b = seal(&t);
        let sw = WeakBlock::from_template(&t, b.header, b.proof.clone());
        c.check_weak(&sw, now).await.unwrap();
        let inv = |r: Result<(), WeakReject>| matches!(r, Err(WeakReject::Invalid(_)));
        // wrong tx_root items
        let mut x = w.clone();
        x.items.insert(0, [5; 32]);
        assert!(inv(c.check_weak(&x, now).await));
        // wrong target
        let mut x = w.clone();
        x.header.target_bits = 0x1f00ffff;
        assert!(inv(c.check_weak(&x, now).await));
        // wrong height
        let mut x = w.clone();
        x.header.height = 5;
        assert!(inv(c.check_weak(&x, now).await));
        // proof that binds another header
        let mut x = w.clone();
        x.header.nonce = 9;
        assert!(inv(c.check_weak(&x, now).await), "proof no longer binds the header");
        // garbage proof with a matching proof_hash
        let mut x = w.clone();
        x.proof = b"nonsense".to_vec();
        x.header.proof_hash = block::proof_hash(&x.proof);
        assert!(inv(c.check_weak(&x, now).await));
        // unknown parent: ignored, not invalid
        let mut x = w.clone();
        x.header.prev_hash = [7; 32];
        assert!(matches!(c.check_weak(&x, now).await, Err(WeakReject::Ignore(_))));
        // future timestamp: ignored
        let mut x = w.clone();
        x.header.timestamp_ms = now + 10_000_000;
        let p = MockVerifier::weak_proof_for(&x.header);
        x.header.proof_hash = block::proof_hash(&p);
        x.proof = p;
        assert!(matches!(c.check_weak(&x, now).await, Err(WeakReject::Ignore(_))));
        // once the tip is 3 above the parent, weak blocks on it are ignored
        let mut parent = g;
        for i in 0..3 {
            let t = c.build_template_at(parent, pk(4), JobChoice::Fallback, &[], now + 1000 * (i + 1)).await.unwrap();
            let b = seal(&t);
            parent = b.hash();
            c.insert_block(b, now + 1000 * (i + 1)).await.unwrap();
        }
        assert!(matches!(c.check_weak(&w, now + 4000).await, Err(WeakReject::Ignore(_))));
    });
}

#[test]
fn weak_off_at_short_block_times() {
    deterministic::Runner::default().start(|ctx| async move {
        let c = C::open(ctx.child("a"), "n", genesis(1500, EASY), MockVerifier).await.unwrap();
        assert_eq!(c.weak_shift_for(&c.genesis_hash()), 0);
        assert!(c.weak_bits_for(&c.genesis_hash()).is_none());
        let t = c.build_template(pk(3), JobChoice::Fallback, &[], T0 + 1000).await.unwrap();
        assert!(matches!(c.check_weak(&weak_of(&t), T0 + 1000).await, Err(WeakReject::Ignore(_))));
    });
}

#[test]
fn real_pearl_weak_proofs() {
    // strong target hard enough that most weak hits are not blocks; weak = strong << 5
    deterministic::Runner::default().start(|ctx| async move {
        let f = pearl_job::job::penalized_adjustment_factor(&pearl_job::MiningConfig::localnet_default());
        let strong = pearl_job::pow::target_to_bits((primitive_types::U256::MAX >> 7) / primitive_types::U256::from(f));
        let c = C::open(ctx.child("a"), "n", genesis(60_000, strong), PearlVerifier).await.unwrap();
        let g = c.genesis_hash();
        let t = c.build_template(pk(3), JobChoice::Fallback, &[], T0 + 1000).await.unwrap();
        let wb = c.weak_bits_for(&g).unwrap();
        let ph = consensus_pow::verifier::to_pearl_header(&t.header);
        let work = pearl_job::BlockWork::Fallback(t.fallback);
        let (mut weak, mut strong_n) = (0, 0);
        for n in 0..400u64 {
            if let Some((mb, is_strong)) = pearl_job::pow::mine_header_nonce_at(&ph, &work, n, wb).unwrap() {
                let h = Header::decode(&mb.header.to_bytes()).unwrap();
                let w = WeakBlock::from_template(&t, h, mb.proof.clone());
                c.check_weak(&w, T0 + 1000).await.unwrap();
                if is_strong { strong_n += 1 } else { weak += 1 }
                // a weak-only proof is not a valid block
                assert_eq!(consensus_pow::PowVerifier::verify(&PearlVerifier, &h, None, &mb.proof, &t.fallback).is_ok(), is_strong);
            }
        }
        assert!(weak >= 3, "weak hits {weak}");
        assert!(weak > strong_n, "weak {weak} strong {strong_n}");
    });
}
