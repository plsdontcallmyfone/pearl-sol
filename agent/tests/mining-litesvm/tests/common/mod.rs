#![allow(dead_code)]
//! Shared LiteSVM harness for the agents, jobs and mining programs.

use anchor_lang::{InstructionData, ToAccountMetas};
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_program, sysvar,
    transaction::Transaction,
};

pub const SOL: u64 = 1_000_000_000;
pub const START_SLOT: u64 = 1_000;

pub fn so(name: &str) -> String {
    format!("{}/../../target/deploy/{name}.so", env!("CARGO_MANIFEST_DIR"))
}

pub struct Env {
    pub svm: LiteSVM,
    pub admin: Keypair,
    pub attester: Keypair,
}

pub type TxResult = Result<TransactionMetadata, FailedTransactionMetadata>;

impl Env {
    /// Programs loaded, nothing initialized.
    pub fn bare() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program_from_file(agents::ID, so("agents")).unwrap();
        svm.add_program_from_file(jobs::ID, so("jobs")).unwrap();
        svm.add_program_from_file(mining::ID, so("mining")).unwrap();
        svm.warp_to_slot(START_SLOT);
        let admin = Keypair::new();
        svm.airdrop(&admin.pubkey(), 100 * SOL).unwrap();
        Env { svm, admin, attester: Keypair::new() }
    }

    /// Programs loaded, both configs initialized, the test attester set in both.
    pub fn new() -> Self {
        Self::with_mining(mining::ConfigParams {
            epoch_slots: Some(100),
            ..Default::default()
        })
    }

    pub fn with_mining(mut mp: mining::ConfigParams) -> Self {
        let mut env = Self::bare();
        let att = env.attester.pubkey();
        mp.attesters = Some(vec![att]);
        let admin = env.admin.insecure_clone();
        let ix = Instruction {
            program_id: mining::ID,
            accounts: mining::accounts::Initialize {
                admin: admin.pubkey(),
                config: mining_config(),
                mining_pool: pool(),
                epoch: epoch_pda(0),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mining::instruction::Initialize { params: mp }.data(),
        };
        env.send(&[ix], &admin, &[]).unwrap();
        let ix = Instruction {
            program_id: jobs::ID,
            accounts: jobs::accounts::InitializeConfig {
                admin: admin.pubkey(),
                config: jobs_config(),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: jobs::instruction::InitializeConfig {
                params: jobs::ConfigParams {
                    evaluators: Some(vec![att]),
                    ..Default::default()
                },
            }
            .data(),
        };
        env.send(&[ix], &admin, &[]).unwrap();
        env
    }

    pub fn user(&mut self, lamports: u64) -> Keypair {
        let k = Keypair::new();
        self.svm.airdrop(&k.pubkey(), lamports).unwrap();
        k
    }

    pub fn send(&mut self, ixs: &[Instruction], payer: &Keypair, extra: &[&Keypair]) -> TxResult {
        let mut signers: Vec<&Keypair> = vec![payer];
        signers.extend_from_slice(extra);
        let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), &signers, self.svm.latest_blockhash());
        let r = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        r
    }

    pub fn slot(&self) -> u64 {
        self.svm.get_sysvar::<solana_sdk::clock::Clock>().slot
    }

    pub fn warp(&mut self, slot: u64) {
        self.svm.warp_to_slot(slot);
    }

    pub fn balance(&self, k: &Pubkey) -> u64 {
        self.svm.get_balance(k).unwrap_or(0)
    }

    pub fn account<T: anchor_lang::AccountDeserialize>(&self, k: &Pubkey) -> T {
        let a = self.svm.get_account(k).expect("account missing");
        T::try_deserialize(&mut a.data.as_slice()).unwrap()
    }

    pub fn exists(&self, k: &Pubkey) -> bool {
        self.svm.get_account(k).map(|a| a.lamports > 0).unwrap_or(false)
    }

    /// Add lamports to the mining pool (stands in for `fund_mining` via the bridge).
    pub fn fund_pool(&mut self, lamports: u64) {
        let mut a = self.svm.get_account(&pool()).unwrap();
        a.lamports += lamports;
        self.svm.set_account(pool(), a).unwrap();
    }

    pub fn rent(&self, len: usize) -> u64 {
        self.svm.minimum_balance_for_rent_exemption(len)
    }

    // ---------- jobs ----------

    pub fn post_job(&mut self, poster: &Keypair, p: jobs::PostJobParams) -> TxResult {
        let ix = post_job_ix(&poster.pubkey(), p);
        self.send(&[ix], poster, &[])
    }

    /// Post a default job (W, P) with a deadline `ttl` slots out. Returns its PDA.
    pub fn post_simple(&mut self, poster: &Keypair, job_id: u64, w: u64, p: u64, ttl: u64) -> Pubkey {
        let params = job_params(job_id, w, p, self.slot(), self.slot() + ttl);
        self.post_job(poster, params).unwrap();
        job_pda(&poster.pubkey(), job_id)
    }

    pub fn submit_result(&mut self, deliverer: &Keypair, job: Pubkey, hash: [u8; 32]) -> TxResult {
        let ix = Instruction {
            program_id: jobs::ID,
            accounts: jobs::accounts::SubmitResult { deliverer: deliverer.pubkey(), job }.to_account_metas(None),
            data: jobs::instruction::SubmitResult {
                result_hash: hash,
                result_uri: "file:///tmp/c.bin".into(),
            }
            .data(),
        };
        self.send(&[ix], deliverer, &[])
    }

    pub fn complete_ix(&self, job: Pubkey, deliverer: Pubkey) -> Instruction {
        Instruction {
            program_id: jobs::ID,
            accounts: jobs::accounts::Complete {
                config: jobs_config(),
                job,
                deliverer,
                instructions: sysvar::instructions::ID,
            }
            .to_account_metas(None),
            data: jobs::instruction::Complete {}.data(),
        }
    }

    // ---------- mining ----------

    pub fn share_ixs(&self, signer: &Keypair, job: Pubkey, miner: Pubkey, nonce: u64) -> Vec<Instruction> {
        let msg = mining::share_message(&job, &miner, nonce);
        vec![ed25519_ix(signer, &msg), self.submit_share_ix(job, miner, nonce)]
    }

    pub fn submit_share_ix(&self, job: Pubkey, miner: Pubkey, nonce: u64) -> Instruction {
        let cfg: mining::MiningConfig = self.account(&mining_config());
        let e = cfg.current_epoch;
        Instruction {
            program_id: mining::ID,
            accounts: mining::accounts::SubmitShare {
                miner,
                config: mining_config(),
                epoch: epoch_pda(e),
                job,
                job_stats: job_stats_pda(&job),
                job_epoch: job_epoch_pda(e, &job),
                shares: shares_pda(e, &job, &miner),
                nonces: nonces_pda(&job, &miner),
                instructions: sysvar::instructions::ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mining::instruction::SubmitShare { nonce }.data(),
        }
    }

    pub fn submit_share(&mut self, miner: &Keypair, job: Pubkey, nonce: u64) -> TxResult {
        let att = self.attester.insecure_clone();
        let ixs = self.share_ixs(&att, job, miner.pubkey(), nonce);
        self.send(&ixs, miner, &[])
    }

    pub fn advance_epoch(&mut self, payer: &Keypair) -> TxResult {
        let cfg: mining::MiningConfig = self.account(&mining_config());
        let e = cfg.current_epoch;
        let ix = Instruction {
            program_id: mining::ID,
            accounts: mining::accounts::AdvanceEpoch {
                payer: payer.pubkey(),
                config: mining_config(),
                mining_pool: pool(),
                current_epoch: epoch_pda(e),
                next_epoch: epoch_pda(e + 1),
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: mining::instruction::AdvanceEpoch {}.data(),
        };
        self.send(&[ix], payer, &[])
    }

    pub fn claim(&mut self, miner: &Keypair, epoch: u64, job: Pubkey) -> TxResult {
        let m = miner.pubkey();
        let ix = Instruction {
            program_id: mining::ID,
            accounts: mining::accounts::Claim {
                miner: m,
                config: mining_config(),
                mining_pool: pool(),
                epoch: epoch_pda(epoch),
                job,
                job_stats: job_stats_pda(&job),
                job_epoch: job_epoch_pda(epoch, &job),
                shares: shares_pda(epoch, &job, &m),
            }
            .to_account_metas(None),
            data: mining::instruction::Claim { epoch_index: epoch }.data(),
        };
        self.send(&[ix], miner, &[])
    }

    pub fn mining_cfg(&self) -> mining::MiningConfig {
        self.account(&mining_config())
    }

    pub fn epoch(&self, i: u64) -> mining::Epoch {
        self.account(&epoch_pda(i))
    }

    /// Lamports the pool can still promise: balance - rent - outstanding.
    pub fn pool_free(&self) -> u64 {
        let a = self.svm.get_account(&pool()).unwrap();
        a.lamports - self.rent(a.data.len()) - self.mining_cfg().outstanding
    }
}

pub fn ed25519_ix(signer: &Keypair, msg: &[u8]) -> Instruction {
    let sig = signer.sign_message(msg);
    let sig: [u8; 64] = sig.as_ref().try_into().unwrap();
    solana_ed25519_program::new_ed25519_instruction_with_signature(msg, &sig, &signer.pubkey().to_bytes())
}

pub fn post_job_ix(poster: &Pubkey, p: jobs::PostJobParams) -> Instruction {
    Instruction {
        program_id: jobs::ID,
        accounts: jobs::accounts::PostJob {
            poster: *poster,
            config: jobs_config(),
            job: job_pda(poster, p.job_id),
            mining_pool: pool(),
            system_program: system_program::ID,
        }
        .to_account_metas(None),
        data: jobs::instruction::PostJob { p }.data(),
    }
}

pub fn job_params(job_id: u64, w: u64, p: u64, created_slot: u64, deadline: u64) -> jobs::PostJobParams {
    jobs::PostJobParams {
        job_id,
        hash_a: [0xAA; 32],
        hash_b: [0xBB; 32],
        config: [7u8; 52],
        m: 64,
        n: 64,
        k: 1024,
        data_uri: "file:///tmp/job/manifest.json".into(),
        worker_pay: w,
        protocol_fee: p,
        created_slot,
        deadline_slot: deadline,
        share_target: 0x207f_ffff,
    }
}

pub fn pda(seeds: &[&[u8]], prog: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(seeds, prog).0
}
pub fn mining_config() -> Pubkey {
    pda(&[mining::CONFIG_SEED], &mining::ID)
}
pub fn pool() -> Pubkey {
    pda(&[mining::POOL_SEED], &mining::ID)
}
pub fn epoch_pda(i: u64) -> Pubkey {
    pda(&[mining::EPOCH_SEED, &i.to_le_bytes()], &mining::ID)
}
pub fn job_stats_pda(job: &Pubkey) -> Pubkey {
    pda(&[mining::JOB_STATS_SEED, job.as_ref()], &mining::ID)
}
pub fn job_epoch_pda(e: u64, job: &Pubkey) -> Pubkey {
    pda(&[mining::JOB_EPOCH_SEED, &e.to_le_bytes(), job.as_ref()], &mining::ID)
}
pub fn shares_pda(e: u64, job: &Pubkey, miner: &Pubkey) -> Pubkey {
    pda(&[mining::SHARES_SEED, &e.to_le_bytes(), job.as_ref(), miner.as_ref()], &mining::ID)
}
pub fn nonces_pda(job: &Pubkey, miner: &Pubkey) -> Pubkey {
    pda(&[mining::NONCES_SEED, job.as_ref(), miner.as_ref()], &mining::ID)
}
pub fn jobs_config() -> Pubkey {
    pda(&[jobs::JOBS_CONFIG_SEED], &jobs::ID)
}
pub fn job_pda(poster: &Pubkey, id: u64) -> Pubkey {
    pda(&[jobs::JOB_SEED, poster.as_ref(), &id.to_le_bytes()], &jobs::ID)
}
pub fn agent_pda(auth: &Pubkey) -> Pubkey {
    pda(&[agents::AGENT_SEED, auth.as_ref()], &agents::ID)
}

/// Anchor custom error code in a failed tx, if any.
pub fn err_code(r: &TxResult) -> Option<u32> {
    use solana_sdk::instruction::InstructionError;
    use solana_sdk::transaction::TransactionError;
    match r {
        Err(f) => match &f.err {
            TransactionError::InstructionError(_, InstructionError::Custom(c)) => Some(*c),
            _ => None,
        },
        Ok(_) => None,
    }
}

/// (instruction index, custom code) of a failed tx. A precompile failure shows up
/// at the precompile's own index.
pub fn err_at(r: &TxResult) -> Option<(u8, u32)> {
    use solana_sdk::instruction::InstructionError;
    use solana_sdk::transaction::TransactionError;
    match r {
        Err(f) => match &f.err {
            TransactionError::InstructionError(i, InstructionError::Custom(c)) => Some((*i, *c)),
            _ => None,
        },
        Ok(_) => None,
    }
}

pub fn logs(r: &TxResult) -> String {
    match r {
        Ok(m) => m.logs.join("\n"),
        Err(f) => format!("{:?}\n{}", f.err, f.meta.logs.join("\n")),
    }
}
