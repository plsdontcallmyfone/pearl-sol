//! Writes the JSON vectors the Foundry tests read (token-evm/test/vectors/):
//!
//! - `mmr.json`: Commonware 2026.9.0 MMR inclusion proofs (the withdrawal log's QMDB
//!   keyless MMR over SHA-256, `Bagging::BackwardFold`), built with commonware-storage
//!   itself, every case also checked with crates/pearl-verify-sbf `mmr::verify_inclusion`
//!   (the Rust code the Solidity `Sha256Mmr` ports). Leaves are keyless `Append(entry)`
//!   elements of v2 withdrawal entries (FORMATS.md 3).
//! - `devnet-withdrawals.json`: the two real devnet withdrawals (v1 entries) and their
//!   final header from token/tests/lightclient-litesvm/fixtures/devnet.json, proofs hex.
//! - `formats.json`: an independent Rust encoding of the FORMATS.md byte layouts
//!   (deposit messages, order job id, v2 entry, PostJob body) for the Solidity helpers.
//!
//! Run: `cargo run --release` in token-evm/vectors-gen (writes ../test/vectors/).

use commonware_codec::Encode;
use commonware_cryptography::Sha256;
use commonware_storage::merkle::{hasher::Standard, Bagging};
use commonware_storage::mmr::{mem::Mmr, Location};
use pearl_verify_sbf::mmr;
use serde_json::{json, Value};
use sha2::Digest;
use std::path::Path;

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

const DOMAIN_RH: u8 = 2;
const RH_CHAIN_ID: u64 = 46630;

/// FORMATS.md 3: `0x02 || domain || nonce u64 LE || from [32] || recipient [32] || amount u64 LE || height u64 LE`.
fn entry_v2(domain: u8, nonce: u64, from: [u8; 32], recipient: [u8; 32], amount: u64, height: u64) -> Vec<u8> {
    let mut e = Vec::with_capacity(90);
    e.push(0x02);
    e.push(domain);
    e.extend_from_slice(&nonce.to_le_bytes());
    e.extend_from_slice(&from);
    e.extend_from_slice(&recipient);
    e.extend_from_slice(&amount.to_le_bytes());
    e.extend_from_slice(&height.to_le_bytes());
    assert_eq!(e.len(), 90);
    e
}

fn evm_recipient(i: u64) -> [u8; 32] {
    let mut r = [0u8; 32];
    for (j, b) in r[12..].iter_mut().enumerate() {
        *b = (0x40 + j as u8) ^ (i as u8);
    }
    r
}

fn element(entry: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 1 + 10 + entry.len()];
    let n = mmr::append_element(entry, &mut out);
    out.truncate(n);
    out
}

fn mmr_vectors() -> Value {
    let hasher: Standard<Sha256> = Standard::new(Bagging::BackwardFold);
    let mut cases = Vec::new();
    let sizes = [1u64, 2, 3, 4, 5, 6, 7, 8, 9, 11, 15, 16, 17, 31, 32, 33, 47, 64, 70];
    for &n in &sizes {
        let elements: Vec<Vec<u8>> = (0..n)
            .map(|i| element(&entry_v2(DOMAIN_RH, i, [0x11 ^ i as u8; 32], evm_recipient(i), 1_000_000 + i, 10 + i)))
            .collect();
        let mut mem = Mmr::new();
        let batch = {
            let mut batch = mem.new_batch();
            for e in &elements {
                batch = batch.add(&hasher, e);
            }
            batch.merkleize(&mem, &hasher)
        };
        mem.apply_batch(&batch).unwrap();
        let peaks = mem.peak_iterator().count();
        let mut inactives = vec![0usize, 1, peaks / 2, peaks];
        inactives.sort();
        inactives.dedup();
        for inactive in inactives {
            let root = mem.root(&hasher, inactive).unwrap();
            let root: [u8; 32] = root.as_ref().try_into().unwrap();
            let locs: Vec<u64> = if n <= 9 { (0..n).collect() } else { vec![0, 1, n / 3, n / 2, n - 2, n - 1] };
            for loc in locs {
                let p = mem.proof(&hasher, Location::new(loc), inactive).unwrap();
                let bytes = p.encode().to_vec();
                let el = &elements[loc as usize];
                mmr::verify_inclusion(sha, &bytes, el, loc, &root).expect("reference must accept");
                cases.push(json!({
                    "leaves": n, "inactive": inactive, "loc": loc, "root": hex::encode(root),
                    "proof": hex::encode(&bytes), "element": hex::encode(el), "valid": true, "why": "ok",
                }));
                // Negative cases, each also refused by the Rust reference.
                let mut neg = |why: &str, proof: &[u8], el: &[u8], loc: u64, root: &[u8; 32]| {
                    assert!(mmr::verify_inclusion(sha, proof, el, loc, root).is_err(), "{why}");
                    cases.push(json!({
                        "leaves": n, "inactive": inactive, "loc": loc, "root": hex::encode(root),
                        "proof": hex::encode(proof), "element": hex::encode(el), "valid": false, "why": why,
                    }));
                };
                if loc == 0 || loc == n - 1 {
                    let mut bad_el = el.clone();
                    let last = bad_el.len() - 1;
                    bad_el[last] ^= 1;
                    neg("element", &bytes, &bad_el, loc, &root);
                    if n > 1 {
                        neg("loc", &bytes, el, (loc + 1) % n, &root);
                    }
                    let mut bad_root = root;
                    bad_root[0] ^= 0x80;
                    neg("root", &bytes, el, loc, &bad_root);
                    neg("truncated", &bytes[..bytes.len() - 1], el, loc, &root);
                    let mut extra = bytes.clone();
                    extra.extend_from_slice(&[0u8; 32]);
                    neg("extra", &extra, el, loc, &root);
                    if bytes.len() > 3 + 32 {
                        let mut flip = bytes.clone();
                        let k = flip.len() - 5;
                        flip[k] ^= 0x01;
                        neg("digest", &flip, el, loc, &root);
                    }
                    neg("beyond", &bytes, el, n, &root);
                }
            }
        }
    }
    // Columnar (one array per field) so forge reads each with a single parseJson call.
    let col = |k: &str| Value::Array(cases.iter().map(|c| c[k].clone()).collect());
    let hexcol = |k: &str| Value::Array(cases.iter().map(|c| json!(format!("0x{}", c[k].as_str().unwrap()))).collect());
    json!({
        "source": "commonware-storage 2026.9.0 mmr::mem::Mmr + Standard<Sha256> BackwardFold; checked with crates/pearl-verify-sbf mmr::verify_inclusion",
        "count": cases.len(),
        "leaves": col("leaves"), "inactive": col("inactive"), "loc": col("loc"), "valid": col("valid"), "why": col("why"),
        "root": hexcol("root"), "proof": hexcol("proof"), "element": hexcol("element"),
    })
}

fn b64(s: &str) -> Vec<u8> {
    // Minimal standard base64 decoder (the fixture's proofs).
    let t = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = t.iter().position(|&x| x == c).expect("base64") as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    out
}

fn devnet_vectors(repo: &Path) -> Value {
    let raw = std::fs::read_to_string(repo.join("token/tests/lightclient-litesvm/fixtures/devnet.json")).unwrap();
    let d: Value = serde_json::from_str(&raw).unwrap();
    let headers = d["headers"].as_array().unwrap();
    let mut out = Vec::new();
    for w in d["withdrawals"].as_array().unwrap() {
        let s = |k: &str| w[k].as_str().map(str::to_string).unwrap_or_else(|| w[k].to_string());
        let entry = hex::decode(s("entry")).unwrap();
        let proof = b64(&s("proof"));
        let loc: u64 = s("location").parse().unwrap();
        let root_hash = s("root_block_hash");
        let h = headers.iter().find(|h| h["hash"].as_str() == Some(&root_hash)).expect("root header");
        let header = hex::decode(h["header"].as_str().unwrap()).unwrap();
        let hdr: &[u8; 256] = header.as_slice().try_into().unwrap();
        let hv = pearl_verify_sbf::header::Header(hdr);
        assert_eq!(hex::encode(hv.block_hash()), root_hash);
        let root = hv.withdraw_root();
        assert_eq!(hex::encode(root), s("withdraw_root"));
        let el = element(&entry);
        mmr::verify_inclusion(sha, &proof, &el, loc, &root).expect("devnet proof");
        out.push(json!({
            "entry": hex::encode(&entry), "element": hex::encode(&el), "loc": loc, "proof": hex::encode(&proof),
            "header": hex::encode(&header), "block_hash": root_hash, "height": hv.height(),
            "tx_root": hex::encode(hv.tx_root()), "state_root": hex::encode(hv.state_root()), "withdraw_root": hex::encode(root),
        }));
    }
    json!({ "source": "token/tests/lightclient-litesvm/fixtures/devnet.json (v1 88-byte entries, real devnet)", "withdrawals": out })
}

fn post_job_body(evals: &[[u8; 32]], uri: &[u8], w: u64, p: u64) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&64u32.to_le_bytes());
    b.extend_from_slice(&32u32.to_le_bytes());
    b.extend_from_slice(&2048u32.to_le_bytes());
    b.extend_from_slice(&[0x5a; 52]);
    b.extend_from_slice(&[0xa1; 32]);
    b.extend_from_slice(&[0xb2; 32]);
    b.extend_from_slice(&(uri.len() as u16).to_le_bytes());
    b.extend_from_slice(uri);
    b.extend_from_slice(&w.to_le_bytes());
    b.extend_from_slice(&p.to_le_bytes());
    b.extend_from_slice(&100u64.to_le_bytes());
    b.extend_from_slice(&8u32.to_le_bytes());
    b.push(evals.len() as u8);
    for e in evals {
        b.extend_from_slice(e);
    }
    b.push(1);
    b
}

fn formats_vectors() -> Value {
    let bridge: [u8; 20] = hex::decode("c0ffee0000000000000000000000000000000b1d").unwrap().try_into().unwrap();
    let recipient = [0x77u8; 32];
    let nonce = 5u64;
    let amount = 2_000_000u64;
    let mut dep = b"cadre:deposit:v2".to_vec();
    dep.push(DOMAIN_RH);
    dep.extend_from_slice(&RH_CHAIN_ID.to_le_bytes());
    dep.extend_from_slice(&bridge);
    dep.extend_from_slice(&nonce.to_le_bytes());
    dep.extend_from_slice(&recipient);
    dep.extend_from_slice(&amount.to_le_bytes());
    let body = post_job_body(&[[0xe1; 32], [0xe2; 32]], b"blob:abc", 1_900_000_000, 100_000_000);
    let body_hash = sha(&[&body]);
    let mut dep_order = b"cadre:deposit-order:v2".to_vec();
    dep_order.push(DOMAIN_RH);
    dep_order.extend_from_slice(&RH_CHAIN_ID.to_le_bytes());
    dep_order.extend_from_slice(&bridge);
    dep_order.extend_from_slice(&nonce.to_le_bytes());
    dep_order.extend_from_slice(&recipient);
    dep_order.extend_from_slice(&amount.to_le_bytes());
    dep_order.extend_from_slice(&body_hash);
    // The chain's rule (docs/BRIDGE-V2.md 3.2): no chain id, no bridge address.
    let mut job = b"cadre:order-job:v2".to_vec();
    job.push(DOMAIN_RH);
    job.extend_from_slice(&nonce.to_le_bytes());
    let job_id = sha(&[&job]);
    let entry = entry_v2(DOMAIN_RH, 9, [0x33; 32], evm_recipient(9), 123_456, 4242);
    json!({
        "bridge": hex::encode(bridge), "chain_id": RH_CHAIN_ID, "nonce": nonce, "recipient": hex::encode(recipient), "amount": amount,
        "deposit_message": hex::encode(&dep),
        "post_job_body": hex::encode(&body), "body_hash": hex::encode(body_hash),
        "deposit_order_message": hex::encode(&dep_order),
        "order_job_preimage": hex::encode(&job), "order_job_id": hex::encode(job_id),
        "entry_v2": hex::encode(&entry), "entry_v2_element": hex::encode(element(&entry)),
    })
}

fn main() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = here.join("../..");
    let out = here.join("../test/vectors");
    std::fs::create_dir_all(&out).unwrap();
    let w = |name: &str, v: Value| {
        std::fs::write(out.join(name), serde_json::to_string_pretty(&v).unwrap() + "\n").unwrap();
        println!("wrote {name}");
    };
    let m = mmr_vectors();
    println!("mmr cases: {}", m["count"]);
    w("mmr.json", m);
    w("devnet-withdrawals.json", devnet_vectors(&repo));
    w("formats.json", formats_vectors());
}
