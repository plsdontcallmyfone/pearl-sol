//! DM and channel kinds (SPEC 4.10): SendMessage, ReclaimMessage, SetConsent,
//! ChannelConfig, ChannelMember, ChannelKey, and the DM parameters (AdminConfig keys 35-44).
//!
//! Rules shared by all six: valid from `dm_upgrade_height(chain_id)`, and the fee must cover
//! the social fee rule `max(min_fee, social_min_fee + social_byte_fee x encoded tx length)`.
//! Each function checks before it writes; a failing tx leaves nothing behind (the executor
//! drops its scratch layer). Escrowed DM pay is held in its MsgEscrow record (counted by
//! `supply_of`), never in an account.

use agent_types::dm::{self as dm, ChannelConfig, Consent, KeyEnvelope, SendMessage};
use agent_types::{is_reserved, pool_address, Hash, Pubkey, Tx};
use state::dm::{self as sd, Channel, DmParams, DmStats, MsgEscrow};
use state::schema::tag;
use state::social as ss;
use state::StateRead;

use crate::social::social_fee_floor;
use crate::{bad, BlockExecutor, TxError};

fn corrupt(e: impl std::fmt::Display) -> TxError {
    TxError::State(state::StateError::Corrupt(e.to_string()))
}

/// The stateless DM checks sendTx runs before the mempool (the executor repeats them):
/// activation height (for the next block) and the social fee floor.
pub fn precheck_dm(tx: &Tx, cfg: &state::Config, next_height: u64) -> Result<(), String> {
    let kind = tx.body.kind();
    let dm_key = matches!(&tx.body, agent_types::TxBody::AdminConfig { key, .. } if sd::is_dm_key(*key));
    if !kind.is_dm() && !dm_key {
        return Ok(());
    }
    let h = dm::dm_upgrade_height(tx.chain_id);
    if next_height < h {
        return Err(format!("DM txs and config keys 35-44 activate at height {h} (next block {next_height})"));
    }
    if dm_key {
        return Ok(());
    }
    let len = tx.encode().len();
    let floor = social_fee_floor(cfg, len);
    if tx.fee < floor {
        return Err(format!("DM fee {} below {floor} (social_min_fee {} + social_byte_fee {} x {len} bytes, at least min_fee)", tx.fee, cfg.social_min_fee, cfg.social_byte_fee));
    }
    Ok(())
}

/// Read the DM parameters from a state version (missing record = defaults).
pub async fn read_dm_params<S: StateRead>(st: &S) -> Result<DmParams, state::StateError> {
    match st.get(&sd::dm_params_key()).await? {
        Some(b) => DmParams::decode(&b).map_err(|e| state::StateError::Corrupt(format!("dm params: {e}"))),
        None => Ok(DmParams::default()),
    }
}

fn check_blob(uri: &[u8], hash: &Hash) -> Result<(), TxError> {
    if uri.is_empty() {
        if *hash != [0; 32] {
            return bad("blob_hash must be zero without a blob_uri");
        }
        return Ok(());
    }
    match blobs::parse_uri(uri) {
        Some(id) if id == *hash => Ok(()),
        Some(_) => bad("blob_hash must be the blob id of blob_uri"),
        None => bad("blob_uri must be a blob: URI"),
    }
}

impl<'a, S: StateRead> BlockExecutor<'a, S> {
    /// Activation and fee rule of a DM tx (called before the fee is taken).
    pub(crate) fn check_dm(&self, tx: &Tx) -> Result<(), TxError> {
        precheck_dm(tx, &self.cfg, self.env.height).map_err(TxError::Invalid)
    }

    async fn dm_params(&self) -> Result<DmParams, TxError> {
        match self.ov.get(&sd::dm_params_key()).await? {
            Some(b) => DmParams::decode(&b).map_err(corrupt),
            None => Ok(DmParams::default()),
        }
    }

    async fn dm_stats(&self, a: &Pubkey) -> Result<DmStats, TxError> {
        match self.ov.get(&sd::dm_stats_key(a)).await? {
            Some(b) => DmStats::decode(&b).map_err(corrupt),
            None => Ok(DmStats::default()),
        }
    }

    fn put_dm_stats(&mut self, a: &Pubkey, s: &DmStats) {
        let v = if *s == DmStats::default() { None } else { Some(s.encode()) };
        self.ov.put(sd::dm_stats_key(a), v);
    }

    async fn channel(&self, id: &Hash) -> Result<Option<Channel>, TxError> {
        match self.ov.get(&sd::channel_key(id)).await? {
            Some(b) => Channel::decode(&b).map(Some).map_err(corrupt),
            None => Ok(None),
        }
    }

    async fn is_member(&self, channel: &Hash, a: &Pubkey) -> Result<bool, TxError> {
        Ok(self.ov.get(&sd::member_key(channel, a)).await?.is_some())
    }

    async fn enc_key_of(&self, a: &Pubkey) -> Result<Pubkey, TxError> {
        Ok(match self.ov.get(&ss::profile_key(a)).await? {
            Some(b) => ss::Profile::decode(&b).map_err(corrupt)?.enc_key,
            None => [0; 32],
        })
    }

    /// Move `amount` (already taken from the payer) to `payee`, with the protocol share
    /// (`dm_protocol_bps`) to the pool.
    async fn settle_pay(&mut self, params: &DmParams, payer: &Pubkey, payee: &Pubkey, amount: u64) -> Result<(), TxError> {
        if amount == 0 {
            return Ok(());
        }
        let cut = ((amount as u128 * params.protocol_bps as u128) / 10_000) as u64;
        if cut > 0 {
            self.ov.credit(&pool_address(), cut).await?;
        }
        self.ov.credit(payee, amount - cut).await?;
        let mut p = self.dm_stats(payer).await?;
        p.pay_out = p.pay_out.saturating_add(amount);
        self.put_dm_stats(payer, &p);
        let mut q = self.dm_stats(payee).await?;
        q.pay_in = q.pay_in.saturating_add(amount - cut);
        self.put_dm_stats(payee, &q);
        Ok(())
    }

    /// AdminConfig for keys 35-44 (the admin check is done by the caller's arm).
    pub(crate) async fn admin_dm_config(&mut self, from: &Pubkey, key: u8, value: &[u8]) -> Result<(), TxError> {
        if *from != self.cfg.admin {
            return bad("not the admin");
        }
        let act = dm::dm_upgrade_height(self.env.chain_id);
        if self.env.height < act {
            return bad(format!("config key {key} activates at height {act}"));
        }
        if key > sd::keys::LAST {
            return bad(format!("config key {key} is reserved"));
        }
        let mut p = self.dm_params().await?;
        p.set(key, value).map_err(TxError::Invalid)?;
        let v = if p == DmParams::default() { None } else { Some(p.encode()) };
        self.ov.put(sd::dm_params_key(), v);
        Ok(())
    }

    pub(crate) async fn send_message(&mut self, from: &Pubkey, nonce: u64, m: &SendMessage) -> Result<(), TxError> {
        let h = self.env.height;
        let params = self.dm_params().await?;
        if m.mode > dm::MODE_SEALED {
            return bad("mode must be 0 (plain) or 1 (sealed)");
        }
        if m.body.len() > params.msg_inline_max as usize {
            return bad(format!("message body {} bytes above msg_inline_max {}", m.body.len(), params.msg_inline_max));
        }
        check_blob(&m.blob_uri, &m.blob_hash)?;
        if m.body.is_empty() && m.blob_uri.is_empty() {
            return bad("a message needs a body or a blob");
        }
        let id = dm::message_id(from, nonce);
        let to_set = m.to != [0; 32];
        let ch_set = m.channel != [0; 32];
        if to_set == ch_set {
            return bad("set exactly one of to (a DM) or channel");
        }
        if ch_set {
            return self.channel_message(from, m).await;
        }
        // ---- a DM
        if m.to == *from {
            return bad("cannot message yourself");
        }
        if is_reserved(&m.to) {
            return bad("not a messageable account");
        }
        match m.mode {
            dm::MODE_PLAIN => {
                if std::str::from_utf8(&m.body).is_err() {
                    return bad("a plain message body must be UTF-8");
                }
            }
            _ => {
                if !m.body.is_empty() {
                    dm::check_sealed_dm(&m.body).map_err(|e| TxError::Invalid(e.into()))?;
                }
                if self.enc_key_of(&m.to).await? == [0; 32] {
                    return bad("the recipient has published no enc_key (SetProfile), so it cannot open a sealed message");
                }
            }
        }
        // consent: a denied sender is refused outright
        let consent = match self.ov.get(&sd::consent_key(&m.to, from)).await? {
            Some(b) => Some(sd::decode_consent(&b).map_err(corrupt)?.0),
            None => None,
        };
        if consent == Some(dm::CONSENT_DENY) {
            return bad("the recipient has blocked you");
        }
        // policy and price (the recipient's profile; no profile = open, free)
        let prof = match self.ov.get(&ss::profile_key(&m.to)).await? {
            Some(b) => Some(ss::Profile::decode(&b).map_err(corrupt)?),
            None => None,
        };
        let allowed = consent == Some(dm::CONSENT_ALLOW);
        let followed = self.ov.get(&ss::follow_key(&m.to, from)).await?.is_some();
        let in_thread = match self.ov.get(&sd::dm_last_key(&m.to, from)).await? {
            Some(b) => {
                let hr = sd::decode_u64(tag::DM_LAST, &b).map_err(corrupt)?;
                let window = prof.as_ref().map(|p| p.thread_free_blocks).unwrap_or(0);
                window == 0 || h <= hr.saturating_add(window as u64)
            }
            None => false,
        };
        if let Some(p) = &prof {
            let passes = allowed || in_thread;
            match p.dm_policy {
                dm::POLICY_OPEN => {}
                dm::POLICY_PRICED => {
                    if !passes && !followed && m.pay < p.dm_price {
                        return bad(format!("the recipient takes cold DMs at a price of {}: pay {} is short", p.dm_price, m.pay));
                    }
                }
                dm::POLICY_FOLLOWED => {
                    if !passes && !followed {
                        return bad("the recipient takes DMs only from accounts it follows");
                    }
                }
                _ => {
                    if !passes {
                        return bad("the recipient takes no new DMs");
                    }
                }
            }
        }
        // pay: settle now, or hold in escrow until a reply or the window ends
        if m.reclaim_after > 0 {
            if m.pay == 0 {
                return bad("reclaim_after needs pay > 0");
            }
            if !(params.reclaim_min..=params.reclaim_max).contains(&m.reclaim_after) {
                return bad(format!("reclaim_after must be {}..={} blocks", params.reclaim_min, params.reclaim_max));
            }
        }
        if m.pay > 0 {
            self.ov.debit(from, m.pay).await?;
            if m.reclaim_after > 0 {
                let mut st = self.dm_stats(from).await?;
                if st.open_escrows >= params.max_open_escrows as u64 {
                    return bad(format!("at most {} open escrows per sender", params.max_open_escrows));
                }
                st.open_escrows += 1;
                st.locked = st.locked.saturating_add(m.pay);
                self.put_dm_stats(from, &st);
                let e = MsgEscrow { from: *from, to: m.to, pay: m.pay, height: h, deadline: h + m.reclaim_after as u64 };
                self.ov.put(sd::escrow_key(&id), Some(e.encode()));
            } else {
                self.settle_pay(&params, from, &m.to, m.pay).await?;
            }
        }
        // a reply in time releases the escrow it answers
        if m.reply_to != [0; 32] {
            if let Some(b) = self.ov.get(&sd::escrow_key(&m.reply_to)).await? {
                let e = MsgEscrow::decode(&b).map_err(corrupt)?;
                if e.to == *from && e.from == m.to && h <= e.deadline {
                    self.ov.put(sd::escrow_key(&m.reply_to), None);
                    let mut s = self.dm_stats(&e.from).await?;
                    s.open_escrows -= 1;
                    s.locked -= e.pay;
                    self.put_dm_stats(&e.from, &s);
                    self.settle_pay(&params, &e.from, from, e.pay).await?;
                    let mut me = self.dm_stats(from).await?;
                    me.answered += 1;
                    self.put_dm_stats(from, &me);
                }
            }
        }
        self.ov.put(sd::dm_last_key(from, &m.to), Some(sd::encode_u64(tag::DM_LAST, h)));
        let mut me = self.dm_stats(from).await?;
        me.sent += 1;
        self.put_dm_stats(from, &me);
        let mut them = self.dm_stats(&m.to).await?;
        them.received += 1;
        self.put_dm_stats(&m.to, &them);
        Ok(())
    }

    async fn channel_message(&mut self, from: &Pubkey, m: &SendMessage) -> Result<(), TxError> {
        let Some(mut c) = self.channel(&m.channel).await? else { return bad("unknown channel") };
        if !self.is_member(&m.channel, from).await? {
            return bad("only members post in a channel");
        }
        if m.pay != 0 || m.reclaim_after != 0 {
            return bad("channel messages carry no pay");
        }
        if m.mode != c.mode {
            return bad(if c.mode == dm::MODE_SEALED { "this channel is sealed: send mode 1" } else { "this channel is plain: send mode 0" });
        }
        if c.mode == dm::MODE_PLAIN {
            if std::str::from_utf8(&m.body).is_err() {
                return bad("a plain message body must be UTF-8");
            }
        } else {
            if c.epoch == 0 {
                return bad("no channel key yet: an admin must post ChannelKey epoch 1");
            }
            if c.rekey_due {
                return bad("a member left or was removed: an admin must post a new key epoch first");
            }
            if !m.body.is_empty() {
                let e = dm::check_sealed_channel(&m.body).map_err(|e| TxError::Invalid(e.into()))?;
                if e != c.epoch {
                    return bad(format!("sealed under epoch {e}, the channel is at epoch {}", c.epoch));
                }
            }
        }
        c.messages += 1;
        self.ov.put(sd::channel_key(&m.channel), Some(c.encode()));
        let mut me = self.dm_stats(from).await?;
        me.sent += 1;
        self.put_dm_stats(from, &me);
        Ok(())
    }

    pub(crate) async fn reclaim_message(&mut self, from: &Pubkey, id: &Hash) -> Result<(), TxError> {
        let Some(b) = self.ov.get(&sd::escrow_key(id)).await? else { return bad("no escrow for this message") };
        let e = MsgEscrow::decode(&b).map_err(corrupt)?;
        if *from == e.from {
            if self.env.height <= e.deadline {
                return bad(format!("the reply window is open until height {}", e.deadline));
            }
        } else if *from != e.to {
            return bad("only the sender (after the window) or the recipient (to decline) can reclaim");
        }
        self.ov.put(sd::escrow_key(id), None);
        self.ov.credit(&e.from, e.pay).await?;
        let mut s = self.dm_stats(&e.from).await?;
        s.open_escrows -= 1;
        s.locked -= e.pay;
        s.refunded += 1;
        self.put_dm_stats(&e.from, &s);
        Ok(())
    }

    pub(crate) async fn set_consent(&mut self, from: &Pubkey, items: &[Consent]) -> Result<(), TxError> {
        for (i, x) in items.iter().enumerate() {
            if x.peer == *from || x.peer == [0; 32] || is_reserved(&x.peer) {
                return bad(format!("consent {i}: not a valid peer"));
            }
            let k = sd::consent_key(from, &x.peer);
            if x.state == dm::CONSENT_CLEAR {
                self.ov.put(k, None);
            } else {
                self.ov.put(k, Some(sd::encode_consent(x.state, self.env.height)));
            }
        }
        Ok(())
    }

    fn check_channel_fields(&self, params: &DmParams, c: &ChannelConfig, creator: &Pubkey) -> Result<(), TxError> {
        if c.name.is_empty() || std::str::from_utf8(&c.name).is_err() {
            return bad("channel name must be 1..=64 bytes of UTF-8");
        }
        if std::str::from_utf8(&c.about).is_err() {
            return bad("channel about must be UTF-8");
        }
        if c.policy > dm::CHANNEL_PAID {
            return bad("policy must be 0 (open), 1 (invite) or 2 (paid join)");
        }
        if (c.policy == dm::CHANNEL_PAID) != (c.join_price > 0) {
            return bad("join_price must be positive exactly for a paid channel");
        }
        if c.mode > dm::MODE_SEALED {
            return bad("mode must be 0 (plain) or 1 (sealed)");
        }
        if c.admins.len() > params.channel_max_admins as usize {
            return bad(format!("at most {} admins", params.channel_max_admins));
        }
        let mut a = c.admins.clone();
        a.sort();
        a.dedup();
        if a.len() != c.admins.len() {
            return bad("duplicate admin");
        }
        if c.admins.iter().any(|x| x == creator || *x == [0; 32] || is_reserved(x)) {
            return bad("admins must be other accounts than the creator");
        }
        Ok(())
    }

    pub(crate) async fn channel_config(&mut self, from: &Pubkey, nonce: u64, c: &ChannelConfig) -> Result<(), TxError> {
        let h = self.env.height;
        let params = self.dm_params().await?;
        if c.channel_id == [0; 32] {
            self.check_channel_fields(&params, c, from)?;
            let id = dm::channel_id(from, nonce);
            if self.ov.get(&sd::channel_key(&id)).await?.is_some() {
                return bad("channel id exists");
            }
            if params.channel_create_fee > 0 {
                self.ov.debit(from, params.channel_create_fee).await?;
                self.ov.credit(&pool_address(), params.channel_create_fee).await?;
            }
            let n = match self.ov.get(&sd::channel_count_key()).await? {
                Some(b) => sd::decode_u64(tag::CHANNEL_COUNT, &b).map_err(corrupt)?,
                None => 0,
            };
            self.ov.put(sd::channel_index_key(n), Some(sd::encode_id(tag::CHANNEL_INDEX, &id)));
            self.ov.put(sd::channel_count_key(), Some(sd::encode_u64(tag::CHANNEL_COUNT, n + 1)));
            // the creator and the admins named at creation are members from the start
            self.ov.put(sd::member_key(&id, from), Some(sd::encode_u64(tag::CHANNEL_MEMBER, h)));
            for a in &c.admins {
                self.ov.put(sd::member_key(&id, a), Some(sd::encode_u64(tag::CHANNEL_MEMBER, h)));
            }
            if c.mode == dm::MODE_SEALED && 1 + c.admins.len() as u32 > params.sealed_max_members {
                return bad(format!("a sealed channel holds at most {} members", params.sealed_max_members));
            }
            let rec = Channel {
                creator: *from,
                name: c.name.clone(),
                about: c.about.clone(),
                policy: c.policy,
                join_price: c.join_price,
                mode: c.mode,
                admins: c.admins.clone(),
                members: 1 + c.admins.len() as u32,
                epoch: 0,
                epoch_height: 0,
                rekey_due: false,
                messages: 0,
                created_height: h,
                set_height: h,
                index: n,
            };
            self.ov.put(sd::channel_key(&id), Some(rec.encode()));
            return Ok(());
        }
        let Some(mut rec) = self.channel(&c.channel_id).await? else { return bad("unknown channel") };
        if !rec.is_admin(from) {
            return bad("only the channel's creator or admins can change it");
        }
        self.check_channel_fields(&params, c, &rec.creator)?;
        if c.mode != rec.mode {
            return bad("a channel's mode is fixed at creation");
        }
        if *from != rec.creator && c.admins != rec.admins {
            return bad("only the creator changes the admin list");
        }
        for (i, a) in c.admins.iter().enumerate() {
            if !self.is_member(&c.channel_id, a).await? {
                return bad(format!("admin {i} must be a member first"));
            }
        }
        rec.name = c.name.clone();
        rec.about = c.about.clone();
        rec.policy = c.policy;
        rec.join_price = c.join_price;
        rec.admins = c.admins.clone();
        rec.set_height = h;
        self.ov.put(sd::channel_key(&c.channel_id), Some(rec.encode()));
        Ok(())
    }

    pub(crate) async fn channel_member(&mut self, from: &Pubkey, channel: &Hash, action: u8, member: &Pubkey, pay: u64) -> Result<(), TxError> {
        let h = self.env.height;
        let params = self.dm_params().await?;
        let Some(mut c) = self.channel(channel).await? else { return bad("unknown channel") };
        match action {
            dm::MEMBER_JOIN | dm::MEMBER_LEAVE => {
                if *member != [0; 32] && member != from {
                    return bad("join and leave name the sender (or zero)");
                }
            }
            _ => {
                if !c.is_admin(from) {
                    return bad("only the channel's creator or admins invite and remove");
                }
                if *member == [0; 32] || is_reserved(member) {
                    return bad("member must be an account");
                }
                if pay != 0 {
                    return bad("invite and remove carry no pay");
                }
            }
        }
        let who = if *member == [0; 32] { *from } else { *member };
        let mk = sd::member_key(channel, &who);
        let is_member = self.ov.get(&mk).await?.is_some();
        match action {
            dm::MEMBER_JOIN => {
                if is_member {
                    return bad("already a member");
                }
                let ik = sd::invite_key(channel, &who);
                let invited = self.ov.get(&ik).await?.is_some();
                if c.mode == dm::MODE_SEALED && c.members >= params.sealed_max_members {
                    return bad(format!("a sealed channel holds at most {} members", params.sealed_max_members));
                }
                if invited {
                    if pay != 0 {
                        return bad("an invited join carries no pay");
                    }
                    self.ov.put(ik, None);
                } else {
                    match c.policy {
                        dm::CHANNEL_OPEN => {
                            if pay != 0 {
                                return bad("an open channel's join carries no pay");
                            }
                        }
                        dm::CHANNEL_INVITE => return bad("this channel is invite only"),
                        _ => {
                            if pay < c.join_price {
                                return bad(format!("joining costs {}: pay {} is short", c.join_price, pay));
                            }
                            self.ov.debit(from, pay).await?;
                            let creator = c.creator;
                            self.settle_pay(&params, from, &creator, pay).await?;
                        }
                    }
                }
                self.ov.put(mk, Some(sd::encode_u64(tag::CHANNEL_MEMBER, h)));
                c.members += 1;
            }
            dm::MEMBER_LEAVE | dm::MEMBER_REMOVE => {
                if !is_member {
                    return bad("not a member");
                }
                if who == c.creator {
                    return bad("the creator cannot leave or be removed");
                }
                if action == dm::MEMBER_REMOVE && c.admins.contains(&who) && *from != c.creator {
                    return bad("only the creator removes an admin");
                }
                if action == dm::MEMBER_LEAVE && pay != 0 {
                    return bad("leave carries no pay");
                }
                self.ov.put(mk, None);
                c.admins.retain(|a| *a != who);
                c.members -= 1;
                if c.mode == dm::MODE_SEALED && c.epoch > 0 {
                    c.rekey_due = true;
                }
            }
            _ => {
                // invite
                if is_member {
                    return bad("already a member");
                }
                let ik = sd::invite_key(channel, &who);
                if self.ov.get(&ik).await?.is_some() {
                    return bad("already invited");
                }
                self.ov.put(ik, Some(sd::encode_u64(tag::CHANNEL_INVITE, h)));
            }
        }
        self.ov.put(sd::channel_key(channel), Some(c.encode()));
        Ok(())
    }

    pub(crate) async fn channel_key(&mut self, from: &Pubkey, channel: &Hash, epoch: u32, envelopes: &[KeyEnvelope]) -> Result<(), TxError> {
        let Some(mut c) = self.channel(channel).await? else { return bad("unknown channel") };
        if c.mode != dm::MODE_SEALED {
            return bad("keys are for sealed channels");
        }
        if !c.is_admin(from) || !self.is_member(channel, from).await? {
            return bad("only a member admin (or the creator) posts channel keys");
        }
        if epoch == c.epoch.saturating_add(1) {
            c.epoch = epoch;
            c.epoch_height = self.env.height;
            c.rekey_due = false;
        } else if !(epoch == c.epoch && epoch > 0) {
            return bad(format!("epoch must be {} (more envelopes) or {} (a new key)", c.epoch, c.epoch + 1));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (i, e) in envelopes.iter().enumerate() {
            if !seen.insert(e.member) {
                return bad(format!("envelope {i}: member named twice"));
            }
            if !self.is_member(channel, &e.member).await? {
                return bad(format!("envelope {i}: not a member"));
            }
            if self.enc_key_of(&e.member).await? == [0; 32] {
                return bad(format!("envelope {i}: the member has no enc_key"));
            }
        }
        self.ov.put(sd::channel_key(channel), Some(c.encode()));
        Ok(())
    }
}
