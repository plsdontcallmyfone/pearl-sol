//! Cx address fields in JSON-RPC output (docs/SPEC.md 2.1, docs/RPC.md "Addresses").
//!
//! Every result keeps its base58 address fields unchanged (transition: existing clients
//! parse them). Next to each Cadre-chain address field `k` this adds `k_cx` with the
//! checksummed Cx form (arrays of addresses get an array). Solana-side keys
//! (`solana_recipient`), X25519 DM keys (`enc_key`, `sender_enc_key`) and p2p node identities
//! (`key`, `me`) are not account addresses and are left alone.

use agent_types::cx::to_cx;
use serde_json::{Map, Value};

/// Field names that hold a Cadre-chain account address in RPC results.
pub const ADDRESS_FIELDS: &[&str] = &[
    "account", "address", "admin", "admins", "agent", "a", "b", "attesters", "author", "contract", "creator",
    "deliverer", "deployer", "escrow", "evaluators", "follower", "from", "house_operators", "member", "miner",
    "operator", "peer", "pool", "poster", "recipient", "registrant", "release_key", "signers", "target", "to",
];

fn cx_of(s: &str) -> Option<String> {
    let v = bs58::decode(s).into_vec().ok()?;
    let k: [u8; 32] = v.try_into().ok()?;
    Some(to_cx(&k))
}

fn cx_value(v: &Value) -> Option<Value> {
    match v {
        Value::String(s) => cx_of(s).map(Value::String),
        Value::Array(a) if !a.is_empty() && a.iter().all(|x| x.is_string()) => {
            let out: Option<Vec<Value>> = a.iter().map(|x| cx_of(x.as_str()?).map(Value::String)).collect();
            out.map(Value::Array)
        }
        _ => None,
    }
}

fn add_object(m: &mut Map<String, Value>) {
    for v in m.values_mut() {
        add_cx_fields(v);
    }
    let adds: Vec<(String, Value)> = m
        .iter()
        .filter(|(k, _)| ADDRESS_FIELDS.contains(&k.as_str()))
        .filter(|(k, _)| !m.contains_key(&format!("{k}_cx")))
        .filter_map(|(k, v)| cx_value(v).map(|c| (format!("{k}_cx"), c)))
        .collect();
    for (k, v) in adds {
        m.insert(k, v);
    }
}

/// Add `*_cx` siblings to every address field, recursively.
pub fn add_cx_fields(v: &mut Value) {
    match v {
        Value::Object(m) => add_object(m),
        Value::Array(a) => a.iter_mut().for_each(add_cx_fields),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn adds_cx_and_keeps_base58() {
        let k = "C1Epvwo91BCyeYr9o9dC2m7t4o5wuLUNSUeZxDQ4ePh7";
        let cx = "Cxa38030920a564589bDFe976BDB50f37CdeaECBBb34b3934e65b3f1a717bB7d06";
        let mut v = json!({"account": k, "solana_recipient": k, "enc_key": k, "balance": "5", "from": 3,
                           "txs": [{"from": k, "to": "short"}], "evaluators": [k, k], "miner_cx": "kept"});
        add_cx_fields(&mut v);
        assert_eq!(v["account"], k);
        assert_eq!(v["account_cx"], cx);
        assert!(v.get("solana_recipient_cx").is_none());
        assert!(v.get("enc_key_cx").is_none());
        assert!(v.get("from_cx").is_none(), "numbers are not addresses");
        assert_eq!(v["txs"][0]["from_cx"], cx);
        assert!(v["txs"][0].get("to_cx").is_none(), "not a 32-byte key");
        assert_eq!(v["evaluators_cx"], json!([cx, cx]));
    }
    #[test]
    fn rpc_params_take_cx_lowercase_base58_and_refuse_bad_checksum() {
        let k = crate::dispatch::parse32("C1Epvwo91BCyeYr9o9dC2m7t4o5wuLUNSUeZxDQ4ePh7").unwrap();
        assert_eq!(crate::dispatch::parse32("Cxa38030920a564589bDFe976BDB50f37CdeaECBBb34b3934e65b3f1a717bB7d06").unwrap(), k);
        assert_eq!(crate::dispatch::parse32("Cxa38030920a564589bdfe976bdb50f37cdeaecbbb34b3934e65b3f1a717bb7d06").unwrap(), k);
        let e = crate::dispatch::parse32("CxA38030920a564589bDFe976BDB50f37CdeaECBBb34b3934e65b3f1a717bB7d06").unwrap_err();
        assert_eq!(e.code, -32602);
        assert!(e.message.contains("checksum failed"), "{}", e.message);
    }
}
