//! Cross-check against the node's own vectors (chain/agent-types/vectors, written by
//! the node agent). Skipped when the file is not there.

use pearl_job::pow::BlockHeader;
use serde_json::Value;

fn vectors(name: &str) -> Option<Value> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chain/agent-types/vectors").join(name);
    let s = std::fs::read_to_string(&p).ok()?;
    Some(serde_json::from_str(&s).unwrap())
}

fn h32(v: &Value) -> [u8; 32] {
    let s = v.as_str().unwrap();
    pearl_job::spec::parse_key32(s).unwrap()
}

#[test]
fn header_matches_agent_types() {
    let Some(v) = vectors("header.json") else {
        eprintln!("chain/agent-types/vectors/header.json missing, skipping");
        return;
    };
    let f = &v["fields"];
    let h = BlockHeader {
        version: f["version"].as_u64().unwrap() as u32,
        height: f["height"].as_u64().unwrap(),
        prev_hash: h32(&f["prev_hash"]),
        timestamp_ms: f["timestamp_ms"].as_u64().unwrap(),
        target_bits: f["target_bits"].as_u64().unwrap() as u32,
        job_id: h32(&f["job_id"]),
        miner: h32(&f["miner"]),
        tx_root: h32(&f["tx_root"]),
        state_root: h32(&f["state_root"]),
        withdraw_root: h32(&f["withdraw_root"]),
        proof_hash: h32(&f["proof_hash"]),
        nonce: f["nonce"].as_u64().unwrap(),
    };
    assert_eq!(hex::encode(h.to_bytes()), v["header"].as_str().unwrap());
    assert_eq!(hex::encode(h.pow_digest()), v["pow_digest"].as_str().unwrap());
    assert_eq!(BlockHeader::from_bytes(&hex::decode(v["header"].as_str().unwrap()).unwrap()).unwrap(), h);
    // header_with_proof = header || proof; proof_hash covers the proof bytes.
    let hwp = hex::decode(v["header_with_proof"].as_str().unwrap()).unwrap();
    assert_eq!(&hwp[..256], &h.to_bytes()[..]);
}
