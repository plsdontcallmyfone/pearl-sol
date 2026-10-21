//! RPC reads of the DM and channel records (SPEC 4.10, docs/RPC.md): getDmParams,
//! getDmGate, getDmStats, getDmThread, getConsent, getMessageEscrow, getMessage, getChannel,
//! listChannels, getChannelMember.
//!
//! Messages themselves are not in state (only escrows, per-pair last heights, consent and
//! counters); their bodies live in blocks, which getMessage and the archive indexer read.

use agent_types::dm::{self as dm, message_id};
use agent_types::{Hash, Pubkey, TxBody};
use commonware_runtime::Spawner;
use consensus_pow::PowVerifier;
use serde_json::{json, Value};
use state::dm::{self as sd, Channel, DmParams, DmStats, MsgEscrow};
use state::schema::tag;
use state::social as ss;

use crate::core::Core;
use crate::dispatch::{b58, parse32, RpcError};

type R = Result<Value, RpcError>;

/// Methods served by [`Core::dm_dispatch`].
pub const DM_METHODS: [&str; 10] = [
    "getDmParams", "getDmGate", "getDmStats", "getDmThread", "getConsent", "getMessageEscrow", "getMessage",
    "getChannel", "listChannels", "getChannelMember",
];

fn corrupt(e: impl std::fmt::Display) -> RpcError {
    RpcError::server(e)
}

fn key_of(p: &Value, keys: &[&str]) -> Result<[u8; 32], RpcError> {
    let s = keys.iter().find_map(|k| p.get(*k).and_then(|v| v.as_str())).ok_or_else(|| RpcError::params(format!("missing {}", keys[0])))?;
    parse32(s)
}

fn u64_of(p: &Value, k: &str) -> Option<u64> {
    p.get(k).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

pub fn dm_params_json(d: &DmParams) -> Value {
    json!({
        "msg_inline_max": d.msg_inline_max, "dm_reclaim_min": d.reclaim_min, "dm_reclaim_max": d.reclaim_max,
        "dm_max_open_escrows": d.max_open_escrows, "dm_protocol_bps": d.protocol_bps,
        "channel_sealed_max_members": d.sealed_max_members, "channel_create_fee": d.channel_create_fee.to_string(),
        "channel_max_admins": d.channel_max_admins,
        "keys": (sd::keys::FIRST..=sd::keys::LAST).map(|k| json!({"key": k, "name": sd::keys::name(k)})).collect::<Vec<_>>(),
    })
}

pub fn dm_stats_json(s: &DmStats) -> Value {
    json!({
        "sent": s.sent, "received": s.received, "pay_in": s.pay_in.to_string(), "pay_out": s.pay_out.to_string(),
        "open_escrows": s.open_escrows, "locked": s.locked.to_string(), "answered": s.answered, "refunded": s.refunded,
    })
}

pub fn escrow_json(id: &Hash, e: &MsgEscrow, next_height: u64) -> Value {
    json!({
        "message_id": hex::encode(id), "from": b58(&e.from), "to": b58(&e.to), "pay": e.pay.to_string(),
        "height": e.height, "deadline": e.deadline,
        "reply_open": next_height <= e.deadline,
        "blocks_left": e.deadline.saturating_sub(next_height.saturating_sub(1)),
        "reclaimable": next_height > e.deadline,
    })
}

pub fn channel_json(id: &Hash, c: &Channel) -> Value {
    let policy_name = ["open", "invite", "paid"].get(c.policy as usize).copied().unwrap_or("unknown");
    json!({
        "channel_id": hex::encode(id), "creator": b58(&c.creator), "name": String::from_utf8_lossy(&c.name),
        "about": String::from_utf8_lossy(&c.about), "policy": c.policy,
        "policy_name": policy_name,
        "join_price": c.join_price.to_string(), "mode": c.mode, "sealed": c.mode == dm::MODE_SEALED,
        "admins": c.admins.iter().map(|a| b58(a)).collect::<Vec<_>>(), "members": c.members,
        "epoch": c.epoch, "epoch_height": c.epoch_height, "rekey_due": c.rekey_due, "messages": c.messages,
        "created_height": c.created_height, "set_height": c.set_height, "index": c.index,
    })
}

fn consent_name(s: Option<u8>) -> Value {
    match s {
        Some(dm::CONSENT_ALLOW) => json!("allow"),
        Some(dm::CONSENT_DENY) => json!("deny"),
        _ => Value::Null,
    }
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    async fn drec(&self, k: &state::Key) -> Result<Option<Vec<u8>>, RpcError> {
        self.chain.read_tip(k).await.map_err(corrupt)
    }

    pub(crate) async fn dm_params(&self) -> Result<DmParams, RpcError> {
        Ok(match self.drec(&sd::dm_params_key()).await? {
            Some(b) => DmParams::decode(&b).map_err(corrupt)?,
            None => DmParams::default(),
        })
    }

    async fn dm_stats_rec(&self, a: &Pubkey) -> Result<DmStats, RpcError> {
        Ok(match self.drec(&sd::dm_stats_key(a)).await? {
            Some(b) => DmStats::decode(&b).map_err(corrupt)?,
            None => DmStats::default(),
        })
    }

    async fn consent_rec(&self, a: &Pubkey, peer: &Pubkey) -> Result<Option<(u8, u64)>, RpcError> {
        Ok(match self.drec(&sd::consent_key(a, peer)).await? {
            Some(b) => Some(sd::decode_consent(&b).map_err(corrupt)?),
            None => None,
        })
    }

    async fn last_rec(&self, from: &Pubkey, to: &Pubkey) -> Result<Option<u64>, RpcError> {
        Ok(match self.drec(&sd::dm_last_key(from, to)).await? {
            Some(b) => Some(sd::decode_u64(tag::DM_LAST, &b).map_err(corrupt)?),
            None => None,
        })
    }

    async fn channel_rec(&self, id: &Hash) -> Result<Option<Channel>, RpcError> {
        Ok(match self.drec(&sd::channel_key(id)).await? {
            Some(b) => Some(Channel::decode(&b).map_err(corrupt)?),
            None => None,
        })
    }

    /// What a DM from `from` to `to` in the next block needs (the executor's rules, read-only).
    async fn dm_gate(&self, from: &Pubkey, to: &Pubkey) -> R {
        let next = self.chain.tip_height() + 1;
        let prof = self.profile_rec(to).await?;
        let consent = self.consent_rec(to, from).await?.map(|c| c.0);
        let followed = self.drec(&ss::follow_key(to, from)).await?.is_some();
        let last_in = self.last_rec(to, from).await?;
        let window = prof.as_ref().map(|p| p.thread_free_blocks).unwrap_or(0);
        let in_thread = last_in.map(|hr| window == 0 || next <= hr.saturating_add(window as u64)).unwrap_or(false);
        let allowed = consent == Some(dm::CONSENT_ALLOW);
        let blocked = consent == Some(dm::CONSENT_DENY);
        let policy = prof.as_ref().map(|p| p.dm_policy).unwrap_or(dm::POLICY_OPEN);
        let price = prof.as_ref().map(|p| p.dm_price).unwrap_or(0);
        let passes = allowed || in_thread;
        let (can, required, reason) = if blocked {
            (false, 0, "the recipient has blocked you")
        } else {
            match policy {
                dm::POLICY_OPEN => (true, 0, "open"),
                dm::POLICY_PRICED if passes || followed => (true, 0, "free: in a thread the recipient started, followed, or allowed"),
                dm::POLICY_PRICED => (true, price, "priced cold DM"),
                dm::POLICY_FOLLOWED if passes || followed => (true, 0, "followed or allowed"),
                dm::POLICY_FOLLOWED => (false, 0, "the recipient takes DMs only from accounts it follows"),
                _ if passes => (true, 0, "allowed or in a thread the recipient started"),
                _ => (false, 0, "the recipient takes no new DMs"),
            }
        };
        let enc = prof.as_ref().map(|p| p.enc_key).unwrap_or([0; 32]);
        let my_enc = self.profile_rec(from).await?.map(|p| p.enc_key).unwrap_or([0; 32]);
        let d = self.dm_params().await?;
        Ok(json!({
            "from": b58(from), "to": b58(to), "next_height": next,
            "can_send": can, "required_pay": required.to_string(), "reason": reason,
            "dm_policy": policy, "dm_price": price.to_string(), "thread_free_blocks": window,
            "consent": consent_name(consent), "followed_by_recipient": followed, "in_thread": in_thread,
            "recipient_last_dm_height": last_in,
            "enc_key": if enc == [0; 32] { Value::Null } else { json!(b58(&enc)) },
            "sender_enc_key": if my_enc == [0; 32] { Value::Null } else { json!(b58(&my_enc)) },
            "sealed_ok": enc != [0; 32],
            "msg_inline_max": d.msg_inline_max, "reclaim_min": d.reclaim_min, "reclaim_max": d.reclaim_max,
        }))
    }

    /// The SendMessage carrying `id`, read from the block at `height` (null when the block or
    /// the message is not there).
    async fn get_message(&self, id: &Hash, height: u64) -> R {
        let Some(bh) = self.chain.canonical_hash(height) else { return Ok(Value::Null) };
        let Some(blk) = self.chain.get_block(&bh).await.map_err(corrupt)? else { return Ok(Value::Null) };
        for (i, t) in blk.txs.iter().enumerate() {
            let TxBody::SendMessage(m) = &t.body else { continue };
            if message_id(&t.from, t.nonce) != *id {
                continue;
            }
            let esc = match self.drec(&sd::escrow_key(id)).await? {
                Some(b) => escrow_json(id, &MsgEscrow::decode(&b).map_err(corrupt)?, self.chain.tip_height() + 1),
                None => Value::Null,
            };
            let plain = m.mode == dm::MODE_PLAIN;
            return Ok(json!({
                "message_id": hex::encode(id), "from": b58(&t.from), "nonce": t.nonce,
                "to": if m.to == [0; 32] { Value::Null } else { json!(b58(&m.to)) },
                "channel": if m.channel == [0; 32] { Value::Null } else { json!(hex::encode(m.channel)) },
                "mode": m.mode, "sealed": !plain,
                "reply_to": if m.reply_to == [0; 32] { Value::Null } else { json!(hex::encode(m.reply_to)) },
                "pay": m.pay.to_string(), "reclaim_after": m.reclaim_after,
                "body": if plain { json!(String::from_utf8_lossy(&m.body)) } else { Value::Null },
                "body_hex": hex::encode(&m.body),
                "blob_uri": String::from_utf8_lossy(&m.blob_uri),
                "thought_ref": String::from_utf8_lossy(&m.thought_ref),
                "height": height, "block_hash": hex::encode(bh), "index": i, "tx_hash": hex::encode(t.hash()),
                "timestamp_ms": blk.header.timestamp_ms, "escrow": esc,
            }));
        }
        Ok(Value::Null)
    }

    /// Admission check of a DM tx at sendTx: when it is the sender's next nonce, run it
    /// through the executor on the tip state so a refused message (blocked sender, short
    /// price, closed inbox, missing enc_key, non-member) never enters the mempool.
    pub(crate) async fn admit_dm(&self, tx: &agent_types::Tx, account_nonce: u64) -> Result<(), String> {
        if !tx.body.kind().is_dm() || tx.nonce != account_nonce {
            return Ok(());
        }
        let tip = self.chain.tip();
        let view = state::View { store: self.chain.store(), snap: &tip.snapshot };
        let env = executor::BlockEnv { chain_id: self.chain.chain_id(), height: tip.header.height + 1, miner: [0; 32], job_id: None };
        let mut ex = executor::BlockExecutor::begin(&view, env).await.map_err(|e| e.to_string())?;
        ex.apply_tx(tx).await.map(|_| ()).map_err(|e| e.to_string())
    }

    /// Serve the DM methods; `None` when `method` is not one of [`DM_METHODS`].
    pub(crate) async fn dm_dispatch(&self, method: &str, p: &Value) -> Option<R> {
        if !DM_METHODS.contains(&method) {
            return None;
        }
        Some(self.dm_call(method, p).await)
    }

    async fn dm_call(&self, method: &str, p: &Value) -> R {
        let next = self.chain.tip_height() + 1;
        match method {
            "getDmParams" => {
                let mut v = dm_params_json(&self.dm_params().await?);
                let h = dm::dm_upgrade_height(self.chain.chain_id());
                v["activation_height"] = if h == u64::MAX { Value::Null } else { json!(h) };
                v["active"] = json!(next >= h);
                Ok(v)
            }
            "getDmGate" => {
                let from = key_of(p, &["from"])?;
                let to = key_of(p, &["to"])?;
                self.dm_gate(&from, &to).await
            }
            "getDmStats" => {
                let a = key_of(p, &["account", "address"])?;
                let mut v = dm_stats_json(&self.dm_stats_rec(&a).await?);
                v["account"] = json!(b58(&a));
                Ok(v)
            }
            "getDmThread" => {
                let a = key_of(p, &["a", "from"])?;
                let b = key_of(p, &["b", "to"])?;
                Ok(json!({
                    "a": b58(&a), "b": b58(&b),
                    "last_a_to_b": self.last_rec(&a, &b).await?, "last_b_to_a": self.last_rec(&b, &a).await?,
                    "consent_a_for_b": consent_name(self.consent_rec(&a, &b).await?.map(|c| c.0)),
                    "consent_b_for_a": consent_name(self.consent_rec(&b, &a).await?.map(|c| c.0)),
                }))
            }
            "getConsent" => {
                let a = key_of(p, &["account"])?;
                let peer = key_of(p, &["peer"])?;
                let c = self.consent_rec(&a, &peer).await?;
                Ok(json!({"account": b58(&a), "peer": b58(&peer), "state": consent_name(c.map(|x| x.0)), "height": c.map(|x| x.1)}))
            }
            "getMessageEscrow" => {
                let id = key_of(p, &["message_id", "id"])?;
                match self.drec(&sd::escrow_key(&id)).await? {
                    Some(b) => Ok(escrow_json(&id, &MsgEscrow::decode(&b).map_err(corrupt)?, next)),
                    None => Ok(Value::Null),
                }
            }
            "getMessage" => {
                let id = key_of(p, &["message_id", "id"])?;
                let height = match u64_of(p, "height") {
                    Some(h) => h,
                    None => match self.drec(&sd::escrow_key(&id)).await? {
                        Some(b) => MsgEscrow::decode(&b).map_err(corrupt)?.height,
                        None => return Err(RpcError::params("height is required (the message is not in escrow)")),
                    },
                };
                self.get_message(&id, height).await
            }
            "getChannel" => {
                let id = key_of(p, &["channel", "channel_id", "id"])?;
                match self.channel_rec(&id).await? {
                    Some(c) => Ok(channel_json(&id, &c)),
                    None => Ok(Value::Null),
                }
            }
            "listChannels" => {
                let offset = u64_of(p, "offset").unwrap_or(0);
                let limit = u64_of(p, "limit").unwrap_or(50).min(200);
                let n = match self.drec(&sd::channel_count_key()).await? {
                    Some(b) => sd::decode_u64(tag::CHANNEL_COUNT, &b).map_err(corrupt)?,
                    None => 0,
                };
                let mut out = Vec::new();
                for i in offset..n.min(offset.saturating_add(limit)) {
                    let Some(b) = self.drec(&sd::channel_index_key(i)).await? else { continue };
                    let id = sd::decode_id(tag::CHANNEL_INDEX, &b).map_err(corrupt)?;
                    if let Some(c) = self.channel_rec(&id).await? {
                        out.push(channel_json(&id, &c));
                    }
                }
                Ok(json!({"total": n, "offset": offset, "channels": out}))
            }
            _ => {
                // getChannelMember
                let id = key_of(p, &["channel", "channel_id"])?;
                let a = key_of(p, &["account", "member"])?;
                let m = match self.drec(&sd::member_key(&id, &a)).await? {
                    Some(b) => Some(sd::decode_u64(tag::CHANNEL_MEMBER, &b).map_err(corrupt)?),
                    None => None,
                };
                let inv = match self.drec(&sd::invite_key(&id, &a)).await? {
                    Some(b) => Some(sd::decode_u64(tag::CHANNEL_INVITE, &b).map_err(corrupt)?),
                    None => None,
                };
                Ok(json!({"channel_id": hex::encode(id), "account": b58(&a), "member": m.is_some(), "since_height": m,
                          "invited": inv.is_some(), "invited_height": inv}))
            }
        }
    }
}
