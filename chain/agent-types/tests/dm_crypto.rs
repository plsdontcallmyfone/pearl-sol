//! Reference implementation of the sealed DM formats (SPEC 4.10) and the published
//! cross-language vectors `vectors/dm-crypto.json` that the TypeScript wallet, site and
//! agent runtime check against (X25519 + HKDF-SHA256 + XChaCha20-Poly1305).
//!
//! `UPDATE_VECTORS=1 cargo test -p agent-types --test dm_crypto` rewrites the file; a plain
//! run checks it byte for byte and opens every envelope again.
//!
//! The chain never runs this code: the executor checks framing only (agent_types::dm).

use agent_types::dm::{check_sealed_channel, check_sealed_dm, KEY_ENVELOPE_LEN};
use agent_types::tx::pubkey_from_seed;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

type HmacSha256 = Hmac<Sha256>;

const DM_LABEL: &[u8] = b"cadre-dm-v1";
const CHAN_LABEL: &[u8] = b"cadre-chan-v1";

/// HKDF-SHA256 (RFC 5869) with a 32-byte output (one expand block).
fn hkdf32(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut ext = <HmacSha256 as Mac>::new_from_slice(salt).unwrap();
    ext.update(ikm);
    let prk = ext.finalize().into_bytes();
    let mut exp = <HmacSha256 as Mac>::new_from_slice(&prk).unwrap();
    exp.update(info);
    exp.update(&[1]);
    exp.finalize().into_bytes().into()
}

/// The X25519 secret a wallet derives from its ed25519 seed: HKDF-SHA256(ikm = seed,
/// salt = "cadre-dm-v1", info = "x25519 enc_key"). enc_key is its public key.
fn enc_secret(ed_seed: &[u8; 32]) -> [u8; 32] {
    hkdf32(ed_seed, DM_LABEL, b"x25519 enc_key")
}

fn x_pub(sk: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*sk)).to_bytes()
}

fn x_dh(sk: &[u8; 32], pk: &[u8; 32]) -> [u8; 32] {
    StaticSecret::from(*sk).diffie_hellman(&PublicKey::from(*pk)).to_bytes()
}

fn aead_seal(key: &[u8; 32], nonce: &[u8; 24], pt: &[u8], aad: &[u8]) -> Vec<u8> {
    let c = XChaCha20Poly1305::new_from_slice(key).unwrap();
    c.encrypt(&XNonce::from(*nonce), Payload { msg: pt, aad }).unwrap()
}

fn aead_open(key: &[u8; 32], nonce: &[u8; 24], ct: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    let c = XChaCha20Poly1305::new_from_slice(key).unwrap();
    c.decrypt(&XNonce::from(*nonce), Payload { msg: ct, aad }).ok()
}

fn dm_info(from: &[u8; 32], to: &[u8; 32]) -> Vec<u8> {
    [DM_LABEL, from, to].concat()
}

/// One DM envelope to `recipient_enc`: `eph_pub | nonce | ct_len u16 | ct`.
fn dm_envelope(eph_sk: &[u8; 32], nonce: &[u8; 24], recipient_enc: &[u8; 32], from: &[u8; 32], to: &[u8; 32], pt: &[u8]) -> Vec<u8> {
    let eph_pub = x_pub(eph_sk);
    let info = dm_info(from, to);
    let key = hkdf32(&x_dh(eph_sk, recipient_enc), &[&eph_pub[..], recipient_enc].concat(), &info);
    let ct = aead_seal(&key, nonce, pt, &info);
    let mut v = eph_pub.to_vec();
    v.extend(nonce);
    v.extend((ct.len() as u16).to_le_bytes());
    v.extend(ct);
    v
}

/// Open the sealed DM `body` with the reader's X25519 secret: tries each envelope.
fn open_dm(body: &[u8], reader_sk: &[u8; 32], from: &[u8; 32], to: &[u8; 32]) -> Option<Vec<u8>> {
    let n = check_sealed_dm(body).ok()?;
    let reader_pub = x_pub(reader_sk);
    let info = dm_info(from, to);
    let mut pos = 2;
    for _ in 0..n {
        let eph: [u8; 32] = body[pos..pos + 32].try_into().unwrap();
        let nonce: [u8; 24] = body[pos + 32..pos + 56].try_into().unwrap();
        let len = u16::from_le_bytes([body[pos + 56], body[pos + 57]]) as usize;
        let ct = &body[pos + 58..pos + 58 + len];
        pos += 58 + len;
        let key = hkdf32(&x_dh(reader_sk, &eph), &[&eph[..], &reader_pub[..]].concat(), &info);
        if let Some(pt) = aead_open(&key, &nonce, ct, &info) {
            return Some(pt);
        }
    }
    None
}

fn chan_key_info(channel: &[u8; 32], epoch: u32, member: &[u8; 32]) -> Vec<u8> {
    [CHAN_LABEL, channel, &epoch.to_le_bytes(), member].concat()
}

fn chan_msg_aad(channel: &[u8; 32], from: &[u8; 32], epoch: u32) -> Vec<u8> {
    [CHAN_LABEL, channel, from, &epoch.to_le_bytes()].concat()
}

fn key_envelope(eph_sk: &[u8; 32], nonce: &[u8; 24], member_enc: &[u8; 32], channel: &[u8; 32], epoch: u32, member: &[u8; 32], group_key: &[u8; 32]) -> Vec<u8> {
    let eph_pub = x_pub(eph_sk);
    let info = chan_key_info(channel, epoch, member);
    let key = hkdf32(&x_dh(eph_sk, member_enc), &[&eph_pub[..], member_enc].concat(), &info);
    let ct = aead_seal(&key, nonce, group_key, &info);
    [eph_pub.to_vec(), nonce.to_vec(), ct].concat()
}

fn open_key_envelope(env: &[u8], member_sk: &[u8; 32], channel: &[u8; 32], epoch: u32, member: &[u8; 32]) -> Option<[u8; 32]> {
    let eph: [u8; 32] = env[..32].try_into().unwrap();
    let nonce: [u8; 24] = env[32..56].try_into().unwrap();
    let info = chan_key_info(channel, epoch, member);
    let key = hkdf32(&x_dh(member_sk, &eph), &[&eph[..], &x_pub(member_sk)[..]].concat(), &info);
    aead_open(&key, &nonce, &env[56..], &info).map(|v| v.try_into().unwrap())
}

fn seed(i: u8) -> [u8; 32] {
    agent_types::blake3_hash(&[b"pearl-sol:vector-seed", &[i]])
}

fn fixed(tag: &str) -> [u8; 32] {
    agent_types::blake3_hash(&[b"pearl-sol:dm-vector", tag.as_bytes()])
}

fn nonce_of(tag: &str) -> [u8; 24] {
    fixed(tag)[..24].try_into().unwrap()
}

fn b58(k: &[u8]) -> String {
    bs58::encode(k).into_string()
}

fn build() -> Value {
    let (alice_seed, bob_seed, carol_seed) = (seed(1), seed(2), seed(4));
    let (alice, bob, carol) = (pubkey_from_seed(&alice_seed), pubkey_from_seed(&bob_seed), pubkey_from_seed(&carol_seed));
    let (a_sk, b_sk, c_sk) = (enc_secret(&alice_seed), enc_secret(&bob_seed), enc_secret(&carol_seed));
    let (a_enc, b_enc, c_enc) = (x_pub(&a_sk), x_pub(&b_sk), x_pub(&c_sk));

    // sealed DM alice -> bob with a self copy
    let text = "Meet me at the harbour desk, caf\u{e9} at dawn.".as_bytes();
    let e0 = dm_envelope(&fixed("dm-eph-0"), &nonce_of("dm-nonce-0"), &b_enc, &alice, &bob, text);
    let e1 = dm_envelope(&fixed("dm-eph-1"), &nonce_of("dm-nonce-1"), &a_enc, &alice, &bob, text);
    let mut body = vec![1u8, 2];
    body.extend(&e0);
    body.extend(&e1);
    assert_eq!(check_sealed_dm(&body), Ok(2));
    assert_eq!(open_dm(&body, &b_sk, &alice, &bob).as_deref(), Some(text), "recipient opens envelope 0");
    assert_eq!(open_dm(&body, &a_sk, &alice, &bob).as_deref(), Some(text), "sender opens its self copy");
    assert_eq!(open_dm(&body, &c_sk, &alice, &bob), None, "a third key opens nothing");
    assert_eq!(open_dm(&body, &b_sk, &alice, &carol), None, "the from/to binding is checked");
    let mut tampered = body.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert_eq!(open_dm(&tampered, &a_sk, &alice, &bob), None, "a flipped tag bit fails");

    // sealed channel: epoch 1 group key to alice and carol, a message from carol
    let channel = agent_types::dm::channel_id(&alice, 8);
    let gk = fixed("group-key-epoch-1");
    let ka = key_envelope(&fixed("ck-eph-a"), &nonce_of("ck-nonce-a"), &a_enc, &channel, 1, &alice, &gk);
    let kc = key_envelope(&fixed("ck-eph-c"), &nonce_of("ck-nonce-c"), &c_enc, &channel, 1, &carol, &gk);
    assert_eq!(ka.len(), KEY_ENVELOPE_LEN);
    assert_eq!(open_key_envelope(&ka, &a_sk, &channel, 1, &alice), Some(gk));
    assert_eq!(open_key_envelope(&kc, &c_sk, &channel, 1, &carol), Some(gk));
    assert_eq!(open_key_envelope(&kc, &b_sk, &channel, 1, &carol), None, "bob is not a member");
    assert_eq!(open_key_envelope(&kc, &c_sk, &channel, 2, &carol), None, "the epoch is bound");
    let ctext = "Bids on GPU hours open at 40.".as_bytes();
    let cn = nonce_of("chan-msg-nonce");
    let aad = chan_msg_aad(&channel, &carol, 1);
    let mut cbody = vec![2u8];
    cbody.extend(1u32.to_le_bytes());
    cbody.extend(cn);
    cbody.extend(aead_seal(&gk, &cn, ctext, &aad));
    assert_eq!(check_sealed_channel(&cbody), Ok(1));
    assert_eq!(aead_open(&gk, &cn, &cbody[29..], &aad).as_deref(), Some(ctext));
    assert_eq!(aead_open(&fixed("group-key-epoch-2"), &cn, &cbody[29..], &aad), None, "another epoch's key fails");

    json!({
        "format": "cadre-dm-crypto-v1",
        "scheme": {
            "enc_secret": "HKDF-SHA256(ikm = ed25519 seed, salt = \"cadre-dm-v1\", info = \"x25519 enc_key\", 32); enc_key = X25519(enc_secret, 9)",
            "dm_key": "HKDF-SHA256(ikm = X25519(eph, R), salt = eph_pub || R, info = \"cadre-dm-v1\" || from || to, 32)",
            "dm_aead": "XChaCha20-Poly1305(dm_key, nonce, plaintext, aad = info)",
            "dm_body": "0x01 | count u8 (1..=2) | (eph_pub [32] | nonce [24] | ct_len u16 LE | ct)*; envelope 0 to the recipient, 1 to the sender",
            "chan_key_key": "HKDF-SHA256(ikm = X25519(eph, M), salt = eph_pub || M, info = \"cadre-chan-v1\" || channel || epoch u32 LE || member, 32)",
            "chan_key_envelope": "eph_pub [32] | nonce [24] | XChaCha20-Poly1305(chan_key_key, nonce, group_key, aad = info) [48]",
            "chan_body": "0x02 | epoch u32 LE | nonce [24] | XChaCha20-Poly1305(group_key, nonce, plaintext, aad = \"cadre-chan-v1\" || channel || from || epoch u32 LE)",
        },
        "accounts": [
            {"name": "alice", "seed": hex::encode(alice_seed), "account": b58(&alice), "enc_secret": hex::encode(a_sk), "enc_key": b58(&a_enc), "enc_key_hex": hex::encode(a_enc)},
            {"name": "bob", "seed": hex::encode(bob_seed), "account": b58(&bob), "enc_secret": hex::encode(b_sk), "enc_key": b58(&b_enc), "enc_key_hex": hex::encode(b_enc)},
            {"name": "carol", "seed": hex::encode(carol_seed), "account": b58(&carol), "enc_secret": hex::encode(c_sk), "enc_key": b58(&c_enc), "enc_key_hex": hex::encode(c_enc)},
        ],
        "hkdf_check": {"ikm": "00".repeat(32), "salt": hex::encode(DM_LABEL), "info": hex::encode(b"x25519 enc_key"), "okm": hex::encode(hkdf32(&[0; 32], DM_LABEL, b"x25519 enc_key"))},
        "sealed_dm": {
            "from": b58(&alice), "to": b58(&bob), "plaintext": String::from_utf8_lossy(text),
            "eph_secrets": [hex::encode(fixed("dm-eph-0")), hex::encode(fixed("dm-eph-1"))],
            "nonces": [hex::encode(nonce_of("dm-nonce-0")), hex::encode(nonce_of("dm-nonce-1"))],
            "body_hex": hex::encode(&body),
        },
        "sealed_channel": {
            "channel_id": hex::encode(channel), "epoch": 1, "group_key": hex::encode(gk),
            "envelopes": [
                {"member": b58(&alice), "eph_secret": hex::encode(fixed("ck-eph-a")), "nonce": hex::encode(nonce_of("ck-nonce-a")), "envelope_hex": hex::encode(&ka)},
                {"member": b58(&carol), "eph_secret": hex::encode(fixed("ck-eph-c")), "nonce": hex::encode(nonce_of("ck-nonce-c")), "envelope_hex": hex::encode(&kc)},
            ],
            "message": {"from": b58(&carol), "plaintext": String::from_utf8_lossy(ctext), "nonce": hex::encode(cn), "body_hex": hex::encode(&cbody)},
        },
    })
}

#[test]
fn dm_crypto_vectors() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors").join("dm-crypto.json");
    let text = serde_json::to_string_pretty(&build()).unwrap() + "\n";
    if std::env::var("UPDATE_VECTORS").is_ok() || !path.exists() {
        std::fs::write(&path, &text).unwrap();
    } else {
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "dm-crypto.json drifted; rerun with UPDATE_VECTORS=1 if intended");
    }
}

#[test]
fn enc_key_never_equals_signing_key() {
    for i in 0..16u8 {
        let s = seed(i);
        assert_ne!(x_pub(&enc_secret(&s)), pubkey_from_seed(&s));
    }
}
