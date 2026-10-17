//! Deterministic block execution (SPEC 2, 3.4, 4).
//!
//! A block executes against its parent's state through an [`Overlay`]. Each tx runs in
//! its own scratch layer: on success the layer merges into the block layer, on failure
//! it is dropped. [`execute_block`] is all-or-nothing (any failing tx rejects the block);
//! [`BlockExecutor`] is also used tx by tx to build templates, skipping failures.
//!
//! Block order: (1) credit coinbase rewards maturing at this height, (2) check the job a
//! job block mines (parent state), (3) txs, (4) job subsidy from the pool and this
//! block's coinbase (fees + subsidy) recorded to mature at `height + K`, and the coinbase
//! receipt (agent_types::coinbase, the last tx_root leaf) built from them.
//!
//! Payout liens (SetRoute, SPEC 4.4): matured coinbase, CompleteJob W and ExpireJob
//! refunds credited to an account with an active lien are split: `route_bps` of the
//! credit (floor, capped by the lien's remaining `cap`) goes to the lien, the rest to
//! the account. The lien's taken amount moves into the bridge escrow in whole SPL units
//! as withdrawal-log entries `{nonce, from = account, solana_recipient, amount_spl,
//! height}` (exactly as a BridgeLock would), and the remainder below 1000 base units
//! waits in the lien's `pending` for the next credit.

use std::collections::BTreeMap;

use agent_types::tx::{order_message, deposit_message, result_message, route_clear_message, verify_plain};
use agent_types::{
    escrow_address, is_reserved, pool_address, Coinbase, Hash, PostJob, Pubkey, Tx, TxBody, TxKind, SPL_SCALE, TOTAL_SUPPLY,
};
use state::config::{keys, Config};
use state::schema::{self, Account, Agent, Counters, Deposit, Immature, JobRecord, JobStatus, Key, Route, Withdrawal};
use state::{StateError, StateRead, WriteSet};

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ExecError {
    #[error("tx {index}: {err}")]
    Tx { index: usize, err: String },
    #[error("block: {0}")]
    Block(String),
    #[error(transparent)]
    State(#[from] StateError),
}

/// Why a single tx failed (the block layer is untouched).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum TxError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    State(#[from] StateError),
}

fn bad<T>(msg: impl Into<String>) -> Result<T, TxError> {
    Err(TxError::Invalid(msg.into()))
}

/// What the block header commits to besides the txs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEnv {
    pub chain_id: u32,
    pub height: u64,
    pub miner: Pubkey,
    /// `None` for a fallback block.
    pub job_id: Option<Hash>,
}

/// Per-block results besides the write set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockSummary {
    pub fees: u64,
    pub subsidy: u64,
    /// Matured coinbase credited by this block (before any lien split).
    pub matured: u64,
    /// The block's coinbase receipt (set by `finish`).
    pub coinbase: Coinbase,
    /// Income taken by payout liens in this block: `(account, amount)`.
    pub routed: Vec<(Pubkey, u64)>,
    pub tx_count: usize,
    pub withdrawals: Vec<Withdrawal>,
    pub jobs_created: Vec<Hash>,
    pub deposits: Vec<u64>,
}

/// Successful tx effects worth reporting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TxOutcome {
    pub job_id: Option<Hash>,
    /// Withdrawal-log entries this tx appended (a BridgeLock, or lien splits).
    pub withdrawals: Vec<Withdrawal>,
    pub routed: Vec<(Pubkey, u64)>,
}

/// Read-through layers over a base state.
pub struct Overlay<'a, S: StateRead> {
    base: &'a S,
    block: BTreeMap<Key, Option<Vec<u8>>>,
    tx: BTreeMap<Key, Option<Vec<u8>>>,
}

impl<'a, S: StateRead> Overlay<'a, S> {
    pub fn new(base: &'a S) -> Self {
        Self { base, block: BTreeMap::new(), tx: BTreeMap::new() }
    }

    pub async fn get(&self, k: &Key) -> Result<Option<Vec<u8>>, StateError> {
        if let Some(v) = self.tx.get(k) {
            return Ok(v.clone());
        }
        if let Some(v) = self.block.get(k) {
            return Ok(v.clone());
        }
        self.base.get(k).await
    }

    pub fn put(&mut self, k: Key, v: Option<Vec<u8>>) {
        self.tx.insert(k, v);
    }

    fn commit_tx(&mut self) {
        let tx = std::mem::take(&mut self.tx);
        self.block.extend(tx);
    }

    fn rollback_tx(&mut self) {
        self.tx.clear();
    }

    pub async fn account(&self, a: &Pubkey) -> Result<Account, StateError> {
        match self.get(&schema::account_key(a)).await? {
            None => Ok(Account::default()),
            Some(b) => Account::decode(&b).map_err(|e| StateError::Corrupt(format!("account: {e}"))),
        }
    }

    pub fn put_account(&mut self, a: &Pubkey, acc: &Account) {
        let v = if *acc == Account::default() { None } else { Some(acc.encode()) };
        self.put(schema::account_key(a), v);
    }

    pub async fn job(&self, id: &Hash) -> Result<Option<JobRecord>, StateError> {
        match self.get(&schema::job_key(id)).await? {
            None => Ok(None),
            Some(b) => JobRecord::decode(&b).map(Some).map_err(|e| StateError::Corrupt(format!("job: {e}"))),
        }
    }

    pub fn put_job(&mut self, j: &JobRecord) {
        self.put(schema::job_key(&j.job_id), Some(j.encode()));
    }

    pub async fn config(&self) -> Result<Config, StateError> {
        let b = self.get(&schema::config_key()).await?.ok_or_else(|| StateError::Corrupt("no config record".into()))?;
        Config::decode(&b).map_err(|e| StateError::Corrupt(format!("config: {e}")))
    }

    pub async fn counters(&self) -> Result<Counters, StateError> {
        match self.get(&schema::counters_key()).await? {
            None => Ok(Counters::default()),
            Some(b) => Counters::decode(&b).map_err(|e| StateError::Corrupt(format!("counters: {e}"))),
        }
    }

    pub fn put_counters(&mut self, c: &Counters) {
        self.put(schema::counters_key(), Some(c.encode()));
    }

    pub async fn route(&self, a: &Pubkey) -> Result<Option<Route>, StateError> {
        match self.get(&schema::route_key(a)).await? {
            None => Ok(None),
            Some(b) => Route::decode(&b).map(Some).map_err(|e| StateError::Corrupt(format!("route: {e}"))),
        }
    }

    pub fn put_route(&mut self, a: &Pubkey, r: &Route) {
        self.put(schema::route_key(a), Some(r.encode()));
    }

    /// Append a withdrawal-log entry for `amount` (a positive multiple of 1000) already
    /// moved into the escrow: next nonce, state record, counters.
    async fn append_withdrawal(&mut self, from: &Pubkey, solana_recipient: &Pubkey, amount: u64, height: u64) -> Result<Withdrawal, TxError> {
        let mut c = self.counters().await?;
        let w = Withdrawal { nonce: c.withdrawal_count, from: *from, solana_recipient: *solana_recipient, amount_spl: amount / SPL_SCALE, height };
        c.withdrawal_count += 1;
        self.put_counters(&c);
        self.put(schema::withdrawal_key(w.nonce), Some(w.encode()));
        Ok(w)
    }

    async fn credit(&mut self, a: &Pubkey, amt: u64) -> Result<(), TxError> {
        if amt == 0 {
            return Ok(());
        }
        let mut acc = self.account(a).await?;
        acc.balance = match acc.balance.checked_add(amt) {
            Some(b) => b,
            None => return bad("balance overflow"),
        };
        self.put_account(a, &acc);
        Ok(())
    }

    async fn debit(&mut self, a: &Pubkey, amt: u64) -> Result<(), TxError> {
        if amt == 0 {
            return Ok(());
        }
        let mut acc = self.account(a).await?;
        if acc.balance < amt {
            return bad(format!("insufficient balance: have {}, need {amt}", acc.balance));
        }
        acc.balance -= amt;
        self.put_account(a, &acc);
        Ok(())
    }

    fn into_writes(mut self) -> BTreeMap<Key, Option<Vec<u8>>> {
        self.commit_tx();
        self.block
    }
}

fn mul_ppm(x: u64, ppm: u64) -> u64 {
    ((x as u128 * ppm as u128) / 1_000_000) as u64
}

/// Check a PostJob body; returns the absolute deadline height.
fn check_post_job(p: &PostJob, cfg: &Config, height: u64, relative_deadline: bool) -> Result<u64, TxError> {
    let mc = match pearl_job::MiningConfig::from_bytes(&p.config) {
        Ok(c) => c,
        Err(e) => return bad(format!("job config: {e}")),
    };
    if p.k != mc.common_dim {
        return bad("k does not match the config's common_dim");
    }
    if let Err(e) = pearl_job::job::check_shape(&mc, p.m as usize, p.n as usize) {
        return bad(format!("job shape: {e}"));
    }
    if p.protocol_fee < cfg.min_protocol_fee {
        return bad(format!("protocol fee {} below min_protocol_fee {}", p.protocol_fee, cfg.min_protocol_fee));
    }
    if mul_ppm(p.protocol_fee, cfg.alpha_ppm) == 0 {
        return bad("protocol fee too small: alpha x P rounds to 0, so the job could never be mined");
    }
    check_uri(&p.data_uri, "data_uri")?;
    if p.max_blocks == 0 || p.max_blocks > cfg.max_job_blocks {
        return bad("max_blocks out of range");
    }
    let mut ev = p.evaluators.clone();
    ev.sort();
    ev.dedup();
    if ev.is_empty() || ev.len() != p.evaluators.len() {
        return bad("evaluators must be non-empty and distinct");
    }
    if p.threshold == 0 || p.threshold as usize > p.evaluators.len() {
        return bad("threshold must be 1..=evaluators");
    }
    let deadline = if relative_deadline {
        if p.deadline_height == 0 {
            return bad("relative deadline must be positive");
        }
        match height.checked_add(p.deadline_height) {
            Some(d) => d,
            None => return bad("deadline overflow"),
        }
    } else {
        p.deadline_height
    };
    if deadline <= height {
        return bad("deadline must be after the current height");
    }
    Ok(deadline)
}

/// A `blob:` URI must be well formed (`blob:` + 64 hex). Other schemes are opaque.
/// Blob availability is never checked here: consensus must not depend on local data.
fn check_uri(uri: &[u8], what: &str) -> Result<(), TxError> {
    if blobs::is_blob_uri(uri) && blobs::parse_uri(uri).is_none() {
        return bad(format!("{what}: malformed blob URI (expected blob:<64 hex>)"));
    }
    Ok(())
}

/// Smallest protocol fee a job may carry under `cfg`: `min_protocol_fee`, and at least
/// enough that `alpha x P` (the job's subsidy cap) is not 0.
pub fn effective_min_protocol_fee(cfg: &Config) -> u64 {
    let by_alpha = if cfg.alpha_ppm == 0 { u64::MAX } else { 1_000_000u64.div_ceil(cfg.alpha_ppm) };
    cfg.min_protocol_fee.max(by_alpha)
}

/// Stateless checks of a PostJob body that sendTx can run before the mempool (the
/// executor repeats them): protocol fee floor and URI shape.
pub fn precheck_post_job(p: &PostJob, cfg: &Config) -> Result<(), String> {
    let min = effective_min_protocol_fee(cfg);
    if p.protocol_fee < min {
        return Err(format!(
            "protocol fee {} below the chain minimum {} (min_protocol_fee {}, alpha_ppm {})",
            p.protocol_fee, min, cfg.min_protocol_fee, cfg.alpha_ppm
        ));
    }
    check_uri(&p.data_uri, "data_uri").map_err(|e| e.to_string())
}

/// Count valid, distinct signatures from `set` over `msg` (raw ed25519). Any signature
/// that is invalid or from outside the set fails the whole check.
fn check_sigs(sigs: &[agent_types::AttesterSig], set: &[Pubkey], threshold: u8, msg: &[u8]) -> Result<(), TxError> {
    let mut seen: Vec<Pubkey> = Vec::new();
    for s in sigs {
        if !set.contains(&s.pubkey) {
            return bad("signer not in the set");
        }
        if seen.contains(&s.pubkey) {
            return bad("duplicate signer");
        }
        if !verify_plain(&s.pubkey, msg, &s.sig) {
            return bad("bad signature from set member");
        }
        seen.push(s.pubkey);
    }
    if threshold == 0 || seen.len() < threshold as usize {
        return bad(format!("{} of {} required signatures", seen.len(), threshold));
    }
    Ok(())
}

pub struct BlockExecutor<'a, S: StateRead> {
    ov: Overlay<'a, S>,
    env: BlockEnv,
    cfg: Config,
    summary: BlockSummary,
}

impl<'a, S: StateRead> BlockExecutor<'a, S> {
    /// Start a block on `base` (the parent's post-state).
    pub async fn begin(base: &'a S, env: BlockEnv) -> Result<Self, ExecError> {
        let ov = Overlay::new(base);
        let cfg = ov.config().await?;
        let mut ex = Self { ov, env, cfg, summary: BlockSummary::default() };
        let ik = schema::immature_key(env.height);
        if let Some(b) = ex.ov.get(&ik).await? {
            let imm = Immature::decode(&b).map_err(|e| StateError::Corrupt(format!("immature: {e}")))?;
            let mut ws = Vec::new();
            let mut routed = Vec::new();
            for (miner, amt, _) in &imm.entries {
                ex.credit_income(miner, *amt, &mut ws, &mut routed).await.map_err(|e| ExecError::Block(e.to_string()))?;
                ex.summary.matured += amt;
            }
            ex.summary.withdrawals.extend(ws);
            ex.summary.routed.extend(routed);
            ex.ov.put(ik, None);
        }
        if let Some(id) = env.job_id {
            let job = ex.ov.job(&id).await?.ok_or_else(|| ExecError::Block("job block names an unknown job".into()))?;
            check_mineable(&job, env.height).map_err(ExecError::Block)?;
        }
        ex.ov.commit_tx();
        Ok(ex)
    }

    /// Credit income `amt` to `who`, splitting the share of an active payout lien off
    /// into withdrawal entries (module doc). Plain credit when no lien is active.
    async fn credit_income(&mut self, who: &Pubkey, amt: u64, ws: &mut Vec<Withdrawal>, routed: &mut Vec<(Pubkey, u64)>) -> Result<(), TxError> {
        let h = self.env.height;
        let Some(mut r) = self.ov.route(who).await? else { return self.ov.credit(who, amt).await };
        if !r.is_active(h) {
            // An ended lien sends out what it still holds (if the bridge was paused) and
            // hands its dust back at the account's next income.
            if r.pending > 0 {
                self.flush_route(who, &mut r, ws).await?;
                self.ov.put_route(who, &r);
            }
            return self.ov.credit(who, amt).await;
        }
        let share = ((amt as u128 * r.route_bps as u128) / 10_000) as u64;
        let share = share.min(r.cap - r.routed);
        r.routed += share;
        r.pending += share;
        if share > 0 {
            routed.push((*who, share));
        }
        self.flush_route(who, &mut r, ws).await?;
        self.ov.put_route(who, &r);
        self.ov.credit(who, amt - share).await
    }

    /// Move the lien's pending amount out in whole SPL units (unless the bridge is
    /// paused); once the lien has ended, the dust left goes back to the account.
    async fn flush_route(&mut self, who: &Pubkey, r: &mut Route, ws: &mut Vec<Withdrawal>) -> Result<(), TxError> {
        let h = self.env.height;
        if !self.cfg.bridge_paused {
            let out = r.pending / SPL_SCALE * SPL_SCALE;
            if out > 0 {
                self.ov.credit(&escrow_address(), out).await?;
                r.pending -= out;
                r.withdrawn += out;
                ws.push(self.ov.append_withdrawal(who, &r.solana_recipient, out, h).await?);
            }
        }
        if !r.is_active(h) && r.pending > 0 && r.pending < SPL_SCALE {
            let dust = std::mem::take(&mut r.pending);
            self.ov.credit(who, dust).await?;
        }
        Ok(())
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn tx_count(&self) -> usize {
        self.summary.tx_count
    }

    /// Apply one tx. On error nothing it did is kept.
    pub async fn apply_tx(&mut self, tx: &Tx) -> Result<TxOutcome, TxError> {
        match self.exec_tx(tx).await {
            Ok(o) => {
                self.ov.commit_tx();
                self.summary.fees += tx.fee;
                self.summary.tx_count += 1;
                if let Some(id) = o.job_id {
                    self.summary.jobs_created.push(id);
                }
                self.summary.withdrawals.extend(o.withdrawals.iter().copied());
                self.summary.routed.extend(o.routed.iter().copied());
                if let TxBody::BridgeUnlock { solana_nonce, .. } = &tx.body {
                    self.summary.deposits.push(*solana_nonce);
                }
                Ok(o)
            }
            Err(e) => {
                self.ov.rollback_tx();
                Err(e)
            }
        }
    }

    async fn exec_tx(&mut self, tx: &Tx) -> Result<TxOutcome, TxError> {
        let h = self.env.height;
        if tx.chain_id != self.env.chain_id {
            return bad("wrong chain_id");
        }
        if is_reserved(&tx.from) {
            return bad("reserved address cannot send");
        }
        if !tx.verify_sig() {
            return bad("bad tx signature");
        }
        let kind = tx.body.kind();
        if kind != TxKind::BridgeUnlock && tx.fee < self.cfg.min_fee {
            return bad(format!("fee {} below min_fee {}", tx.fee, self.cfg.min_fee));
        }
        let mut acc = self.ov.account(&tx.from).await?;
        if acc.nonce != tx.nonce {
            return bad(format!("bad nonce: expected {}, got {}", acc.nonce, tx.nonce));
        }
        if acc.balance < tx.fee {
            return bad("cannot pay fee");
        }
        acc.balance -= tx.fee;
        acc.nonce += 1;
        self.ov.put_account(&tx.from, &acc);

        let mut out = TxOutcome::default();
        match &tx.body {
            TxBody::Transfer { to, amount } => {
                if *to == escrow_address() {
                    return bad("use BridgeLock to send to the escrow");
                }
                self.ov.debit(&tx.from, *amount).await?;
                self.ov.credit(to, *amount).await?;
            }
            TxBody::RegisterAgent { name, uri } => {
                if name.is_empty() {
                    return bad("empty agent name");
                }
                let key = schema::agent_key(&tx.from);
                let registered_height = match self.ov.get(&key).await? {
                    Some(b) => Agent::decode(&b).map_err(|e| StateError::Corrupt(e.to_string()))?.registered_height,
                    None => h,
                };
                let a = Agent { name: name.clone(), uri: uri.clone(), registered_height, updated_height: h };
                self.ov.put(key, Some(a.encode()));
            }
            TxBody::PostJob(p) => {
                let deadline = check_post_job(p, &self.cfg, h, false)?;
                let id = schema::job_id_for_post(&tx.from, tx.nonce);
                self.create_job(&tx.from, id, p, deadline, false).await?;
                out.job_id = Some(id);
            }
            TxBody::SubmitResult { job_id, result_hash, uri } => {
                let Some(mut j) = self.ov.job(job_id).await? else { return bad("unknown job") };
                if j.status != JobStatus::Open {
                    return bad("job is not open");
                }
                if h > j.deadline_height {
                    return bad("job deadline passed");
                }
                check_uri(uri, "result uri")?;
                j.status = JobStatus::Delivered;
                j.deliverer = tx.from;
                j.result_hash = *result_hash;
                j.result_uri = uri.clone();
                self.ov.put_job(&j);
            }
            TxBody::CompleteJob { job_id, sigs } => {
                let Some(mut j) = self.ov.job(job_id).await? else { return bad("unknown job") };
                if j.status != JobStatus::Delivered {
                    return bad("job is not delivered");
                }
                let msg = result_message(job_id, &j.deliverer, &j.result_hash);
                check_sigs(sigs, &j.evaluators, j.threshold, &msg)?;
                j.status = JobStatus::Completed;
                j.closed_height = h;
                let pay = j.worker_pay;
                let to = j.deliverer;
                self.ov.put_job(&j);
                self.credit_income(&to, pay, &mut out.withdrawals, &mut out.routed).await?;
            }
            TxBody::ExpireJob { job_id } => {
                let Some(mut j) = self.ov.job(job_id).await? else { return bad("unknown job") };
                if !j.status.is_live() {
                    return bad("job already closed");
                }
                if h <= j.deadline_height {
                    return bad("job deadline not reached");
                }
                j.status = JobStatus::Expired;
                j.closed_height = h;
                let refund = j.worker_pay;
                let to = j.poster;
                self.ov.put_job(&j);
                self.credit_income(&to, refund, &mut out.withdrawals, &mut out.routed).await?;
            }
            TxBody::BridgeUnlock { solana_nonce, recipient, amount_spl, job_order, sigs } => {
                if self.cfg.bridge_paused {
                    return bad("bridge paused");
                }
                let dk = schema::deposit_key(*solana_nonce);
                if self.ov.get(&dk).await?.is_some() {
                    return bad("deposit already processed");
                }
                let msg = match job_order {
                    None => deposit_message(*solana_nonce, recipient, *amount_spl),
                    Some(p) => order_message(*solana_nonce, recipient, *amount_spl, p),
                };
                check_sigs(sigs, &self.cfg.attesters, self.cfg.attester_threshold, &msg)?;
                let Some(amount) = amount_spl.checked_mul(SPL_SCALE) else { return bad("amount overflow") };
                if amount == 0 {
                    return bad("zero deposit");
                }
                if *recipient == escrow_address() {
                    return bad("cannot unlock to the escrow");
                }
                self.ov.debit(&escrow_address(), amount).await.map_err(|_| TxError::Invalid("escrow too small".into()))?;
                self.ov.credit(recipient, amount).await?;
                let d = Deposit { height: h, recipient: *recipient, amount_spl: *amount_spl };
                self.ov.put(dk, Some(d.encode()));
                let mut c = self.ov.counters().await?;
                c.deposit_count += 1;
                self.ov.put_counters(&c);
                if let Some(p) = job_order {
                    let deadline = check_post_job(p, &self.cfg, h, true)?;
                    let id = schema::job_id_for_order(*solana_nonce);
                    self.create_job(recipient, id, p, deadline, true).await?;
                    out.job_id = Some(id);
                }
            }
            TxBody::BridgeLock { solana_recipient, amount } => {
                if self.cfg.bridge_paused {
                    return bad("bridge paused");
                }
                if *amount == 0 || amount % SPL_SCALE != 0 {
                    return bad("amount must be a positive multiple of 1000");
                }
                self.ov.debit(&tx.from, *amount).await?;
                self.ov.credit(&escrow_address(), *amount).await?;
                let w = self.ov.append_withdrawal(&tx.from, solana_recipient, *amount, h).await?;
                out.withdrawals.push(w);
            }
            TxBody::AdminConfig { key, value } => {
                if tx.from != self.cfg.admin {
                    return bad("not the admin");
                }
                if *key > keys::LAST {
                    return bad("unknown config key");
                }
                let mut c = self.cfg.clone();
                c.set(*key, value).map_err(TxError::Invalid)?;
                c.validate().map_err(TxError::Invalid)?;
                self.ov.put(schema::config_key(), Some(c.encode()));
                self.cfg = c;
            }
            TxBody::SetRoute { solana_recipient, route_bps, until_height, cap, release_key } => {
                if *route_bps == 0 || *route_bps > 10_000 {
                    return bad("route_bps must be 1..=10000");
                }
                if *until_height <= h {
                    return bad("until_height must be after the current height");
                }
                if *cap == 0 {
                    return bad("cap must be positive");
                }
                if *solana_recipient == [0; 32] {
                    return bad("solana_recipient must be set");
                }
                let r = match self.ov.route(&tx.from).await? {
                    Some(old) if old.is_active(h) => {
                        // One lien at a time; the account may only strengthen it.
                        if old.solana_recipient != *solana_recipient || old.release_key != *release_key {
                            return bad("an active route exists: only an extension with the same recipient and release key is allowed");
                        }
                        if *route_bps < old.route_bps || *until_height < old.until_height || *cap < old.cap {
                            return bad("an active route can only be extended (route_bps, until_height and cap may not decrease)");
                        }
                        Route {
                            route_bps: *route_bps,
                            until_height: *until_height,
                            cap: *cap,
                            route_nonce: old.route_nonce + 1,
                            set_height: h,
                            ..old
                        }
                    }
                    old => {
                        let next = old.map(|o| o.route_nonce + 1).unwrap_or(0);
                        if let Some(mut o) = old {
                            // The ended lien's holdings go out to its recipient first.
                            self.flush_route(&tx.from, &mut o, &mut out.withdrawals).await?;
                            if o.pending >= SPL_SCALE {
                                return bad("the previous route still holds funds while the bridge is paused");
                            }
                            if o.pending > 0 {
                                self.ov.credit(&tx.from, o.pending).await?;
                            }
                        }
                        Route {
                            solana_recipient: *solana_recipient,
                            route_bps: *route_bps,
                            until_height: *until_height,
                            cap: *cap,
                            release_key: *release_key,
                            route_nonce: next,
                            set_height: h,
                            routed: 0,
                            pending: 0,
                            withdrawn: 0,
                            cleared: false,
                        }
                    }
                };
                self.ov.put_route(&tx.from, &r);
            }
            TxBody::ClearRoute { account, route_nonce, sig } => {
                let Some(mut r) = self.ov.route(account).await? else { return bad("no route") };
                if !r.is_active(h) {
                    return bad("route is not active");
                }
                if r.route_nonce != *route_nonce {
                    return bad("route_nonce does not match the active route");
                }
                if r.release_key == [0; 32] {
                    return bad("this route has no release key");
                }
                let msg = route_clear_message(self.env.chain_id, account, *route_nonce);
                if !verify_plain(&r.release_key, &msg, sig) {
                    return bad("bad release signature");
                }
                r.cleared = true;
                self.flush_route(account, &mut r, &mut out.withdrawals).await?;
                self.ov.put_route(account, &r);
            }
        }
        Ok(out)
    }

    async fn create_job(&mut self, poster: &Pubkey, id: Hash, p: &PostJob, deadline: u64, order: bool) -> Result<(), TxError> {
        if self.ov.job(&id).await?.is_some() {
            return bad("job id already exists");
        }
        let Some(total) = p.worker_pay.checked_add(p.protocol_fee) else { return bad("W + P overflow") };
        self.ov.debit(poster, total).await?;
        self.ov.credit(&pool_address(), p.protocol_fee).await?;
        let mut c = self.ov.counters().await?;
        let j = JobRecord {
            job_id: id,
            poster: *poster,
            m: p.m,
            n: p.n,
            k: p.k,
            config: p.config,
            hash_a: p.hash_a,
            hash_b: p.hash_b,
            data_uri: p.data_uri.clone(),
            worker_pay: p.worker_pay,
            protocol_fee: p.protocol_fee,
            deadline_height: deadline,
            max_blocks: p.max_blocks,
            evaluators: p.evaluators.clone(),
            threshold: p.threshold,
            created_height: self.env.height,
            terms_hash: if order {
                schema::order_terms_hash(poster, p.m, p.n, p.k, p.max_blocks, p.deadline_height)
            } else {
                schema::terms_hash(poster, &id, p.m, p.n, p.k, p.worker_pay, p.protocol_fee, deadline, p.max_blocks)
            },
            header_id: if order { *poster } else { id },
            status: JobStatus::Open,
            blocks_mined: 0,
            cap_remaining: mul_ppm(p.protocol_fee, self.cfg.alpha_ppm),
            subsidy_paid: 0,
            deliverer: [0; 32],
            result_hash: [0; 32],
            result_uri: Vec::new(),
            closed_height: 0,
            index: c.job_count,
        };
        self.ov.put(schema::job_index_key(c.job_count), Some(schema::encode_job_index(&id)));
        c.job_count += 1;
        self.ov.put_counters(&c);
        self.ov.put_job(&j);
        Ok(())
    }

    /// Subsidy, coinbase, and the write set.
    pub async fn finish(mut self, withdrawal_log: bool) -> Result<(WriteSet, BlockSummary), ExecError> {
        let h = self.env.height;
        let blk = |e: TxError| ExecError::Block(e.to_string());
        if let Some(id) = self.env.job_id {
            let mut j = self.ov.job(&id).await?.ok_or_else(|| ExecError::Block("job vanished".into()))?;
            let pool = self.ov.account(&pool_address()).await?.balance;
            let s = mul_ppm(pool, self.cfg.emission_ppm).min(j.cap_remaining);
            j.blocks_mined += 1;
            j.cap_remaining -= s;
            j.subsidy_paid += s;
            self.ov.put_job(&j);
            self.ov.debit(&pool_address(), s).await.map_err(blk)?;
            self.summary.subsidy = s;
        }
        let reward = self.summary.fees + self.summary.subsidy;
        let mature_height = h + self.cfg.finality_k as u64;
        self.summary.coinbase = Coinbase {
            height: h,
            miner: self.env.miner,
            fees: self.summary.fees,
            subsidy: self.summary.subsidy,
            job_id: self.env.job_id.unwrap_or([0; 32]),
            mature_height,
        };
        if reward > 0 {
            let k = schema::immature_key(mature_height);
            let mut imm = match self.ov.get(&k).await? {
                Some(b) => Immature::decode(&b).map_err(|e| StateError::Corrupt(e.to_string()))?,
                None => Immature::default(),
            };
            imm.entries.push((self.env.miner, reward, h));
            self.ov.put(k, Some(imm.encode()));
        }
        let withdrawals = if withdrawal_log { self.summary.withdrawals.iter().map(Withdrawal::log_entry).collect() } else { Vec::new() };
        Ok((WriteSet { writes: self.ov.into_writes(), withdrawals }, self.summary))
    }
}

/// Can a block at `height` mine `job`? (parent-state check)
pub fn check_mineable(job: &JobRecord, height: u64) -> Result<(), String> {
    if !job.status.is_live() {
        return Err(format!("job is {}", job.status.name()));
    }
    if height > job.deadline_height {
        return Err("job expired".into());
    }
    if job.blocks_mined >= job.max_blocks {
        return Err("job max_blocks used up".into());
    }
    if job.cap_remaining == 0 {
        return Err("job subsidy cap used up".into());
    }
    Ok(())
}

/// Execute a whole block: every tx must succeed.
pub async fn execute_block<S: StateRead>(base: &S, env: BlockEnv, txs: &[Tx]) -> Result<(WriteSet, BlockSummary), ExecError> {
    let mut ex = BlockExecutor::begin(base, env).await?;
    if txs.len() > ex.cfg.max_block_txs as usize {
        return Err(ExecError::Block("too many txs".into()));
    }
    for (index, tx) in txs.iter().enumerate() {
        ex.apply_tx(tx).await.map_err(|e| ExecError::Tx { index, err: e.to_string() })?;
    }
    ex.finish(true).await
}

/// Genesis state: the config record and the whole supply in the escrow.
pub fn genesis_writes(cfg: &Config) -> Result<WriteSet, String> {
    cfg.validate()?;
    let mut ws = WriteSet::default();
    ws.writes.insert(schema::config_key(), Some(cfg.encode()));
    ws.writes.insert(
        schema::account_key(&escrow_address()),
        Some(Account { balance: TOTAL_SUPPLY, nonce: 0 }.encode()),
    );
    ws.writes.insert(schema::counters_key(), Some(Counters::default().encode()));
    Ok(ws)
}

/// Supply accounting over a full set of record values: balances (escrow and pool
/// included) + W held by live jobs + immature coinbase + liens' pending amounts. Must equal [`TOTAL_SUPPLY`].
pub fn supply_of(values: &[Vec<u8>]) -> Result<u128, String> {
    let mut total: u128 = 0;
    for v in values {
        match v.first() {
            Some(&schema::tag::ACCOUNT) => total += Account::decode(v).map_err(|e| e.to_string())?.balance as u128,
            Some(&schema::tag::JOB) => {
                let j = JobRecord::decode(v).map_err(|e| e.to_string())?;
                if j.status.is_live() {
                    total += j.worker_pay as u128;
                }
            }
            Some(&schema::tag::IMMATURE) => total += Immature::decode(v).map_err(|e| e.to_string())?.total() as u128,
            Some(&schema::tag::ROUTE) => total += Route::decode(v).map_err(|e| e.to_string())?.pending as u128,
            _ => {}
        }
    }
    Ok(total)
}
