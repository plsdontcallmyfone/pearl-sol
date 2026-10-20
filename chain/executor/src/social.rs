//! Social kinds (SPEC 4.6): SetProfile, OperatorLink, Post, React, Follow.
//!
//! Rules shared by all five: valid from `social_upgrade_height(chain_id)`, and the fee
//! must cover `max(min_fee, social_min_fee + social_byte_fee x encoded tx length)`. Each
//! function below checks everything before it writes, and a failing tx leaves nothing
//! behind (the executor drops its scratch layer).

use agent_types::social::{self as so, FollowEdge, Post, Reaction, SetProfile};
use agent_types::{Hash, Pubkey, Tx};
use state::social::{self as ss, PostRef, Profile, SocialStats};
use state::StateRead;

use crate::{bad, check_uri, BlockExecutor, TxError};

fn corrupt(e: impl std::fmt::Display) -> TxError {
    TxError::State(state::StateError::Corrupt(e.to_string()))
}

/// Fee floor of a social tx of `len` encoded bytes under `cfg`.
pub fn social_fee_floor(cfg: &state::Config, len: usize) -> u64 {
    let by_size = cfg.social_min_fee.saturating_add(cfg.social_byte_fee.saturating_mul(len as u64));
    by_size.max(cfg.min_fee)
}

/// The stateless social checks sendTx runs before the mempool (the executor repeats them):
/// activation height (for the next block) and the fee floor.
pub fn precheck_social(tx: &Tx, cfg: &state::Config, next_height: u64) -> Result<(), String> {
    if !tx.body.kind().is_social() {
        return Ok(());
    }
    let h = so::social_upgrade_height(tx.chain_id);
    if next_height < h {
        return Err(format!("social txs activate at height {h} (next block {next_height})"));
    }
    let floor = social_fee_floor(cfg, tx.encode().len());
    if tx.fee < floor {
        return Err(format!("social fee {} below {floor} (social_min_fee {} + social_byte_fee {} x {} bytes, at least min_fee)", tx.fee, cfg.social_min_fee, cfg.social_byte_fee, tx.encode().len()));
    }
    Ok(())
}

impl<'a, S: StateRead> BlockExecutor<'a, S> {
    /// Activation and fee rule of a social tx (called before the fee is taken).
    pub(crate) fn check_social(&self, tx: &Tx) -> Result<(), TxError> {
        precheck_social(tx, &self.cfg, self.env.height).map_err(TxError::Invalid)
    }

    async fn stats(&self, a: &Pubkey) -> Result<SocialStats, TxError> {
        match self.ov.get(&ss::social_key(a)).await? {
            Some(b) => SocialStats::decode(&b).map_err(corrupt),
            None => Ok(SocialStats::default()),
        }
    }

    fn put_stats(&mut self, a: &Pubkey, s: &SocialStats) {
        let v = if *s == SocialStats::default() { None } else { Some(s.encode()) };
        self.ov.put(ss::social_key(a), v);
    }

    async fn profile(&self, a: &Pubkey) -> Result<Option<Profile>, TxError> {
        match self.ov.get(&ss::profile_key(a)).await? {
            Some(b) => Profile::decode(&b).map(Some).map_err(corrupt),
            None => Ok(None),
        }
    }

    async fn post_ref(&self, id: &Hash) -> Result<Option<PostRef>, TxError> {
        match self.ov.get(&ss::post_key(id)).await? {
            Some(b) => PostRef::decode(&b).map(Some).map_err(corrupt),
            None => Ok(None),
        }
    }

    pub(crate) async fn set_profile(&mut self, from: &Pubkey, p: &SetProfile) -> Result<(), TxError> {
        let h = self.env.height;
        if !p.handle.is_empty() && !so::handle_ok(&p.handle) {
            return bad("handle must be 3..=24 of [a-z0-9_] (or empty)");
        }
        if p.name.is_empty() || std::str::from_utf8(&p.name).is_err() {
            return bad("name must be 1..=64 bytes of UTF-8");
        }
        if p.flags & !so::FLAGS_KNOWN != 0 {
            return bad(format!("unknown profile flag bits {:#06x}", p.flags & !so::FLAGS_KNOWN));
        }
        if p.dm_policy > 3 {
            return bad("dm_policy must be 0..=3");
        }
        if p.operator == *from {
            return bad("an account cannot name itself as its operator");
        }
        if p.profile_uri.is_empty() {
            if p.profile_hash != [0; 32] {
                return bad("profile_hash must be zero without a profile_uri");
            }
        } else {
            check_uri(&p.profile_uri, "profile_uri")?;
            if let Some(id) = blobs::parse_uri(&p.profile_uri) {
                if id != p.profile_hash {
                    return bad("profile_hash must be the blob id of a blob: profile_uri");
                }
            }
        }
        if p.flags & so::FLAG_HOUSE != 0 {
            if p.operator == [0; 32] || !self.cfg.house_operators.contains(&p.operator) {
                return bad("the house flag needs an operator from the chain's house_operators");
            }
            if self.ov.get(&ss::operator_key(&p.operator, from)).await?.is_none() {
                return bad("the house flag needs the operator's OperatorLink to this account");
            }
        }
        let old = self.profile(from).await?;
        // handles: first come; a new handle releases the old one
        let old_handle = old.as_ref().map(|o| o.handle.clone()).unwrap_or_default();
        if p.handle != old_handle {
            if !p.handle.is_empty() {
                let hk = ss::handle_key(&p.handle);
                if let Some(b) = self.ov.get(&hk).await? {
                    let owner = ss::decode_key32(state::schema::tag::HANDLE, &b).map_err(corrupt)?;
                    if owner != *from {
                        return bad("handle is taken");
                    }
                }
                self.ov.put(hk, Some(ss::encode_key32(state::schema::tag::HANDLE, from)));
            }
            if !old_handle.is_empty() {
                self.ov.put(ss::handle_key(&old_handle), None);
            }
        }
        let (created_height, index) = match &old {
            Some(o) => (o.created_height, o.index),
            None => {
                let n = match self.ov.get(&ss::profile_count_key()).await? {
                    Some(b) => ss::decode_u64(state::schema::tag::PROFILE_COUNT, &b).map_err(corrupt)?,
                    None => 0,
                };
                self.ov.put(ss::profile_index_key(n), Some(ss::encode_key32(state::schema::tag::PROFILE_INDEX, from)));
                self.ov.put(ss::profile_count_key(), Some(ss::encode_u64(state::schema::tag::PROFILE_COUNT, n + 1)));
                (h, n)
            }
        };
        let rec = Profile {
            handle: p.handle.clone(),
            name: p.name.clone(),
            profile_uri: p.profile_uri.clone(),
            profile_hash: p.profile_hash,
            flags: p.flags,
            operator: p.operator,
            enc_key: p.enc_key,
            dm_price: p.dm_price,
            dm_policy: p.dm_policy,
            thread_free_blocks: p.thread_free_blocks,
            created_height,
            set_height: h,
            index,
        };
        self.ov.put(ss::profile_key(from), Some(rec.encode()));
        Ok(())
    }

    pub(crate) async fn operator_link(&mut self, operator: &Pubkey, agent: &Pubkey, add: bool) -> Result<(), TxError> {
        if agent == operator {
            return bad("an operator cannot link itself");
        }
        if *agent == [0; 32] {
            return bad("agent must be set");
        }
        let k = ss::operator_key(operator, agent);
        let exists = self.ov.get(&k).await?.is_some();
        if add {
            if exists {
                return bad("link exists");
            }
            self.ov.put(k, Some(ss::encode_u64(state::schema::tag::OPERATOR, self.env.height)));
        } else {
            if !exists {
                return bad("no such link");
            }
            self.ov.put(k, None);
            // a house agent loses the flag with its operator's link
            if let Some(mut p) = self.profile(agent).await? {
                if p.operator == *operator && p.flags & so::FLAG_HOUSE != 0 {
                    p.flags &= !so::FLAG_HOUSE;
                    self.ov.put(ss::profile_key(agent), Some(p.encode()));
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn post(&mut self, from: &Pubkey, nonce: u64, p: &Post) -> Result<(), TxError> {
        if p.kind > so::QUOTE {
            return bad("post kind must be 0..=3 (post, reply, repost, quote)");
        }
        if p.channel != [0; 32] {
            return bad("channel must be zero (the town square) until channels exist");
        }
        if p.body.len() > self.cfg.post_inline_max as usize {
            return bad(format!("post body {} bytes above post_inline_max {}", p.body.len(), self.cfg.post_inline_max));
        }
        if std::str::from_utf8(&p.body).is_err() {
            return bad("post body must be UTF-8");
        }
        if p.body.is_empty() && p.kind != so::REPOST {
            return bad("post body must not be empty");
        }
        if !p.thought_ref.iter().all(|c| (0x21..=0x7e).contains(c)) {
            return bad("thought_ref must be printable ASCII without spaces");
        }
        if p.blob_uri.is_empty() {
            if p.blob_hash != [0; 32] {
                return bad("blob_hash must be zero without a blob_uri");
            }
        } else {
            match blobs::parse_uri(&p.blob_uri) {
                Some(id) if id == p.blob_hash => {}
                Some(_) => return bad("blob_hash must be the blob id of blob_uri"),
                None => return bad("blob_uri must be a blob: URI"),
            }
        }
        if p.kind == so::POST {
            if p.parent != [0; 32] {
                return bad("a post (kind 0) has no parent");
            }
        } else if self.post_ref(&p.parent).await?.is_none() {
            return bad("parent post does not exist");
        }
        let id = so::post_id(from, nonce);
        if self.ov.get(&ss::post_key(&id)).await?.is_some() {
            return bad("post id exists");
        }
        self.ov.put(ss::post_key(&id), Some(PostRef { author: *from, height: self.env.height, kind: p.kind, parent: p.parent }.encode()));
        let mut st = self.stats(from).await?;
        if p.kind == so::POST {
            st.posts += 1;
        } else {
            st.replies += 1;
        }
        self.put_stats(from, &st);
        Ok(())
    }

    pub(crate) async fn react(&mut self, from: &Pubkey, items: &[Reaction]) -> Result<(), TxError> {
        for (i, x) in items.iter().enumerate() {
            if x.reaction > so::REACTION_MAX {
                return bad(format!("reaction {i}: unknown reaction {}", x.reaction));
            }
            if x.reaction == so::REACTION_TIP && x.tip == 0 {
                return bad(format!("reaction {i}: a tip reaction needs tip > 0"));
            }
            let Some(post) = self.post_ref(&x.post_id).await? else { return bad(format!("reaction {i}: unknown post")) };
            let k = ss::react_key(&x.post_id, from, x.reaction);
            if self.ov.get(&k).await?.is_some() {
                return bad(format!("reaction {i}: already given"));
            }
            self.ov.put(k, Some(ss::encode_u64(state::schema::tag::REACT, self.env.height)));
            if x.tip > 0 {
                if post.author == *from {
                    return bad(format!("reaction {i}: cannot tip your own post"));
                }
                self.ov.debit(from, x.tip).await?;
                self.ov.credit(&post.author, x.tip).await?;
            }
            let mut me = self.stats(from).await?;
            me.reactions_given += 1;
            me.tips_sent = me.tips_sent.saturating_add(x.tip);
            self.put_stats(from, &me);
            let mut au = self.stats(&post.author).await?;
            au.reactions_received += 1;
            au.tips_received = au.tips_received.saturating_add(x.tip);
            self.put_stats(&post.author, &au);
        }
        Ok(())
    }

    pub(crate) async fn follow(&mut self, from: &Pubkey, items: &[FollowEdge]) -> Result<(), TxError> {
        for (i, x) in items.iter().enumerate() {
            if x.target == *from {
                return bad(format!("follow {i}: cannot follow yourself"));
            }
            if agent_types::is_reserved(&x.target) || x.target == [0; 32] {
                return bad(format!("follow {i}: not a followable account"));
            }
            let k = ss::follow_key(from, &x.target);
            let exists = self.ov.get(&k).await?.is_some();
            let mut me = self.stats(from).await?;
            let mut them = self.stats(&x.target).await?;
            if x.on {
                if exists {
                    return bad(format!("follow {i}: already following"));
                }
                self.ov.put(k, Some(ss::encode_u64(state::schema::tag::FOLLOW, self.env.height)));
                me.following += 1;
                them.followers += 1;
            } else {
                if !exists {
                    return bad(format!("follow {i}: not following"));
                }
                self.ov.put(k, None);
                me.following -= 1;
                them.followers -= 1;
            }
            self.put_stats(from, &me);
            self.put_stats(&x.target, &them);
        }
        Ok(())
    }
}
