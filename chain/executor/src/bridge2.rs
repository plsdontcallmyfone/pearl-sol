//! Two-side bridge (docs/BRIDGE-V2.md): BridgeUnlockV2 (40), BridgeLockV2 (41), SetRouteV2 (42),
//! AdminConfig keys 50-55 and the per-domain counters.
//!
//! Activation: the v2 kinds are valid only from `bridge_v2_height` (own record, state::bridge2;
//! default u64::MAX = disabled). Before it every block executes exactly as before this module:
//! the only additions on the old paths are reads of records that do not exist yet (bridge
//! params, a lien's domain), which return the defaults and write nothing.
//!
//! From activation, domain 1 (Solana, kinds 6/7/9) gets one extra rule so the Solana vault can
//! never be asked for more than it holds: a BridgeLock or a lien payout to Solana must not exceed
//! domain 1's outstanding, `TOTAL_SUPPLY - escrow - sum(outstanding of the other domains)`.
//! While no other side has unlocked anything that is all of the circulating supply, so the rule
//! never refuses a lock a sender could fund.

use agent_types::bridge2::{deposit_message_v2, is_ext_domain, order_job_id_v2, order_message_v2, WithdrawalV2, DOMAIN_SOLANA, EXT_SCALE};
use agent_types::{escrow_address, Pubkey, Tx, TxBody, TOTAL_SUPPLY};
use state::bridge2::{self as b2, BridgeParams, DepositV2, DomainCfg, DomainStats};
use state::config::Config;
use state::schema::Route;
use state::{StateError, StateRead};

use crate::{bad, check_sigs, BlockExecutor, Overlay, TxError, TxOutcome};

fn corrupt(what: &str, e: impl std::fmt::Display) -> StateError {
    StateError::Corrupt(format!("{what}: {e}"))
}

impl<S: StateRead> Overlay<'_, S> {
    pub async fn bridge_params(&self) -> Result<BridgeParams, StateError> {
        match self.get(&b2::params_key()).await? {
            None => Ok(BridgeParams::default()),
            Some(b) => BridgeParams::decode(&b).map_err(|e| corrupt("bridge params", e)),
        }
    }

    pub async fn domain_stats(&self, d: u8) -> Result<DomainStats, StateError> {
        match self.get(&b2::domain_stats_key(d)).await? {
            None => Ok(DomainStats::default()),
            Some(b) => DomainStats::decode(&b).map_err(|e| corrupt("domain stats", e)),
        }
    }

    /// The bridge domain a lien pays out to (1 = Solana when no record exists).
    pub async fn route_domain(&self, a: &Pubkey) -> Result<u8, StateError> {
        match self.get(&b2::route_domain_key(a)).await? {
            None => Ok(DOMAIN_SOLANA),
            Some(b) => b2::decode_route_domain(&b).map_err(|e| corrupt("route domain", e)),
        }
    }

    /// Set a lien's domain; domain 1 removes the record (and only touches state if one exists).
    pub(crate) async fn set_route_domain(&mut self, a: &Pubkey, d: u8) -> Result<(), StateError> {
        let k = b2::route_domain_key(a);
        if d == DOMAIN_SOLANA {
            if self.get(&k).await?.is_some() {
                self.put(k, None);
            }
        } else {
            self.put(k, Some(b2::encode_route_domain(d)));
        }
        Ok(())
    }

    /// Sum of the outstanding of every configured external domain.
    pub async fn ext_outstanding(&self, p: &BridgeParams) -> Result<u64, StateError> {
        let mut s: u64 = 0;
        for d in &p.domains {
            s = s.saturating_add(self.domain_stats(d.domain).await?.outstanding());
        }
        Ok(s)
    }

    /// Domain 1's outstanding: `TOTAL_SUPPLY - escrow - sum(outstanding of the other domains)`.
    pub async fn solana_outstanding(&self, p: &BridgeParams) -> Result<u64, StateError> {
        let esc = self.account(&escrow_address()).await?.balance;
        Ok(TOTAL_SUPPLY.saturating_sub(esc).saturating_sub(self.ext_outstanding(p).await?))
    }
}

/// Stateless checks of a BridgeUnlockV2 for sendTx and gossip (the executor repeats them): the v2
/// kinds are active at `next_height`, the domain is configured and running, the attester set
/// signed the v2 message.
pub fn precheck_bridge_unlock_v2(tx: &Tx, cfg: &Config, p: &BridgeParams, next_height: u64) -> Result<(), String> {
    if tx.body.kind().is_bridge_v2() && !p.is_active(next_height) {
        return Err(not_active(p));
    }
    let TxBody::BridgeUnlockV2 { domain, ext_nonce, recipient, amount_ext, job_order, sigs } = &tx.body else { return Ok(()) };
    if cfg.bridge_paused {
        return Err("bridge paused".into());
    }
    let dc = running_domain(p, *domain).map_err(|e| e.to_string())?;
    if *amount_ext == 0 || amount_ext.checked_mul(EXT_SCALE).is_none() {
        return Err("deposit amount out of range".into());
    }
    let msg = unlock_message(dc, *ext_nonce, recipient, *amount_ext, job_order.as_ref());
    check_sigs(sigs, &cfg.attesters, cfg.attester_threshold, &msg).map_err(|e| e.to_string())
}

fn not_active(p: &BridgeParams) -> String {
    if p.activation_height == u64::MAX {
        "bridge v2 kinds are not active (bridge_v2_height unset)".into()
    } else {
        format!("bridge v2 kinds activate at height {}", p.activation_height)
    }
}

fn running_domain(p: &BridgeParams, d: u8) -> Result<&DomainCfg, TxError> {
    if d == DOMAIN_SOLANA {
        return bad("domain 1 (Solana) uses BridgeUnlock, BridgeLock and SetRoute");
    }
    if !is_ext_domain(d) {
        return bad(format!("bad bridge domain {d}"));
    }
    let Some(dc) = p.domain(d) else { return bad(format!("bridge domain {d} is not configured")) };
    if dc.paused {
        return bad(format!("bridge domain {d} is paused"));
    }
    Ok(dc)
}

fn unlock_message(dc: &DomainCfg, nonce: u64, recipient: &Pubkey, amount_ext: u64, job: Option<&agent_types::PostJob>) -> Vec<u8> {
    match job {
        None => deposit_message_v2(dc.domain, dc.ext_chain_id, &dc.bridge_addr, nonce, recipient, amount_ext),
        Some(p) => order_message_v2(dc.domain, dc.ext_chain_id, &dc.bridge_addr, nonce, recipient, amount_ext, p),
    }
}

fn check_recipient(dc: &DomainCfg, r: &Pubkey) -> Result<(), TxError> {
    if *r == [0; 32] {
        return bad("recipient must be set");
    }
    if dc.is_evm() && r[..12] != [0; 12] {
        return bad("an EVM recipient is a 20-byte address left-padded with 12 zero bytes");
    }
    Ok(())
}

impl<'a, S: StateRead> BlockExecutor<'a, S> {
    /// exec_tx gate for kinds 40-42.
    pub(crate) async fn check_bridge_v2_active(&self) -> Result<(), TxError> {
        let p = self.ov.bridge_params().await?;
        if !p.is_active(self.env.height) {
            return bad(not_active(&p));
        }
        Ok(())
    }

    /// From activation, a BridgeLock (kind 7) may not take more than domain 1's outstanding.
    pub(crate) async fn check_solana_lock(&self, amount: u64) -> Result<(), TxError> {
        let p = self.ov.bridge_params().await?;
        if p.is_active(self.env.height) {
            let out = self.ov.solana_outstanding(&p).await?;
            if amount > out {
                return bad(format!("domain 1 outstanding {out} is below {amount}"));
            }
        }
        Ok(())
    }

    /// May a lien pay `amount` out to `domain` now? (Otherwise it keeps it pending, as while the
    /// bridge is paused.)
    pub(crate) async fn lien_payout_allowed(&self, domain: u8, amount: u64) -> Result<bool, TxError> {
        let p = self.ov.bridge_params().await?;
        if domain == DOMAIN_SOLANA {
            if !p.is_active(self.env.height) {
                return Ok(true);
            }
            return Ok(amount <= self.ov.solana_outstanding(&p).await?);
        }
        if !p.is_active(self.env.height) || p.domain(domain).is_none_or(|d| d.paused) {
            return Ok(false);
        }
        let s = self.ov.domain_stats(domain).await?;
        Ok(s.locked.checked_add(amount).is_some_and(|l| l <= s.unlocked))
    }

    /// Move `amount` (a positive multiple of 1000, already taken from its owner) into the escrow
    /// for `domain` and append the v2 log entry. The caller checked the outstanding.
    pub(crate) async fn lock_to_domain(&mut self, domain: u8, from: &Pubkey, recipient: &Pubkey, amount: u64) -> Result<WithdrawalV2, TxError> {
        let mut s = self.ov.domain_stats(domain).await?;
        let Some(locked) = s.locked.checked_add(amount).filter(|l| *l <= s.unlocked) else {
            return bad(format!("domain {domain} outstanding {} is below {amount}", s.outstanding()));
        };
        self.ov.credit(&escrow_address(), amount).await?;
        let w = WithdrawalV2 { domain, nonce: s.withdrawal_count, from: *from, recipient: *recipient, amount_ext: amount / EXT_SCALE, height: self.env.height };
        s.locked = locked;
        s.withdrawal_count += 1;
        self.ov.put(b2::domain_stats_key(domain), Some(s.encode()));
        self.ov.put(b2::withdrawal_v2_key(domain, w.nonce), Some(b2::encode_withdrawal_v2(&w)));
        Ok(w)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn bridge_unlock_v2(
        &mut self,
        domain: u8,
        ext_nonce: u64,
        recipient: &Pubkey,
        amount_ext: u64,
        job_order: Option<&agent_types::PostJob>,
        sigs: &[agent_types::AttesterSig],
        out: &mut TxOutcome,
    ) -> Result<(), TxError> {
        let h = self.env.height;
        if self.cfg.bridge_paused {
            return bad("bridge paused");
        }
        let p = self.ov.bridge_params().await?;
        let dc = running_domain(&p, domain)?.clone();
        let dk = b2::deposit_v2_key(domain, ext_nonce);
        if self.ov.get(&dk).await?.is_some() {
            return bad("deposit already processed");
        }
        let msg = unlock_message(&dc, ext_nonce, recipient, amount_ext, job_order);
        check_sigs(sigs, &self.cfg.attesters, self.cfg.attester_threshold, &msg)?;
        let Some(amount) = amount_ext.checked_mul(EXT_SCALE) else { return bad("amount overflow") };
        if amount == 0 {
            return bad("zero deposit");
        }
        if *recipient == escrow_address() {
            return bad("cannot unlock to the escrow");
        }
        self.ov.debit(&escrow_address(), amount).await.map_err(|_| TxError::Invalid("escrow too small".into()))?;
        self.ov.credit(recipient, amount).await?;
        self.ov.put(dk, Some(DepositV2 { height: h, recipient: *recipient, amount_ext }.encode()));
        let mut s = self.ov.domain_stats(domain).await?;
        let Some(u) = s.unlocked.checked_add(amount) else { return bad("domain counter overflow") };
        s.unlocked = u;
        s.deposit_count += 1;
        self.ov.put(b2::domain_stats_key(domain), Some(s.encode()));
        if let Some(job) = job_order {
            let (deadline, ticket) = self.check_job_or_ticket(job, h, true).await?;
            let id = order_job_id_v2(domain, ext_nonce);
            self.create_job(recipient, id, job, deadline, true, ticket).await?;
            out.job_id = Some(id);
        }
        Ok(())
    }

    pub(crate) async fn bridge_lock_v2(&mut self, from: &Pubkey, domain: u8, recipient: &Pubkey, amount: u64, out: &mut TxOutcome) -> Result<(), TxError> {
        if self.cfg.bridge_paused {
            return bad("bridge paused");
        }
        let p = self.ov.bridge_params().await?;
        let dc = running_domain(&p, domain)?.clone();
        check_recipient(&dc, recipient)?;
        if amount == 0 || amount % EXT_SCALE != 0 {
            return bad("amount must be a positive multiple of 1000");
        }
        let s = self.ov.domain_stats(domain).await?;
        if s.locked.checked_add(amount).is_none_or(|l| l > s.unlocked) {
            return bad(format!("domain {domain} outstanding {} is below {amount}", s.outstanding()));
        }
        self.ov.debit(from, amount).await?;
        let w = self.lock_to_domain(domain, from, recipient, amount).await?;
        out.withdrawals_v2.push(w);
        Ok(())
    }

    /// SetRouteV2: the same rules as SetRoute (one lien at a time, extend only), paying out to
    /// `domain` (2..=15). The lien record is the same Route record; the domain sits in its own
    /// record (tag 57).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn set_route_v2(
        &mut self,
        from: &Pubkey,
        domain: u8,
        recipient: &Pubkey,
        route_bps: u16,
        until_height: u64,
        cap: u64,
        release_key: &Pubkey,
        out: &mut TxOutcome,
    ) -> Result<(), TxError> {
        let h = self.env.height;
        let p = self.ov.bridge_params().await?;
        let dc = running_domain(&p, domain)?.clone();
        check_recipient(&dc, recipient)?;
        if route_bps == 0 || route_bps > 10_000 {
            return bad("route_bps must be 1..=10000");
        }
        if until_height <= h {
            return bad("until_height must be after the current height");
        }
        if cap == 0 {
            return bad("cap must be positive");
        }
        let old_domain = self.ov.route_domain(from).await?;
        let r = match self.ov.route(from).await? {
            Some(old) if old.is_active(h) => {
                if old_domain != domain || old.solana_recipient != *recipient || old.release_key != *release_key {
                    return bad("an active route exists: only an extension with the same domain, recipient and release key is allowed");
                }
                if route_bps < old.route_bps || until_height < old.until_height || cap < old.cap {
                    return bad("an active route can only be extended (route_bps, until_height and cap may not decrease)");
                }
                Route { route_bps, until_height, cap, route_nonce: old.route_nonce + 1, set_height: h, ..old }
            }
            old => {
                let next = old.map(|o| o.route_nonce + 1).unwrap_or(0);
                if let Some(mut o) = old {
                    self.flush_route(from, &mut o, out).await?;
                    if o.pending >= agent_types::SPL_SCALE {
                        return bad("the previous route still holds funds it cannot pay out yet");
                    }
                    if o.pending > 0 {
                        self.ov.credit(from, o.pending).await?;
                    }
                }
                Route {
                    solana_recipient: *recipient,
                    route_bps,
                    until_height,
                    cap,
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
        self.ov.put_route(from, &r);
        self.ov.set_route_domain(from, domain).await?;
        Ok(())
    }

    /// AdminConfig keys 50-55 (own record, state::bridge2).
    pub(crate) async fn admin_bridge_config(&mut self, from: &Pubkey, key: u8, value: &[u8]) -> Result<(), TxError> {
        if *from != self.cfg.admin {
            return bad("not the admin");
        }
        let h = self.env.height;
        let mut p = self.ov.bridge_params().await?;
        match key {
            b2::keys::BRIDGE_V2_HEIGHT => {
                let v = u64::from_le_bytes(value.try_into().map_err(|_| TxError::Invalid("expected u64".into()))?);
                if p.is_active(h) {
                    return bad(format!("bridge v2 is active since height {}; its height can no longer change", p.activation_height));
                }
                if v != u64::MAX && v <= h {
                    return bad(format!("bridge_v2_height must be above the current height {h} (or u64::MAX to disable)"));
                }
                p.activation_height = v;
            }
            b2::keys::BRIDGE_DOMAIN => {
                let d = DomainCfg::decode_value(value).map_err(TxError::Invalid)?;
                p.upsert(d).map_err(TxError::Invalid)?;
            }
            _ => return bad("unknown config key"),
        }
        let k = b2::params_key();
        if p == BridgeParams::default() {
            if self.ov.get(&k).await?.is_some() {
                self.ov.put(k, None);
            }
        } else {
            self.ov.put(k, Some(p.encode()));
        }
        Ok(())
    }
}
