//! Legacy Solana transactions, built by hand.

use anyhow::{ensure, Result};
use ed25519_dalek::{Signer, SigningKey};

use crate::key::{ed25519_program, pubkey_of, Pubkey};

#[derive(Clone, Debug)]
pub struct AccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl AccountMeta {
    pub fn w(pubkey: Pubkey) -> Self {
        Self { pubkey, is_signer: false, is_writable: true }
    }
    pub fn r(pubkey: Pubkey) -> Self {
        Self { pubkey, is_signer: false, is_writable: false }
    }
    pub fn signer_w(pubkey: Pubkey) -> Self {
        Self { pubkey, is_signer: true, is_writable: true }
    }
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub program_id: Pubkey,
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

fn compact_u16(out: &mut Vec<u8>, mut v: usize) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        b |= 0x80;
        out.push(b);
    }
}

/// Compile and sign a legacy transaction. `signers[0]` pays the fee.
pub fn build_signed(ixs: &[Instruction], signers: &[&SigningKey], blockhash: [u8; 32]) -> Result<Vec<u8>> {
    ensure!(!signers.is_empty(), "need a fee payer");
    let payer = pubkey_of(signers[0]);
    // (pubkey, signer, writable), payer first, merged by key.
    let mut keys: Vec<(Pubkey, bool, bool)> = vec![(payer, true, true)];
    let mut add = |pk: Pubkey, s: bool, w: bool| {
        if let Some(e) = keys.iter_mut().find(|e| e.0 == pk) {
            e.1 |= s;
            e.2 |= w;
        } else {
            keys.push((pk, s, w));
        }
    };
    for ix in ixs {
        for a in &ix.accounts {
            add(a.pubkey, a.is_signer, a.is_writable);
        }
    }
    for ix in ixs {
        add(ix.program_id, false, false);
    }
    let payer_entry = keys.remove(0);
    let rank = |e: &(Pubkey, bool, bool)| match (e.1, e.2) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    };
    keys.sort_by_key(rank); // stable: keeps first-seen order within a class
    keys.insert(0, payer_entry);
    let n_sig = keys.iter().filter(|e| e.1).count();
    let n_ro_signed = keys.iter().filter(|e| e.1 && !e.2).count();
    let n_ro_unsigned = keys.iter().filter(|e| !e.1 && !e.2).count();
    let signer_keys: Vec<Pubkey> = keys.iter().filter(|e| e.1).map(|e| e.0).collect();
    for sk in signers {
        ensure!(signer_keys.contains(&pubkey_of(sk)), "signer {} not referenced", pubkey_of(sk));
    }
    ensure!(signer_keys.len() == signers.len(), "{} signer accounts but {} keypairs", signer_keys.len(), signers.len());

    let idx = |pk: &Pubkey| keys.iter().position(|e| e.0 == *pk).unwrap() as u8;
    let mut msg = vec![n_sig as u8, n_ro_signed as u8, n_ro_unsigned as u8];
    compact_u16(&mut msg, keys.len());
    for e in &keys {
        msg.extend_from_slice(&e.0 .0);
    }
    msg.extend_from_slice(&blockhash);
    compact_u16(&mut msg, ixs.len());
    for ix in ixs {
        msg.push(idx(&ix.program_id));
        compact_u16(&mut msg, ix.accounts.len());
        for a in &ix.accounts {
            msg.push(idx(&a.pubkey));
        }
        compact_u16(&mut msg, ix.data.len());
        msg.extend_from_slice(&ix.data);
    }
    let mut tx = Vec::new();
    compact_u16(&mut tx, n_sig);
    for pk in &signer_keys {
        let sk = signers.iter().find(|s| pubkey_of(s) == *pk).unwrap();
        tx.extend_from_slice(&sk.sign(&msg).to_bytes());
    }
    tx.extend_from_slice(&msg);
    ensure!(tx.len() <= 1232, "transaction is {} bytes, over the 1232 limit", tx.len());
    Ok(tx)
}

/// Ed25519 precompile instruction carrying one (pubkey, signature, message), all
/// offsets pointing into this same instruction (index u16::MAX), as sigcheck requires.
pub fn ed25519_verify_ix(pubkey: &[u8; 32], signature: &[u8; 64], message: &[u8]) -> Instruction {
    const HDR: usize = 2 + 14;
    let pk_off = HDR;
    let sig_off = pk_off + 32;
    let msg_off = sig_off + 64;
    let mut d = Vec::with_capacity(msg_off + message.len());
    d.push(1u8);
    d.push(0u8);
    for v in [sig_off as u16, u16::MAX, pk_off as u16, u16::MAX, msg_off as u16, message.len() as u16, u16::MAX] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(pubkey);
    d.extend_from_slice(signature);
    d.extend_from_slice(message);
    Instruction { program_id: ed25519_program(), accounts: vec![], data: d }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact() {
        let mut v = Vec::new();
        compact_u16(&mut v, 0x7f);
        compact_u16(&mut v, 0x80);
        compact_u16(&mut v, 0x3fff);
        assert_eq!(v, vec![0x7f, 0x80, 0x01, 0xff, 0x7f]);
    }

    #[test]
    fn ed25519_layout() {
        let ix = ed25519_verify_ix(&[1; 32], &[2; 64], b"hello");
        assert_eq!(ix.data[0], 1);
        assert_eq!(u16::from_le_bytes([ix.data[2], ix.data[3]]), 48); // sig offset
        assert_eq!(u16::from_le_bytes([ix.data[6], ix.data[7]]), 16); // pubkey offset
        assert_eq!(u16::from_le_bytes([ix.data[10], ix.data[11]]), 112); // msg offset
        assert_eq!(&ix.data[112..], b"hello");
    }
}
