//! cadre-auth-v1 replay guard (security pass 2026-10-21, finding V-3).

use cadre_gateway::{auth_message, now_ms, Auth, Keys};
use ed25519_dalek::{Signer, SigningKey};

fn signed(sk: &SigningKey, method: &str, path: &str, body: &[u8], ts: u64) -> Vec<(String, String)> {
    let sig = sk.sign(&auth_message(method, path, ts, body));
    vec![
        ("X-Cadre-Wallet".into(), bs58::encode(sk.verifying_key().to_bytes()).into_string()),
        ("X-Cadre-Timestamp".into(), ts.to_string()),
        ("X-Cadre-Signature".into(), hex::encode(sig.to_bytes())),
    ]
}

/// Exploit before the fix: the seen-signature list is not saved, so a request captured
/// shortly before a gateway restart was accepted again by the restarted gateway (still
/// inside the 120 s skew window). Now a signature older than the process is refused.
#[test]
fn a_request_signed_before_a_restart_is_not_replayable() {
    let sk = SigningKey::from_bytes(&[9; 32]);
    let ts = now_ms();
    let h = signed(&sk, "POST", "/v1/chat/completions", b"{}", ts);
    let mut before = Keys::default();
    assert_eq!(before.check("POST", "/v1/chat/completions", &h, b"{}", ts).unwrap(), Auth::Wallet(sk.verifying_key().to_bytes()));
    std::thread::sleep(std::time::Duration::from_millis(5));
    // the gateway restarts: its keys are loaded from ledger.json, the replay guard is empty
    let json = serde_json::to_string(&before).unwrap();
    let mut after: Keys = serde_json::from_str(&json).unwrap();
    let e = after.check("POST", "/v1/chat/completions", &h, b"{}", now_ms()).unwrap_err().to_string();
    assert!(e.contains("before the gateway started"), "{e}");
    // a fresh signature works
    let h2 = signed(&sk, "POST", "/v1/chat/completions", b"{}", now_ms());
    assert!(after.check("POST", "/v1/chat/completions", &h2, b"{}", now_ms()).is_ok());
}

#[test]
fn the_seen_set_expires_with_the_queue() {
    let sk = SigningKey::from_bytes(&[3; 32]);
    let mut k = Keys::default();
    let t0 = now_ms();
    for i in 0..500u64 {
        let h = signed(&sk, "GET", "/v1/keys", format!("{i}").as_bytes(), t0);
        k.check("GET", "/v1/keys", &h, format!("{i}").as_bytes(), t0).unwrap();
        assert!(k.check("GET", "/v1/keys", &h, format!("{i}").as_bytes(), t0).unwrap_err().to_string().contains("already used"));
    }
    assert_eq!((k.seen.len(), k.seen_set.len()), (500, 500));
    // 2 x the skew later every entry is dropped from both
    let later = t0 + 2 * cadre_gateway::auth::MAX_SKEW_MS + 1;
    let h = signed(&sk, "GET", "/v1/keys", b"late", later);
    k.check("GET", "/v1/keys", &h, b"late", later).unwrap();
    assert_eq!((k.seen.len(), k.seen_set.len()), (1, 1));
}
