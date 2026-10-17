//! Shared helpers for the light-client LiteSVM tests (tests/ holds the tests):
//! a native chain generator (headers mined with crates/pearl-job, WTEMA via
//! crates/pearl-verify-sbf), agent-chain tx builders, a tx_root builder, and an
//! in-process relay that drives the light client exactly like services/header-relay.

use anchor_lang::{AccountDeserialize, AccountSerialize, InstructionData, ToAccountMetas};
use lightclient::client::{self, Step};
use lightclient::{layout, Params};
use litesvm::LiteSVM;
use pearl_job::pow::{self, BlockHeader, BlockWork, FallbackParams};
use pearl_job::PreparedJob;
use pearl_verify_sbf as pv;
use solana_sdk::{
    account::Account,
    clock::Clock,
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction, InstructionError},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction, system_program,
    transaction::{Transaction, TransactionError},
};

pub const T0: u64 = 1_760_000_000_000;
pub const CHAIN_ID: u32 = 1347616769;
pub const PROOF_CAP: u32 = 32 * 1024;
/// k of the localnet config (`pearl_job::MiningConfig::localnet_default`, r = 128).
pub const LOCAL_K: u32 = 2048;
pub const ED25519: Pubkey = solana_sdk::pubkey!("Ed25519SigVerify111111111111111111111111111");
pub const IX_SYSVAR: Pubkey = solana_sdk::pubkey!("Sysvar1nstructions1111111111111111111111111");
pub const TOKEN_2022: Pubkey = solana_sdk::pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ATA_PROGRAM: Pubkey = solana_sdk::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

pub fn params(k: u32) -> Params {
    Params {
        chain_id: CHAIN_ID,
        finality_k: k,
        target_block_time_ms: 10_000,
        half_life_ms: 600_000,
        pow_limit_bits: 0x207fffff,
        min_difficulty_bits: 0,
        max_future_ms: 120_000,
        fallback_m: 64,
        fallback_n: 64,
        fallback_config: pearl_job::MiningConfig::localnet_default().to_bytes(),
        relay_reward: 10_000,
        retain_final: 2,
    }
}

pub fn fallback_params(p: &Params) -> FallbackParams {
    FallbackParams { m: p.fallback_m, n: p.fallback_n, config: pearl_job::MiningConfig::from_bytes(&p.fallback_config).unwrap() }
}

// ---------------------------------------------------------------- chain generator

#[derive(Clone, Debug)]
pub struct Blk {
    pub h: BlockHeader,
    pub bytes: [u8; 256],
    pub hash: [u8; 32],
    pub proof: Vec<u8>,
    pub ts_hist: Vec<u64>,
    pub cum_work: pv::U256,
}

impl Blk {
    /// A harvested header (node getHeader) with its proof.
    pub fn from_wire(bytes: [u8; 256], proof: Vec<u8>, cum_work: pv::U256, ts_hist: Vec<u64>) -> Self {
        let h = BlockHeader::from_bytes(&bytes).unwrap();
        Blk { h, bytes, hash: h.block_hash(), proof, ts_hist, cum_work }
    }

    pub fn job_id(&self) -> Option<[u8; 32]> {
        if self.h.is_fallback() { None } else { Some(self.h.job_id) }
    }
}

pub fn genesis(bits: u32) -> Blk {
    let h = BlockHeader { version: 1, height: 0, timestamp_ms: T0, target_bits: bits, ..Default::default() };
    let bytes = h.to_bytes();
    Blk { h, bytes, hash: h.block_hash(), proof: vec![], ts_hist: vec![T0], cum_work: pv::difficulty::work(bits) }
}

pub fn required_bits(p: &Params, parent: &Blk) -> u32 {
    pv::difficulty::next_bits(&pv::difficulty::Retarget {
        parent_bits: parent.h.target_bits,
        parent_ts_ms: parent.h.timestamp_ms,
        grandparent_ts_ms: if parent.h.height == 0 { None } else { Some(parent.ts_hist[1]) },
        target_block_time_ms: p.target_block_time_ms,
        half_life_ms: p.half_life_ms,
        pow_limit_bits: p.pow_limit_bits,
        min_difficulty_bits: p.min_difficulty_bits,
    })
}

pub enum Work<'a> {
    Fallback,
    Job(&'a PreparedJob, [u8; 32]),
}

pub struct ChildOpts {
    pub dt: u64,
    pub tx_root: [u8; 32],
    pub withdraw_root: [u8; 32],
    pub miner: u8,
    /// Override the WTEMA target (to build invalid headers).
    pub bits: Option<u32>,
    pub ts: Option<u64>,
}

impl Default for ChildOpts {
    fn default() -> Self {
        Self { dt: 10_000, tx_root: [0; 32], withdraw_root: [0; 32], miner: 0x4d, bits: None, ts: None }
    }
}

pub fn child(p: &Params, parent: &Blk, work: &Work, o: ChildOpts) -> Blk {
    let bits = o.bits.unwrap_or_else(|| required_bits(p, parent));
    let ts = o.ts.unwrap_or(parent.h.timestamp_ms + o.dt);
    let t = BlockHeader {
        version: 1,
        height: parent.h.height + 1,
        prev_hash: parent.hash,
        timestamp_ms: ts,
        target_bits: bits,
        job_id: match work {
            Work::Fallback => [0; 32],
            Work::Job(_, id) => *id,
        },
        miner: [o.miner; 32],
        tx_root: o.tx_root,
        state_root: [2; 32],
        withdraw_root: o.withdraw_root,
        proof_hash: [0; 32],
        nonce: 0,
    };
    let bw = match work {
        Work::Fallback => BlockWork::Fallback(fallback_params(p)),
        Work::Job(prep, _) => BlockWork::Job(prep),
    };
    let mined = pow::mine_header(&t, &bw, 0..1_000_000).unwrap().expect("mined");
    let mut ts_hist = vec![ts];
    ts_hist.extend(parent.ts_hist.iter().take(10));
    let bytes = mined.header.to_bytes();
    Blk {
        h: mined.header,
        bytes,
        hash: mined.header.block_hash(),
        proof: mined.proof,
        ts_hist,
        cum_work: parent.cum_work.saturating_add(pv::difficulty::work(bits)),
    }
}

/// A losing proof (tile 0) for a nonce that does not meet the target.
pub fn losing_block(p: &Params, parent: &Blk) -> Blk {
    let bits = required_bits(p, parent);
    let fp = fallback_params(p);
    for nonce in 0..10_000u64 {
        let mut h = BlockHeader {
            version: 1,
            height: parent.h.height + 1,
            prev_hash: parent.hash,
            timestamp_ms: parent.h.timestamp_ms + 10_000,
            target_bits: bits,
            miner: [0x4d; 32],
            state_root: [2; 32],
            nonce,
            ..Default::default()
        };
        let pd = h.pow_digest();
        let fj = pow::fallback_job(&pd, &fp).unwrap();
        let rows: Vec<usize> = (0..4).collect();
        let cols: Vec<usize> = (0..8).collect();
        let proof = pow::block_proof_to_bytes(&fj.build_proof(nonce, &rows, &cols));
        h.proof_hash = pow::proof_hash(&proof);
        if pow::verify_block(&h, None, &proof, &fp).is_err() {
            let mut ts_hist = vec![h.timestamp_ms];
            ts_hist.extend(parent.ts_hist.iter().take(10));
            return Blk { h, bytes: h.to_bytes(), hash: h.block_hash(), proof, ts_hist, cum_work: parent.cum_work };
        }
    }
    panic!("no losing nonce")
}

// ---------------------------------------------------------------- agent-chain txs

pub fn tx_bytes(from: &[u8; 32], nonce: u64, kind: u8, body: &[u8]) -> Vec<u8> {
    let mut t = vec![1u8];
    t.extend_from_slice(&CHAIN_ID.to_le_bytes());
    t.extend_from_slice(from);
    t.extend_from_slice(&nonce.to_le_bytes());
    t.extend_from_slice(&1000u64.to_le_bytes());
    t.push(kind);
    t.extend_from_slice(body);
    t.extend_from_slice(&[0u8; 64]); // the light client does not check the tx signature
    t
}

pub struct PostJob {
    pub m: u32,
    pub n: u32,
    pub config: [u8; 52],
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    pub worker_pay: u64,
    pub protocol_fee: u64,
    pub deadline: u64,
    pub max_blocks: u32,
    pub evaluators: Vec<[u8; 32]>,
}

impl PostJob {
    pub fn body(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&self.m.to_le_bytes());
        b.extend_from_slice(&self.n.to_le_bytes());
        b.extend_from_slice(&u32::from_le_bytes(self.config[0..4].try_into().unwrap()).to_le_bytes());
        b.extend_from_slice(&self.config);
        b.extend_from_slice(&self.hash_a);
        b.extend_from_slice(&self.hash_b);
        let uri = b"blob:test";
        b.extend_from_slice(&(uri.len() as u16).to_le_bytes());
        b.extend_from_slice(uri);
        b.extend_from_slice(&self.worker_pay.to_le_bytes());
        b.extend_from_slice(&self.protocol_fee.to_le_bytes());
        b.extend_from_slice(&self.deadline.to_le_bytes());
        b.extend_from_slice(&self.max_blocks.to_le_bytes());
        b.push(self.evaluators.len() as u8);
        for e in &self.evaluators {
            b.extend_from_slice(e);
        }
        b.push(1);
        b
    }
}

pub fn post_job_ids(poster: &[u8; 32], nonce: u64, m: u32, n: u32, w: u64, p: u64, deadline: u64, max_blocks: u32) -> ([u8; 32], [u8; 32]) {
    let job_id = *blake3::hash(&[&poster[..], &nonce.to_le_bytes()].concat()).as_bytes();
    let mut t = b"pearl-sol:terms:v2".to_vec();
    t.extend_from_slice(poster);
    t.extend_from_slice(&job_id);
    for x in [m, n, LOCAL_K] {
        t.extend_from_slice(&x.to_le_bytes());
    }
    for x in [w, p, deadline] {
        t.extend_from_slice(&x.to_le_bytes());
    }
    t.extend_from_slice(&max_blocks.to_le_bytes());
    (job_id, *blake3::hash(&t).as_bytes())
}

/// chain/agent-types/src/merkle.rs: returns (root, path for each tx) with 33-byte entries.
pub fn tx_root(txs: &[Vec<u8>]) -> ([u8; 32], Vec<Vec<u8>>) {
    if txs.is_empty() {
        return ([0; 32], vec![]);
    }
    let leaf = |h: &[u8; 32]| *blake3::hash(&[&[0u8][..], h].concat()).as_bytes();
    let node = |l: &[u8; 32], r: &[u8; 32]| *blake3::hash(&[&[1u8][..], l, r].concat()).as_bytes();
    let mut level: Vec<[u8; 32]> = txs.iter().map(|t| leaf(blake3::hash(t).as_bytes())).collect();
    let mut paths = vec![Vec::new(); txs.len()];
    let mut idx: Vec<usize> = (0..txs.len()).collect();
    while level.len() > 1 {
        for (t, i) in idx.iter_mut().enumerate() {
            let sib = *i ^ 1;
            if sib < level.len() {
                paths[t].extend_from_slice(&level[sib]);
                paths[t].push((sib < *i) as u8);
            }
            *i /= 2;
        }
        level = level.chunks(2).map(|c| if c.len() == 2 { node(&c[0], &c[1]) } else { c[0] }).collect();
    }
    (level[0], paths)
}

// ---------------------------------------------------------------- SVM env

pub fn custom(code: u32) -> impl Fn(&TransactionError) -> bool {
    move |e| matches!(e, TransactionError::InstructionError(_, InstructionError::Custom(c)) if *c == code)
}

pub fn lc_code(e: lightclient::LcError) -> u32 {
    u32::from(e)
}

pub struct Env {
    pub svm: LiteSVM,
    pub payer: Keypair,
    pub work: Keypair,
    pub p: Params,
    /// (label, CU) of every successful tx sent through `send_labeled`.
    pub cu_log: Vec<(String, u64)>,
    pub txs_sent: u64,
}

pub fn program_path(name: &str) -> String {
    format!("{}/../../target/deploy/{name}.so", env!("CARGO_MANIFEST_DIR"))
}

impl Env {
    pub fn new(p: Params, checkpoint: &Blk) -> Self {
        Self::new_at(p, checkpoint, (T0 / 1000) as i64 + 1_000_000)
    }

    /// `clock_s`: Solana unix time (header timestamps may not exceed it + max_future).
    pub fn new_at(p: Params, checkpoint: &Blk, clock_s: i64) -> Self {
        let mut svm = LiteSVM::new();
        let so = std::fs::read(program_path("lightclient")).expect("build first: cd token && anchor build -p lightclient");
        svm.add_program(lightclient::ID, &so);
        let mut clock: Clock = svm.get_sysvar();
        clock.unix_timestamp = clock_s;
        svm.set_sysvar(&clock);
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
        let mut env = Env { svm, payer, work: Keypair::new(), p, cu_log: vec![], txs_sent: 0 };
        let init = client::initialize(env.payer.pubkey(), p, checkpoint.bytes, pv::difficulty::u256_le(&checkpoint.cum_work), checkpoint.ts_hist.clone());
        env.send(&[init], &[]).expect("initialize");
        // R5: the final-header accumulator starts at the checkpoint
        env.send(&[client::init_accumulator(env.payer.pubkey())], &[]).expect("init_accumulator");
        // fund the relay reward pool
        let fund = system_instruction::transfer(&env.payer.pubkey(), &client::pool_pda(), 1_000_000_000);
        env.send(&[fund], &[]).expect("fund pool");
        env.new_work_account();
        env
    }

    pub fn new_work_account(&mut self) {
        let size = client::work_size_for(&self.p);
        let rent = self.svm.minimum_balance_for_rent_exemption(size);
        let work = self.work.insecure_clone();
        let create = system_instruction::create_account(&self.payer.pubkey(), &work.pubkey(), rent, size as u64, &lightclient::ID);
        let init = client::init_work(self.payer.pubkey(), work.pubkey(), PROOF_CAP);
        self.send(&[create, init], &[&work]).expect("work account");
    }

    pub fn add_program(&mut self, id: Pubkey, name: &str) {
        let so = std::fs::read(program_path(name)).unwrap_or_else(|_| panic!("build first: anchor build -p {name}"));
        self.svm.add_program(id, &so);
    }

    pub fn send(&mut self, ixs: &[Instruction], extra: &[&Keypair]) -> Result<u64, TransactionError> {
        self.send_labeled("", ixs, extra)
    }

    /// Sends with a 1.4M CU limit; returns CU consumed.
    pub fn send_labeled(&mut self, label: &str, ixs: &[Instruction], extra: &[&Keypair]) -> Result<u64, TransactionError> {
        let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(1_400_000)];
        all.extend_from_slice(ixs);
        let mut signers: Vec<&Keypair> = vec![&self.payer];
        for k in extra {
            if k.pubkey() != self.payer.pubkey() {
                signers.push(k);
            }
        }
        let tx = Transaction::new_signed_with_payer(&all, Some(&self.payer.pubkey()), &signers, self.svm.latest_blockhash());
        let size = bincode_len(&tx);
        assert!(size <= 1232, "tx too large: {size} bytes ({label})");
        let r = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        self.txs_sent += 1;
        match r {
            Ok(m) => {
                if !label.is_empty() {
                    self.cu_log.push((label.to_string(), m.compute_units_consumed));
                }
                Ok(m.compute_units_consumed)
            }
            Err(f) => {
                if std::env::var("LC_LOGS").is_ok() {
                    for l in &f.meta.logs {
                        eprintln!("  log: {l}");
                    }
                }
                Err(f.err)
            }
        }
    }

    /// Send and return `(CU, return data)`.
    pub fn send_return(&mut self, ixs: &[Instruction]) -> Result<(u64, Vec<u8>), TransactionError> {
        let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(1_400_000)];
        all.extend_from_slice(ixs);
        let tx = Transaction::new_signed_with_payer(&all, Some(&self.payer.pubkey()), &[&self.payer], self.svm.latest_blockhash());
        assert!(bincode_len(&tx) <= 1232, "tx too large");
        let r = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        match r {
            Ok(m) => Ok((m.compute_units_consumed, m.return_data.data)),
            Err(f) => {
                if std::env::var("LC_LOGS").is_ok() {
                    for l in &f.meta.logs {
                        eprintln!("  log: {l}");
                    }
                }
                Err(f.err)
            }
        }
    }

    /// Raw accumulator `(start_height, count, last_hash)`.
    pub fn acc(&self) -> (u64, u64, [u8; 32]) {
        client::acc_state(&self.svm.get_account(&client::acc_addr()).unwrap().data).unwrap()
    }

    pub fn logs_of(&mut self, ixs: &[Instruction]) -> Vec<String> {
        let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(1_400_000)];
        all.extend_from_slice(ixs);
        let tx = Transaction::new_signed_with_payer(&all, Some(&self.payer.pubkey()), &[&self.payer], self.svm.latest_blockhash());
        let r = self.svm.send_transaction(tx);
        self.svm.expire_blockhash();
        match r {
            Ok(m) => m.logs,
            Err(f) => f.meta.logs,
        }
    }

    pub fn submit(&mut self, b: &Blk) -> Result<u64, TransactionError> {
        let ix = client::submit_header(self.payer.pubkey(), b.bytes);
        self.send_labeled("submit_header", &[ix], &[])
    }

    /// Upload the proof (begin + chunks); the first chunk rides with begin_verify.
    pub fn upload(&mut self, b: &Blk) -> Result<(), TransactionError> {
        let (pk, w) = (self.payer.pubkey(), self.work.pubkey());
        let job = b.job_id();
        let begin = client::begin_verify(pk, w, &b.hash, job.as_ref(), b.proof.len() as u32);
        let first = client::FIRST_CHUNK.min(b.proof.len());
        self.send_labeled("begin_verify+write", &[begin, client::write_proof(pk, w, 0, b.proof[..first].to_vec())], &[])?;
        let mut off = first;
        while off < b.proof.len() {
            let n = client::WRITE_CHUNK.min(b.proof.len() - off);
            self.send_labeled("write_proof", &[client::write_proof(pk, w, off as u32, b.proof[off..off + n].to_vec())], &[])?;
            off += n;
        }
        Ok(())
    }

    pub fn steps(&mut self, b: &Blk, plan: &[Vec<Step>]) -> Result<(), TransactionError> {
        let (pk, w) = (self.payer.pubkey(), self.work.pubkey());
        for tx in plan {
            let ixs: Vec<Instruction> = tx.iter().map(|s| s.instruction(pk, w, &b.hash, pk)).collect();
            let label = tx.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join("+");
            self.send_labeled(&label, &ixs, &[])?;
        }
        Ok(())
    }

    /// Job blocks in these tests use the localnet config, as the fallback config does.
    pub fn plan(&self, b: &Blk) -> Vec<Vec<Step>> {
        let shape = client::Shape::new(b.h.is_fallback(), self.p.fallback_m, self.p.fallback_n, &self.p.fallback_config, b.proof.len());
        client::plan(&shape, client::TX_CU_BUDGET)
    }

    /// Submit + upload + verify one header the way the relay does.
    pub fn relay(&mut self, b: &Blk) -> Result<(), TransactionError> {
        self.submit(b)?;
        self.upload(b)?;
        let plan = self.plan(b);
        self.steps(b, &plan)
    }

    pub fn header(&self, hash: &[u8; 32]) -> Option<Vec<u8>> {
        self.svm.get_account(&client::header_addr(hash)).map(|a| a.data)
    }

    pub fn status(&self, hash: &[u8; 32]) -> u8 {
        self.header(hash).map(|d| d[layout::STATUS]).unwrap_or(0)
    }

    pub fn is_final(&self, hash: &[u8; 32]) -> bool {
        self.header(hash).map(|d| d[layout::IS_FINAL] == 1).unwrap_or(false)
    }

    pub fn config(&self) -> lightclient::Config {
        let a = self.svm.get_account(&client::config_pda()).unwrap();
        lightclient::Config::try_deserialize(&mut a.data.as_slice()).unwrap()
    }

    pub fn mark_final(&mut self, hash: &[u8; 32]) -> Result<u64, TransactionError> {
        self.send(&[client::mark_final(hash)], &[])
    }
}

fn bincode_len(tx: &Transaction) -> usize {
    // legacy wire size: shortvec sigs + sigs + message
    let msg = tx.message.serialize();
    let n = tx.signatures.len();
    1 + 64 * n + msg.len() + if n > 127 { 1 } else { 0 }
}

/// One ed25519 precompile instruction carrying (pubkey, sig, msg) entries, all
/// offsets inside this instruction.
pub fn ed25519_ix(entries: &[([u8; 32], [u8; 64], Vec<u8>)]) -> Instruction {
    let n = entries.len();
    let mut data = vec![n as u8, 0u8];
    let header = 2 + 14 * n;
    let mut body: Vec<u8> = Vec::new();
    let mut offsets: Vec<u8> = Vec::new();
    for (pk, sig, msg) in entries {
        let pk_off = header + body.len();
        body.extend_from_slice(pk);
        let sig_off = header + body.len();
        body.extend_from_slice(sig);
        let msg_off = header + body.len();
        body.extend_from_slice(msg);
        for v in [sig_off as u16, u16::MAX, pk_off as u16, u16::MAX, msg_off as u16, msg.len() as u16, u16::MAX] {
            offsets.extend_from_slice(&v.to_le_bytes());
        }
    }
    data.extend_from_slice(&offsets);
    data.extend_from_slice(&body);
    Instruction { program_id: ED25519, accounts: vec![], data }
}

// ---------------------------------------------------------------- bridge setup

pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[owner.as_ref(), TOKEN_2022.as_ref(), mint.as_ref()], &ATA_PROGRAM).0
}
pub fn bridge_config() -> Pubkey {
    Pubkey::find_program_address(&[b"config"], &bridge::ID).0
}
pub fn processed_pda(nonce: u64) -> Pubkey {
    Pubkey::find_program_address(&[b"processed", &[bridge::CHAIN_AGENT], &nonce.to_le_bytes()], &bridge::ID).0
}

pub struct BridgeSetup {
    pub mint: Pubkey,
    pub mint_auth: Keypair,
    pub signer: Keypair,
}

impl Env {
    pub fn create_ata(&mut self, owner: &Pubkey, mint: &Pubkey) -> Pubkey {
        let ix = Instruction {
            program_id: ATA_PROGRAM,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(ata(owner, mint), false),
                AccountMeta::new_readonly(*owner, false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new_readonly(system_program::ID, false),
                AccountMeta::new_readonly(TOKEN_2022, false),
            ],
            data: vec![1],
        };
        self.send(&[ix], &[]).expect("create ata");
        ata(owner, mint)
    }

    pub fn token_balance(&self, account: &Pubkey) -> u64 {
        let a = self.svm.get_account(account).expect("token account");
        u64::from_le_bytes(a.data[64..72].try_into().unwrap())
    }

    /// Bridge with a Token-2022 mint, a funded vault (`vault_amount`), release modes set.
    pub fn setup_bridge(&mut self, vault_amount: u64, modes: u8) -> BridgeSetup {
        self.add_program(bridge::ID, "bridge");
        let mint_auth = Keypair::new();
        let mint_kp = Keypair::new();
        let mint = mint_kp.pubkey();
        let mint_len = 82;
        let rent = self.svm.minimum_balance_for_rent_exemption(mint_len);
        self.svm
            .set_account(mint, Account { lamports: rent, data: vec![0; mint_len], owner: TOKEN_2022, executable: false, rent_epoch: 0 })
            .unwrap();
        let mut im = vec![20u8, 6];
        im.extend_from_slice(mint_auth.pubkey().as_ref());
        im.push(0);
        self.send(&[Instruction { program_id: TOKEN_2022, accounts: vec![AccountMeta::new(mint, false)], data: im }], &[]).expect("mint");
        let signer = Keypair::new();
        let init = Instruction {
            program_id: bridge::ID,
            accounts: bridge::accounts::Initialize {
                admin: self.payer.pubkey(),
                config: bridge_config(),
                mint,
                vault: ata(&bridge_config(), &mint),
                token_program: TOKEN_2022,
                associated_token_program: ATA_PROGRAM,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: bridge::instruction::Initialize { signers: vec![signer.pubkey()], threshold: 1 }.data(),
        };
        self.send(&[init], &[]).expect("bridge init");
        // mint straight into the vault (stands in for earlier deposits)
        let vault = ata(&bridge_config(), &mint);
        let mut data = vec![7u8];
        data.extend_from_slice(&vault_amount.to_le_bytes());
        let mt = Instruction {
            program_id: TOKEN_2022,
            accounts: vec![AccountMeta::new(mint, false), AccountMeta::new(vault, false), AccountMeta::new_readonly(mint_auth.pubkey(), true)],
            data,
        };
        let ma = mint_auth.insecure_clone();
        self.send(&[mt], &[&ma]).expect("mint to vault");
        self.set_release_modes(modes).expect("modes");
        BridgeSetup { mint, mint_auth, signer }
    }

    pub fn set_release_modes(&mut self, modes: u8) -> Result<u64, TransactionError> {
        let ix = Instruction {
            program_id: bridge::ID,
            accounts: bridge::accounts::AdminOnly { admin: self.payer.pubkey(), config: bridge_config() }.to_account_metas(None),
            data: bridge::instruction::SetReleaseModes { modes, light_client: lightclient::ID }.data(),
        };
        self.send(&[ix], &[])
    }

    pub fn release_proof_ix(&self, bs: &BridgeSetup, nonce: u64, entry: &[u8], location: u64, proof: Vec<u8>, header_hash: &[u8; 32], recipient_token: Pubkey) -> Instruction {
        Instruction {
            program_id: bridge::ID,
            accounts: bridge::accounts::ReleaseProof {
                payer: self.payer.pubkey(),
                config: bridge_config(),
                mint: bs.mint,
                vault: ata(&bridge_config(), &bs.mint),
                recipient_token,
                processed: processed_pda(nonce),
                lc_header: client::header_addr(header_hash),
                token_program: TOKEN_2022,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: bridge::instruction::ReleaseProof { nonce, entry: entry.to_vec(), location, proof }.data(),
        }
    }

    pub fn release_signed_ix(&self, bs: &BridgeSetup, nonce: u64, recipient: Pubkey, amount: u64, recipient_token: Pubkey) -> Instruction {
        Instruction {
            program_id: bridge::ID,
            accounts: bridge::accounts::Release {
                payer: self.payer.pubkey(),
                config: bridge_config(),
                mint: bs.mint,
                vault: ata(&bridge_config(), &bs.mint),
                recipient_token,
                processed: processed_pda(nonce),
                instructions: IX_SYSVAR,
                token_program: TOKEN_2022,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: bridge::instruction::Release { nonce, recipient, amount }.data(),
        }
    }
}

// ---------------------------------------------------------------- storefront setup

pub fn sf_config() -> Pubkey {
    Pubkey::find_program_address(&[b"config"], &storefront::ID).0
}

impl Env {
    pub fn setup_storefront(&mut self, attester: &Pubkey, modes: u8) {
        self.add_program(storefront::ID, "storefront");
        let trader = storefront::trader_pda().0;
        let init = Instruction {
            program_id: storefront::ID,
            accounts: storefront::accounts::Initialize { admin: self.payer.pubkey(), config: sf_config(), trader, system_program: system_program::ID }
                .to_account_metas(None),
            data: storefront::instruction::Initialize { params: storefront::ConfigParams::default(), attesters: vec![*attester], threshold: 1 }.data(),
        };
        self.send(&[init], &[]).expect("storefront init");
        self.set_finalize_modes(modes).expect("modes");
    }

    pub fn set_finalize_modes(&mut self, modes: u8) -> Result<u64, TransactionError> {
        let ix = Instruction {
            program_id: storefront::ID,
            accounts: storefront::accounts::AdminOnly { admin: self.payer.pubkey(), config: sf_config() }.to_account_metas(None),
            data: storefront::instruction::SetFinalizeModes { modes, light_client: lightclient::ID }.data(),
        };
        self.send(&[ix], &[])
    }

    /// Writes a pending Order for bridge nonce `nonce` (as `buy_job` would have).
    pub fn inject_order(&mut self, nonce: u64, buyer: &Pubkey, evaluator: &Pubkey) -> Pubkey {
        let (addr, bump) = storefront::order_pda(nonce);
        let mut evaluators = [Pubkey::default(); storefront::MAX_EVALUATORS];
        evaluators[0] = *evaluator;
        let o = storefront::Order {
            version: 1,
            status: storefront::STATUS_PENDING,
            venue: 0,
            bump,
            deposit_nonce: nonce,
            job_id: storefront::order_job_id(nonce),
            buyer: *buyer,
            recipient: Pubkey::new_unique(),
            body_hash: [0; 32],
            sol_spent: 1,
            tokens: 1,
            worker_pay: 1,
            protocol_fee: 1,
            created_slot: 0,
            created_ts: 0,
            deliverer: Pubkey::default(),
            result_hash: [0; 32],
            closed_slot: 0,
            result_uri_len: 0,
            result_uri: [0; storefront::MAX_URI],
            spec: storefront::JobSpec {
                m: 64,
                n: 64,
                k: pearl_job::MiningConfig::localnet_default().common_dim,
                config: pearl_job::MiningConfig::localnet_default().to_bytes(),
                hash_a: [0; 32],
                hash_b: [0; 32],
                deadline_blocks: 100,
                max_blocks: 10,
                threshold: 1,
                evaluator_count: 1,
                evaluators,
                data_uri_len: 0,
                data_uri: [0; storefront::MAX_URI],
            },
        };
        let mut data = Vec::new();
        o.try_serialize(&mut data).unwrap();
        assert_eq!(data.len(), storefront::layout::LEN);
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm.set_account(addr, Account { lamports, data, owner: storefront::ID, executable: false, rent_epoch: 0 }).unwrap();
        addr
    }

    pub fn order(&self, nonce: u64) -> storefront::Order {
        let a = self.svm.get_account(&storefront::order_pda(nonce).0).unwrap();
        storefront::Order::try_deserialize(&mut a.data.as_slice()).unwrap()
    }
}
