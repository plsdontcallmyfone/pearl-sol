//! Instruction builders and chain helpers for the jobs / mining programs.

use anyhow::{anyhow, Result};
use ed25519_dalek::SigningKey;

use crate::accounts::{ix_discriminator, EpochAccount, JobAccount, JobStatus, MiningConfigAccount, JOB_STATUS_OFFSET};
use crate::key::{find_program_address, pubkey_of, sysvar_instructions, Pubkey, SYSTEM_PROGRAM};
use crate::rpc::Rpc;
use crate::tx::{build_signed, ed25519_verify_ix, AccountMeta, Instruction};

#[derive(Clone, Copy, Debug)]
pub struct Programs {
    pub jobs: Pubkey,
    pub mining: Pubkey,
}

impl Default for Programs {
    fn default() -> Self {
        Self {
            jobs: Pubkey::from_b58(crate::JOBS_PROGRAM).unwrap(),
            mining: Pubkey::from_b58(crate::MINING_PROGRAM).unwrap(),
        }
    }
}

impl Programs {
    pub fn mining_config(&self) -> Pubkey {
        find_program_address(&[b"config"], &self.mining).0
    }
    pub fn mining_pool(&self) -> Pubkey {
        find_program_address(&[b"mining_pool"], &self.mining).0
    }
    pub fn epoch(&self, index: u64) -> Pubkey {
        find_program_address(&[b"epoch", &index.to_le_bytes()], &self.mining).0
    }
    pub fn jobs_config(&self) -> Pubkey {
        find_program_address(&[b"jobs_config"], &self.jobs).0
    }

    pub fn submit_share_ix(&self, miner: Pubkey, job: Pubkey, epoch_index: u64, nonce: u64) -> Instruction {
        let e = epoch_index.to_le_bytes();
        let m = &self.mining;
        let mut data = ix_discriminator("submit_share").to_vec();
        data.extend_from_slice(&nonce.to_le_bytes());
        Instruction {
            program_id: *m,
            accounts: vec![
                AccountMeta::signer_w(miner),
                AccountMeta::r(self.mining_config()),
                AccountMeta::w(self.epoch(epoch_index)),
                AccountMeta::r(job),
                AccountMeta::w(find_program_address(&[b"job_stats", &job.0], m).0),
                AccountMeta::w(find_program_address(&[b"job_epoch", &e, &job.0], m).0),
                AccountMeta::w(find_program_address(&[b"shares", &e, &job.0, &miner.0], m).0),
                AccountMeta::w(find_program_address(&[b"nonces", &job.0, &miner.0], m).0),
                AccountMeta::r(sysvar_instructions()),
                AccountMeta::r(SYSTEM_PROGRAM),
            ],
            data,
        }
    }

    pub fn advance_epoch_ix(&self, payer: Pubkey, current: u64) -> Instruction {
        Instruction {
            program_id: self.mining,
            accounts: vec![
                AccountMeta::signer_w(payer),
                AccountMeta::w(self.mining_config()),
                AccountMeta::r(self.mining_pool()),
                AccountMeta::w(self.epoch(current)),
                AccountMeta::w(self.epoch(current + 1)),
                AccountMeta::r(SYSTEM_PROGRAM),
            ],
            data: ix_discriminator("advance_epoch").to_vec(),
        }
    }

    pub fn submit_result_ix(&self, deliverer: Pubkey, job: Pubkey, result_hash: [u8; 32], result_uri: &str) -> Instruction {
        let mut data = ix_discriminator("submit_result").to_vec();
        data.extend_from_slice(&result_hash);
        data.extend_from_slice(&(result_uri.len() as u32).to_le_bytes());
        data.extend_from_slice(result_uri.as_bytes());
        Instruction {
            program_id: self.jobs,
            accounts: vec![AccountMeta { pubkey: deliverer, is_signer: true, is_writable: false }, AccountMeta::w(job)],
            data,
        }
    }

    pub fn complete_ix(&self, job: Pubkey, deliverer: Pubkey) -> Instruction {
        Instruction {
            program_id: self.jobs,
            accounts: vec![
                AccountMeta::r(self.jobs_config()),
                AccountMeta::w(job),
                AccountMeta::w(deliverer),
                AccountMeta::r(sysvar_instructions()),
            ],
            data: ix_discriminator("complete").to_vec(),
        }
    }

    pub fn claim_ix(&self, miner: Pubkey, job: Pubkey, epoch: u64) -> Instruction {
        let m = &self.mining;
        let e = epoch.to_le_bytes();
        let mut data = ix_discriminator("claim").to_vec();
        data.extend_from_slice(&e);
        Instruction {
            program_id: *m,
            accounts: vec![
                AccountMeta::signer_w(miner),
                AccountMeta::w(self.mining_config()),
                AccountMeta::w(self.mining_pool()),
                AccountMeta::w(self.epoch(epoch)),
                AccountMeta::r(job),
                AccountMeta::w(find_program_address(&[b"job_stats", &job.0], m).0),
                AccountMeta::w(find_program_address(&[b"job_epoch", &e, &job.0], m).0),
                AccountMeta::w(find_program_address(&[b"shares", &e, &job.0, &miner.0], m).0),
            ],
            data,
        }
    }

    pub fn open_jobs(&self, rpc: &Rpc) -> Result<Vec<JobAccount>> {
        let disc = crate::accounts::account_discriminator("Job").to_vec();
        let accs = rpc.get_program_accounts(&self.jobs, &[(0, disc), (JOB_STATUS_OFFSET, vec![0u8])])?;
        let mut out = Vec::new();
        for (pk, a) in accs {
            match JobAccount::decode(pk, &a.data) {
                Ok(j) if j.status == JobStatus::Open => out.push(j),
                Ok(_) => {}
                Err(e) => eprintln!("skip job {pk}: {e:#}"),
            }
        }
        Ok(out)
    }

    pub fn get_job(&self, rpc: &Rpc, job: &Pubkey) -> Result<JobAccount> {
        let a = rpc.get_account(job)?.ok_or_else(|| anyhow!("job {job} not found"))?;
        anyhow::ensure!(a.owner == self.jobs, "job {job} is not owned by the jobs program");
        JobAccount::decode(*job, &a.data)
    }

    pub fn get_mining_config(&self, rpc: &Rpc) -> Result<MiningConfigAccount> {
        let a = rpc.get_account(&self.mining_config())?.ok_or_else(|| anyhow!("mining config not initialised"))?;
        MiningConfigAccount::decode(&a.data)
    }

    pub fn get_epoch(&self, rpc: &Rpc, index: u64) -> Result<EpochAccount> {
        let a = rpc.get_account(&self.epoch(index))?.ok_or_else(|| anyhow!("epoch {index} not found"))?;
        EpochAccount::decode(&a.data)
    }

    /// Advance the epoch if the current one is over. Returns the current index after.
    pub fn ensure_epoch(&self, rpc: &Rpc, payer: &SigningKey) -> Result<u64> {
        let cfg = self.get_mining_config(rpc)?;
        let ep = self.get_epoch(rpc, cfg.current_epoch)?;
        if rpc.get_slot()? >= ep.end_slot {
            let ix = self.advance_epoch_ix(pubkey_of(payer), cfg.current_epoch);
            let tx = build_signed(&[ix], &[payer], rpc.latest_blockhash()?)?;
            rpc.send_and_confirm(&tx)?;
            return Ok(cfg.current_epoch + 1);
        }
        Ok(cfg.current_epoch)
    }

    /// ed25519 precompile (attester signature over the Share message) + submit_share.
    pub fn send_share(
        &self,
        rpc: &Rpc,
        miner: &SigningKey,
        job: Pubkey,
        nonce: u64,
        attester: &[u8; 32],
        signature: &[u8; 64],
    ) -> Result<String> {
        let mpk = pubkey_of(miner);
        let epoch = self.ensure_epoch(rpc, miner)?;
        let msg = pearl_job::messages::share_message(&job.0, &mpk.0, nonce);
        let ixs = [ed25519_verify_ix(attester, signature, &msg), self.submit_share_ix(mpk, job, epoch, nonce)];
        let tx = build_signed(&ixs, &[miner], rpc.latest_blockhash()?)?;
        rpc.send_and_confirm(&tx)
    }

    pub fn send_submit_result(&self, rpc: &Rpc, deliverer: &SigningKey, job: Pubkey, result_hash: [u8; 32], uri: &str) -> Result<String> {
        let ix = self.submit_result_ix(pubkey_of(deliverer), job, result_hash, uri);
        let tx = build_signed(&[ix], &[deliverer], rpc.latest_blockhash()?)?;
        rpc.send_and_confirm(&tx)
    }

    /// ed25519 precompile (attester signature over the Result message) + complete.
    pub fn send_complete(
        &self,
        rpc: &Rpc,
        payer: &SigningKey,
        job: Pubkey,
        deliverer: Pubkey,
        result_hash: &[u8; 32],
        attester: &[u8; 32],
        signature: &[u8; 64],
    ) -> Result<String> {
        let msg = pearl_job::messages::result_message(&job.0, &deliverer.0, result_hash);
        let ixs = [ed25519_verify_ix(attester, signature, &msg), self.complete_ix(job, deliverer)];
        let tx = build_signed(&ixs, &[payer], rpc.latest_blockhash()?)?;
        rpc.send_and_confirm(&tx)
    }
}
