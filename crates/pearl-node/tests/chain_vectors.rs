//! Cross-check SPEC section 2 tx encoding and the v2 Result message against the node's
//! vectors (chain/agent-types/vectors). Skipped when the files are not there.

use ed25519_dalek::{Signer, SigningKey};
use pearl_job::messages::result_message;
use pearl_job::spec::parse_key32;
use pearl_node::tx::{self, Body, Tx};
use serde_json::Value;

fn vectors(name: &str) -> Option<Value> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../chain/agent-types/vectors").join(name);
    Some(serde_json::from_str(&std::fs::read_to_string(&p).ok()?).unwrap())
}

fn k(v: &Value) -> [u8; 32] {
    parse_key32(v.as_str().unwrap()).unwrap()
}

fn n(v: &Value) -> u64 {
    pearl_node::num(v).unwrap()
}

#[test]
fn txs_match_agent_types() {
    let Some(v) = vectors("tx.json") else {
        eprintln!("tx.json missing, skipping");
        return;
    };
    let mut checked = 0;
    for t in v.as_array().unwrap() {
        let b = &t["body"];
        let body = match t["name"].as_str().unwrap() {
            "transfer" => Body::Transfer { to: k(&b["to"]), amount: n(&b["amount"]) },
            "submit_result" => Body::SubmitResult { job_id: k(&b["job_id"]), result_hash: k(&b["result_hash"]), uri: b["uri"].as_str().unwrap().into() },
            "complete_job" => Body::CompleteJob {
                job_id: k(&b["job_id"]),
                sigs: b["sigs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| (k(&s["pubkey"]), hex::decode(s["sig"].as_str().unwrap()).unwrap().try_into().unwrap()))
                    .collect(),
            },
            "expire_job" => Body::ExpireJob { job_id: k(&b["job_id"]) },
            _ => continue,
        };
        let key = SigningKey::from_bytes(&k(&t["seed"]));
        assert_eq!(key.verifying_key().to_bytes(), k(&t["from"]), "seed -> pubkey");
        let tx = Tx { chain_id: n(&t["chain_id"]) as u32, from: k(&t["from"]), nonce: n(&t["nonce"]), fee: n(&t["fee"]), body };
        let unsigned = tx::unsigned_bytes(&tx).unwrap();
        let name = t["name"].as_str().unwrap();
        assert_eq!(hex::encode(&unsigned), t["signing_bytes"].as_str().unwrap(), "{name}: unsigned bytes");
        assert_eq!(hex::encode(tx::signing_bytes(&unsigned)), t["framed_message"].as_str().unwrap(), "{name}: framing");
        let wire = tx::sign(&tx, &key).unwrap();
        assert_eq!(hex::encode(&wire), t["tx"].as_str().unwrap(), "{name}: wire");
        assert_eq!(hex::encode(blake3::hash(&wire).as_bytes()), t["tx_hash"].as_str().unwrap(), "{name}: tx_hash");
        assert_eq!(tx::decode(&wire).unwrap().0, tx);
        checked += 1;
    }
    assert_eq!(checked, 4, "transfer, submit_result, complete_job, expire_job");
}

#[test]
fn result_message_matches_agent_types() {
    let Some(v) = vectors("messages.json") else {
        eprintln!("messages.json missing, skipping");
        return;
    };
    let r = v.as_array().unwrap().iter().find(|m| m["name"] == "result").expect("result vector");
    // Raw ed25519 over b"pearl-sol:result:v1" || job_id || deliverer || result_hash (no framing).
    let msg = hex::decode(r["message"].as_str().unwrap()).unwrap();
    assert_eq!(r["domain"], "pearl-sol:result:v1");
    let body = &msg[19..];
    let (job, deliverer, rh): ([u8; 32], [u8; 32], [u8; 32]) =
        (body[..32].try_into().unwrap(), body[32..64].try_into().unwrap(), body[64..96].try_into().unwrap());
    let ours = result_message(&job, &deliverer, &rh);
    assert_eq!(ours, msg);
    let key = SigningKey::from_bytes(&k(&r["seed"]));
    assert_eq!(key.verifying_key().to_bytes(), k(&r["pubkey"]));
    assert_eq!(hex::encode(key.sign(&ours).to_bytes()), r["sig"].as_str().unwrap());
}

/// SPEC 4.5: the gateway's PostSegments bytes equal the chain's vector.
#[test]
fn post_segments_matches_agent_types() {
    let Some(v) = vectors("standing.json") else {
        eprintln!("standing.json missing, skipping");
        return;
    };
    let t = v.as_array().unwrap().iter().find(|x| x["name"] == "post_segments").unwrap();
    let b = &t["body"];
    let p6 = |s: &Value| -> [u8; 6] { hex::decode(s.as_str().unwrap()).unwrap().try_into().unwrap() };
    let segments = b["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| tx::SegmentSpec {
            matrix: n(&g["matrix"]) as u16,
            m: n(&g["m"]) as u32,
            k: n(&g["k"]) as u32,
            n: n(&g["n"]) as u32,
            max_blocks: n(&g["max_blocks"]) as u32,
            hash_a: k(&g["hash_a"]),
            hash_b: k(&g["hash_b"]),
            c_hash: k(&g["c_hash"]),
            a_uri: g["a_uri"].as_str().unwrap().into(),
        })
        .collect();
    let head = tx::SegmentsHead { model_id: k(&b["model_id"]), rank: n(&b["rank"]) as u16, rows_pattern: p6(&b["rows_pattern"]), cols_pattern: p6(&b["cols_pattern"]) };
    let body = Body::PostSegments { ticket_id: k(&b["ticket_id"]), round: n(&b["round"]) as u16, head, segments };
    let key = SigningKey::from_bytes(&k(&t["seed"]));
    let tx = Tx { chain_id: n(&t["chain_id"]) as u32, from: k(&t["from"]), nonce: n(&t["nonce"]), fee: n(&t["fee"]), body };
    assert_eq!(hex::encode(tx::sign(&tx, &key).unwrap()), t["tx"].as_str().unwrap());
}
