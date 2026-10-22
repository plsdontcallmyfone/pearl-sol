//! Shared Cx address vectors (vectors/cx-address.json at the repo root, written by
//! packages/cx-address/scripts/gen-vectors.ts): the Rust and TS implementations must agree.

use agent_types::cx::*;
use serde_json::Value;

fn vectors() -> Value {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors/cx-address.json");
    serde_json::from_str(&std::fs::read_to_string(p).expect("vectors/cx-address.json")).unwrap()
}

fn key(hex_s: &str) -> [u8; 32] {
    hex::decode(hex_s).unwrap().try_into().unwrap()
}

#[test]
fn encode_short_base58() {
    for v in vectors()["valid"].as_array().unwrap() {
        let k = key(v["hex"].as_str().unwrap());
        assert_eq!(to_cx(&k), v["cx"].as_str().unwrap(), "{}", v["note"]);
        assert_eq!(to_cx(&k).len(), CX_LEN);
        assert_eq!(short_cx(&k), v["short"].as_str().unwrap());
        assert_eq!(to_base58(&k), v["base58"].as_str().unwrap());
    }
}

#[test]
fn round_trip_all_accepted_forms() {
    for v in vectors()["valid"].as_array().unwrap() {
        let k = key(v["hex"].as_str().unwrap());
        for f in ["cx", "cx_lowercase", "base58"] {
            assert_eq!(parse_address(v[f].as_str().unwrap()).unwrap(), k, "{f} {}", v["note"]);
        }
        assert_eq!(display_cx(v["cx_lowercase"].as_str().unwrap()).unwrap(), v["cx"].as_str().unwrap());
        assert_eq!(short_address(v["base58"].as_str().unwrap()), v["short"].as_str().unwrap());
        assert_eq!(parse_address(&format!(" {}\n", v["cx"].as_str().unwrap())).unwrap(), k);
    }
}

#[test]
fn rejects_with_codes() {
    for v in vectors()["invalid"].as_array().unwrap() {
        let e = parse_address(v["input"].as_str().unwrap()).unwrap_err();
        assert_eq!(e.code(), v["error"].as_str().unwrap(), "{}", v["note"]);
        assert!(!is_address(v["input"].as_str().unwrap()));
    }
    assert!(CxError::Checksum.to_string().contains("checksum failed"));
    assert_eq!(short_address("nope"), "nope");
}

#[test]
fn owner_example() {
    let k = parse_address("C1Epvwo91BCyeYr9o9dC2m7t4o5wuLUNSUeZxDQ4ePh7").unwrap();
    assert_eq!(to_cx(&k), "Cxa38030920a564589bDFe976BDB50f37CdeaECBBb34b3934e65b3f1a717bB7d06");
    assert_eq!(short_cx(&k), "Cxa380...7d06");
}
