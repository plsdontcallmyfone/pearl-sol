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
}

impl Sol {
    pub fn new(url: &str, payer: Keypair, cu_price: u64) -> Self {
        Self { rpc: RpcClient::new_with_commitment(url.to_string(), CommitmentConfig::confirmed()), payer, txs: 0, cu: 0, cu_price }
    }

    pub fn pk(&self) -> Pubkey {
        self.payer.pubkey()
    }

    pub fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> Result<()> {
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
            match self.rpc.send_and_confirm_transaction(&tx) {
                Ok(_) => {
                    self.txs += 1;
                    return Ok(());
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
