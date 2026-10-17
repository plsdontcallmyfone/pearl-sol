//! SPEC section 6 signed message formats (ed25519, all little-endian).

pub const SHARE_TAG: &[u8] = b"pearl-sol:share:v1";
pub const RESULT_TAG: &[u8] = b"pearl-sol:result:v1";
pub const DEPOSIT_TAG: &[u8] = b"pearl-sol:deposit:v1";
pub const WITHDRAW_TAG: &[u8] = b"pearl-sol:withdraw:v1";

/// `b"pearl-sol:share:v1" || job [32] || miner [32] || nonce u64` (90 bytes).
pub fn share_message(job: &[u8; 32], miner: &[u8; 32], nonce: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(SHARE_TAG.len() + 72);
    v.extend_from_slice(SHARE_TAG);
    v.extend_from_slice(job);
    v.extend_from_slice(miner);
    v.extend_from_slice(&nonce.to_le_bytes());
    v
}

/// `b"pearl-sol:result:v1" || job [32] || deliverer [32] || result_hash [32]` (115 bytes).
pub fn result_message(job: &[u8; 32], deliverer: &[u8; 32], result_hash: &[u8; 32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(RESULT_TAG.len() + 96);
    v.extend_from_slice(RESULT_TAG);
    v.extend_from_slice(job);
    v.extend_from_slice(deliverer);
    v.extend_from_slice(result_hash);
    v
}

/// Commonware `sign(namespace, msg)` framing: `varint(len ns) || ns || msg`
/// (unsigned LEB128 length). The v2 chain verifies every signature over these bytes.
pub fn frame(ns: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + ns.len() + msg.len());
    let mut x = ns.len() as u64;
    loop {
        let b = (x & 0x7f) as u8;
        x >>= 7;
        if x == 0 {
            v.push(b);
            break;
        }
        v.push(b | 0x80);
    }
    v.extend_from_slice(ns);
    v.extend_from_slice(msg);
    v
}

/// v2 (own chain) Result message, the bytes an evaluator signs for CompleteJob:
/// `frame(b"pearl-sol:result:v1", job_id || deliverer || result_hash)`, that is
/// `0x13 ||` the v1 [`result_message`] (116 bytes).
pub fn result_message_v2(job: &[u8; 32], deliverer: &[u8; 32], result_hash: &[u8; 32]) -> Vec<u8> {
    frame(RESULT_TAG, &[&job[..], &deliverer[..], &result_hash[..]].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_layouts() {
        let m = share_message(&[1; 32], &[2; 32], 0x0102030405060708);
        assert_eq!(m.len(), 18 + 32 + 32 + 8);
        assert_eq!(&m[..18], b"pearl-sol:share:v1");
        assert_eq!(&m[18..50], &[1; 32]);
        assert_eq!(&m[50..82], &[2; 32]);
        assert_eq!(&m[82..], &[8, 7, 6, 5, 4, 3, 2, 1]);
        let r = result_message(&[1; 32], &[2; 32], &[3; 32]);
        assert_eq!(r.len(), 19 + 96);
        assert_eq!(&r[..19], b"pearl-sol:result:v1");
        assert_eq!(&r[83..], &[3; 32]);
        let r2 = result_message_v2(&[1; 32], &[2; 32], &[3; 32]);
        assert_eq!(r2[0], 19);
        assert_eq!(&r2[1..], &r[..]);
    }
}
