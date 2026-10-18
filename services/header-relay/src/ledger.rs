//! The Solana side the relay pipeline needs, as a trait: batched account reads, fire
//! and forget sends, batched signature status polls. `sol::Sol` implements it over RPC;
//! the catch-up test (tests/catchup.rs) implements it over LiteSVM with simulated slots.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    hash::Hash,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    transaction::Transaction,
};

/// Compute units a transaction may ask for.
pub const MAX_CU: u32 = 1_400_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxState {
    /// Not seen yet, or not yet confirmed.
    Pending,
    /// Confirmed without error.
    Ok,
    /// Landed with an error (or was refused).
    Failed(String),
}

pub trait Ledger {
    fn payer(&self) -> &Keypair;
    fn pk(&self) -> Pubkey {
        self.payer().pubkey()
    }
    /// Micro-lamports per CU (priority fee), 0 = none.
    fn cu_price(&self) -> u64;
    /// Accounts at confirmed commitment (None = missing or empty), in order.
    fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<Vec<u8>>>>;
    fn account(&self, key: &Pubkey) -> Result<Option<Vec<u8>>> {
        Ok(self.accounts(&[*key])?.pop().flatten())
    }
    fn blockhash(&self) -> Result<Hash>;
    fn blockhash_valid(&self, h: &Hash) -> Result<bool>;
    /// Send without waiting (no preflight); the signature is tracked with `states`.
    fn submit(&mut self, tx: Transaction) -> Result<Signature>;
    fn states(&self, sigs: &[Signature]) -> Result<Vec<TxState>>;
    fn rent(&self, size: usize) -> Result<u64>;
    /// Wait a little (RPC: sleep; simulation: one slot passes).
    fn pause(&mut self, ms: u64);
}

/// A signed transaction: CU limit (and price) first, then `ixs`.
pub fn build<L: Ledger + ?Sized>(l: &L, ixs: &[Instruction], extra: &[&Keypair], cu: u32, bh: Hash) -> Transaction {
    let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(cu)];
    if l.cu_price() > 0 {
        all.push(ComputeBudgetInstruction::set_compute_unit_price(l.cu_price()));
    }
    all.extend_from_slice(ixs);
    let mut signers: Vec<&Keypair> = vec![l.payer()];
    signers.extend(extra.iter().copied().filter(|k| k.pubkey() != l.pk()));
    Transaction::new_signed_with_payer(&all, Some(&l.pk()), &signers, bh)
}

/// Send one transaction and wait until it is confirmed. Program errors are final;
/// an expired blockhash is retried (3 attempts).
pub fn send_wait<L: Ledger + ?Sized>(l: &mut L, ixs: &[Instruction], extra: &[&Keypair], cu: u32) -> Result<Signature> {
    let mut last = String::new();
    for _ in 0..3 {
        let bh = l.blockhash()?;
        let sig = l.submit(build(l, ixs, extra, cu, bh))?;
        let t0 = Instant::now();
        loop {
            match l.states(&[sig])?.pop().unwrap_or(TxState::Pending) {
                TxState::Ok => return Ok(sig),
                TxState::Failed(e) => bail!("transaction {sig} failed: {e}"),
                TxState::Pending => {}
            }
            if t0.elapsed() > Duration::from_secs(20) && !l.blockhash_valid(&bh)? {
                last = format!("transaction {sig} expired unconfirmed");
                break;
            }
            l.pause(50);
        }
    }
    Err(anyhow!("send failed: {last}"))
}
