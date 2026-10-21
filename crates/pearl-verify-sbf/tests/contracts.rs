//! Contract proofs (docs/CONTRACTS.md "Proofs") against the chain's published vectors
//! (chain/agent-types/vectors/contracts.json): the contracts record under tx_root, an event
//! and a receipt under it, and tampering fails.

use pearl_verify_sbf::contracts;
use pearl_verify_sbf::header::{off, Header};

fn vectors() -> serde_json::Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chain/agent-types/vectors/contracts.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}
fn hx(v: &serde_json::Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

#[test]
fn event_and_receipt_prove_under_tx_root() {
    let v = vectors();
    let b = &v["block"];
    let mut hb = [0u8; 256];
    hb[off::VERSION..off::VERSION + 4].copy_from_slice(&1u32.to_le_bytes());
    hb[off::HEIGHT..off::HEIGHT + 8].copy_from_slice(&42u64.to_le_bytes());
    hb[off::TX_ROOT..off::TX_ROOT + 32].copy_from_slice(&hx(&b["tx_root"]));
    let h = Header(&hb);
    let rec = hx(&v["record"]["bytes"]);
    let rpath = hx(&b["record_path"]);
    let ev = hx(&v["event"]["bytes"]);
    let epath = hx(&b["event_path"]);
    let r = contracts::verify_record(&h, &rec, &rpath).unwrap();
    assert_eq!((r.height, r.event_count, r.receipt_count), (42, 1, 1));
    let e = contracts::verify_event(&h, &rec, &rpath, &ev, &epath).unwrap();
    assert_eq!(hex::encode(e.topic(0).unwrap()), v["event_topic"]["topic"].as_str().unwrap());
    assert_eq!(e.data, &5u64.to_le_bytes());
    let rc = hx(&v["receipt"]["bytes"]);
    let (_, status, gas) = contracts::verify_receipt(&h, &rec, &rpath, &rc, &[]).unwrap();
    assert_eq!((status, gas), (0, 412_345));
    // tampering
    let mut bad = ev.clone();
    *bad.last_mut().unwrap() ^= 1;
    assert!(contracts::verify_event(&h, &rec, &rpath, &bad, &epath).is_err());
    let mut bad = rec.clone();
    bad[50] ^= 1;
    assert!(contracts::verify_event(&h, &bad, &rpath, &ev, &epath).is_err());
    let mut bp = rpath.clone();
    bp[3] ^= 1;
    assert!(contracts::verify_record(&h, &rec, &bp).is_err());
    let mut other = hb;
    other[off::HEIGHT] ^= 1;
    assert!(contracts::verify_record(&Header(&other), &rec, &rpath).is_err());
    // the coinbase receipt still proves as the last leaf of the same block
    let cb = hx(&b["coinbase"]);
    let cp = hx(&b["coinbase_path"]);
    pearl_verify_sbf::chaintx::verify_tx_path(&pearl_verify_sbf::chaintx::tx_hash(&cb), &cp, &h.tx_root()).unwrap();
}
