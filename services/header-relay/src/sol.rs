//! Solana side: send instructions (with a CU limit), read light-client accounts.

use anchor_lang::AccountDeserialize;
use anyhow::{anyhow, bail, Result};
use lightclient::client;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    compute_budget::ComputeBudgetInstruction,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};

pub struct Sol {
    pub rpc: RpcClient,
    pub payer: Keypair,
    pub txs: u64,
    pub cu: u64,
    /// micro-lamports per CU (priority fee); 0 = none
    pub cu_price: u64,
    /// Signature-status poll interval while confirming. 50 ms suits a local validator; a
    /// public RPC (Solana devnet: about 40 calls of one method per 10 s per IP) throttles
    /// that with 429 backoff, which stalls the relay, so the devnet runs with a few hundred ms.
    pub poll: std::time::Duration,
    /// Minimum spacing between sendTransaction calls (`--send-gap-ms`, default none). The
    /// pipelined relay sends every slot's chunk txs back to back; on Solana devnet's public
    /// RPC (about 40 sendTransaction per 10 s per IP) such bursts get 429 backoff that stalls
    /// whole passes, so the devnet paces sends at a few hundred ms.
    pub send_gap: std::time::Duration,
    last_send: std::sync::Mutex<Option<std::time::Instant>>,
    /// (blockhash, first seen) handed out by `fresh_blockhash`.
    bh_cache: Option<(solana_sdk::hash::Hash, std::time::Instant)>,
}

impl Sol {
    pub fn new(url: &str, payer: Keypair, cu_price: u64) -> Self {
        Self { rpc: RpcClient::new_with_commitment(url.to_string(), CommitmentConfig::confirmed()), payer, txs: 0, cu: 0, cu_price,
               poll: std::time::Duration::from_millis(50), send_gap: std::time::Duration::ZERO, last_send: std::sync::Mutex::new(None), bh_cache: None }
    }

    /// Sets the confirm poll interval (`--confirm-poll-ms`).
    pub fn with_poll(mut self, ms: u64) -> Self {
        self.poll = std::time::Duration::from_millis(ms.max(10));
        self
    }

    /// Sets the minimum spacing between sendTransaction calls (`--send-gap-ms`).
    pub fn with_send_gap(mut self, ms: u64) -> Self {
        self.send_gap = std::time::Duration::from_millis(ms);
        self
    }

    /// Waits until `send_gap` has passed since the previous sendTransaction.
    fn pace(&self) {
        if self.send_gap.is_zero() {
            return;
        }
        let mut last = self.last_send.lock().unwrap();
        if let Some(t) = *last {
            let since = t.elapsed();
            if since < self.send_gap {
                std::thread::sleep(self.send_gap - since);
            }
        }
        *last = Some(std::time::Instant::now());
    }

    pub fn pk(&self) -> Pubkey {
        self.payer.pubkey()
    }

    /// Sends one transaction and waits until it is confirmed; returns its signature.
    /// Polls every `poll` (default 50 ms; RpcClient::send_and_confirm_transaction sleeps 500 ms between
    /// status checks, which made one header cost tens of seconds on a local validator).
    pub fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> Result<String> {
        let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(1_400_000)];
        if self.cu_price > 0 {
            all.push(ComputeBudgetInstruction::set_compute_unit_price(self.cu_price));
        }
        all.extend_from_slice(ixs);
        let mut signers: Vec<&Keypair> = vec![&self.payer];
        signers.extend(extra.iter().copied().filter(|k| k.pubkey() != self.payer.pubkey()));
        let mut last = None;
        for _ in 0..3 {
            let bh = self.rpc.get_latest_blockhash()?;
            let tx = Transaction::new_signed_with_payer(&all, Some(&self.payer.pubkey()), &signers, bh);
            match self.send_confirm(&tx, &bh) {
                Ok(sig) => {
                    self.txs += 1;
                    return Ok(sig);
                }
                Err(e) => {
                    let s = e.to_string();
                    // program errors are final; transport / blockhash errors retry
                    if s.contains("custom program error") || s.contains("Error processing Instruction") {
                        bail!("{s}");
                    }
                    last = Some(s);
                }
            }
        }
        Err(anyhow!("send failed: {}", last.unwrap_or_default()))
    }

    /// Sends independent transactions (no ordering between them) back to back, then
    /// waits until all are confirmed. Used for the proof chunk uploads, which write
    /// disjoint ranges of the work account.
    pub fn send_batch(&mut self, txs: &[Vec<Instruction>]) -> Result<()> {
        if txs.is_empty() {
            return Ok(());
        }
        let bh = self.rpc.get_latest_blockhash()?;
        let mut sigs = Vec::with_capacity(txs.len());
        for ixs in txs {
            let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(200_000)];
            if self.cu_price > 0 {
                all.push(ComputeBudgetInstruction::set_compute_unit_price(self.cu_price));
            }
            all.extend_from_slice(ixs);
            let tx = Transaction::new_signed_with_payer(&all, Some(&self.payer.pubkey()), &[&self.payer], bh);
            self.pace();
            sigs.push(self.rpc.send_transaction(&tx)?);
        }
        let t0 = std::time::Instant::now();
        let mut left: Vec<_> = sigs.clone();
        while !left.is_empty() {
            let st = self.rpc.get_signature_statuses(&left)?.value;
            let mut next = vec![];
            for (sig, s) in left.iter().zip(st) {
                match s {
                    Some(s) if s.err.is_some() => bail!("batch tx {sig} failed: {:?}", s.err),
                    Some(s) if s.satisfies_commitment(CommitmentConfig::confirmed()) => {}
                    _ => next.push(*sig),
                }
            }
            left = next;
            if !left.is_empty() {
                if t0.elapsed() > std::time::Duration::from_secs(60) {
                    bail!("{} batch txs unconfirmed after 60 s", left.len());
                }
                std::thread::sleep(self.poll);
            }
        }
        self.txs += txs.len() as u64;
        Ok(())
    }

    fn send_confirm(&self, tx: &Transaction, bh: &solana_sdk::hash::Hash) -> Result<String> {
        self.pace();
        let sig = self.rpc.send_transaction(tx)?;
        let t0 = std::time::Instant::now();
        loop {
            let st = self.rpc.get_signature_statuses(&[sig])?.value.into_iter().next().flatten();
            if let Some(st) = st {
                if let Some(e) = st.err {
                    bail!("transaction {sig} failed: Error processing Instruction: {e}");
                }
                if st.satisfies_commitment(CommitmentConfig::confirmed()) {
                    return Ok(sig.to_string());
                }
            } else if t0.elapsed() > std::time::Duration::from_secs(5) && !self.rpc.is_blockhash_valid(bh, CommitmentConfig::processed())? {
                bail!("transaction {sig} expired unconfirmed");
            }
            std::thread::sleep(self.poll);
        }
    }

    pub fn account(&self, k: &Pubkey) -> Result<Option<Vec<u8>>> {
        match self.rpc.get_account_with_commitment(k, CommitmentConfig::confirmed())?.value {
            Some(a) if a.lamports > 0 => Ok(Some(a.data)),
            _ => Ok(None),
        }
    }

    pub fn config(&self) -> Result<lightclient::Config> {
        let d = self.account(&client::config_pda())?.ok_or_else(|| anyhow!("light client not initialized"))?;
        Ok(lightclient::Config::try_deserialize(&mut d.as_slice())?)
    }

    pub fn canon(&self) -> Result<Vec<[u8; 32]>> {
        let d = self.account(&client::canon_pda())?.ok_or_else(|| anyhow!("no canon account"))?;
        Ok(d[8..].chunks(32).map(|c| c.try_into().unwrap()).collect())
    }

    /// (status, is_final, relayer) of a header account.
    pub fn header_state(&self, hash: &[u8; 32]) -> Result<Option<(u8, bool, Pubkey)>> {
        Ok(self.account(&client::header_addr(hash))?.map(|d| {
            (d[lightclient::layout::STATUS], d[lightclient::layout::IS_FINAL] == 1, Pubkey::new_from_array(d[424..456].try_into().unwrap()))
        }))
    }
}

impl crate::ledger::Ledger for Sol {
    fn payer(&self) -> &Keypair {
        &self.payer
    }
    fn cu_price(&self) -> u64 {
        self.cu_price
    }
    fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut out = Vec::with_capacity(keys.len());
        for c in keys.chunks(100) {
            let v = self.rpc.get_multiple_accounts_with_commitment(c, CommitmentConfig::confirmed())?.value;
            out.extend(v.into_iter().map(|a| a.filter(|a| a.lamports > 0).map(|a| a.data)));
        }
        Ok(out)
    }
    fn blockhash(&self) -> Result<solana_sdk::hash::Hash> {
        Ok(self.rpc.get_latest_blockhash()?)
    }
    fn fresh_blockhash(&mut self, _bh: solana_sdk::hash::Hash) -> Result<solana_sdk::hash::Hash> {
        let now = std::time::Instant::now();
        if let Some((c, t)) = self.bh_cache {
            if now.duration_since(t) < std::time::Duration::from_secs(15) {
                return Ok(c);
            }
        }
        let fresh = self.rpc.get_latest_blockhash()?;
        self.bh_cache = Some((fresh, now));
        Ok(fresh)
    }
    fn blockhash_valid(&self, h: &solana_sdk::hash::Hash) -> Result<bool> {
        Ok(self.rpc.is_blockhash_valid(h, CommitmentConfig::processed())?)
    }
    fn submit(&mut self, tx: Transaction) -> Result<solana_sdk::signature::Signature> {
        let cfg = solana_client::rpc_config::RpcSendTransactionConfig { skip_preflight: true, ..Default::default() };
        self.pace();
        let sig = self.rpc.send_transaction_with_config(&tx, cfg)?;
        self.txs += 1;
        Ok(sig)
    }
    fn states(&self, sigs: &[solana_sdk::signature::Signature]) -> Result<Vec<crate::ledger::TxState>> {
        use crate::ledger::TxState;
        let mut out = Vec::with_capacity(sigs.len());
        for c in sigs.chunks(256) {
            for s in self.rpc.get_signature_statuses(c)?.value {
                out.push(match s {
                    Some(s) if s.err.is_some() => TxState::Failed(format!("{:?}", s.err.unwrap())),
                    Some(s) if s.satisfies_commitment(CommitmentConfig::confirmed()) => TxState::Ok,
                    _ => TxState::Pending,
                });
            }
        }
        Ok(out)
    }
    fn rent(&self, size: usize) -> Result<u64> {
        Ok(self.rpc.get_minimum_balance_for_rent_exemption(size)?)
    }
    fn pause(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}
