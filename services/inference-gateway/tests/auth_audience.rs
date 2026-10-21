//! cadre-auth-v2 (security pass V-9): a signature names the gateway's server key, so a
//! request signed for one gateway (say the test box) is refused by another (devnet)
//! inside the skew window. v1 stays accepted unless CADRE_AUTH_REQUIRE_AUDIENCE=1.

use cadre_gateway::{auth_message, auth_message_v2, now_ms, Auth, Keys};
use ed25519_dalek::{Signer, SigningKey};

fn headers(sk: &SigningKey, ts: u64, msg: &[u8]) -> Vec<(String, String)> {
    vec![
        ("X-Cadre-Wallet".into(), bs58::encode(sk.verifying_key().to_bytes()).into_string()),
        ("X-Cadre-Timestamp".into(), ts.to_string()),
        ("X-Cadre-Signature".into(), hex::encode(sk.sign(msg).to_bytes())),
    ]
}

fn gateway_keys(aud: &str, strict: bool) -> Keys {
    Keys { audience: Some(aud.into()), require_audience: strict, boot_ms: cadre_gateway::auth::BootMs(0), ..Keys::default() }
}

#[test]
fn v2_signature_is_bound_to_one_gateway() {
    let sk = SigningKey::from_bytes(&[4; 32]);
    let me = sk.verifying_key().to_bytes();
    let ts = now_ms();
    let (p, b) = ("/v1/chat/completions", b"{}");
    let h = headers(&sk, ts, &auth_message_v2("GatewayA", "POST", p, ts, b));
    // accepted by gateway A (lenient and strict)
    assert_eq!(gateway_keys("GatewayA", false).check("POST", p, &h, b, ts).unwrap(), Auth::Wallet(me));
    assert_eq!(gateway_keys("GatewayA", true).check("POST", p, &h, b, ts).unwrap(), Auth::Wallet(me));
    // replayed to gateway B: refused
    assert!(gateway_keys("GatewayB", false).check("POST", p, &h, b, ts).is_err());
    assert!(gateway_keys("GatewayB", true).check("POST", p, &h, b, ts).is_err());
}

#[test]
fn v1_signatures_only_when_the_gateway_is_lenient() {
    let sk = SigningKey::from_bytes(&[5; 32]);
    let ts = now_ms();
    let (p, b) = ("/v1/keys", b"");
    let h = headers(&sk, ts, &auth_message("POST", p, ts, b));
    assert!(gateway_keys("GatewayA", false).check("POST", p, &h, b, ts).is_ok());
    let e = gateway_keys("GatewayA", true).check("POST", p, &h, b, ts).unwrap_err().to_string();
    assert!(e.contains("cadre-auth-v2"), "{e}");
}
