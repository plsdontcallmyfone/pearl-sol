//! RPC reads of the social records (SPEC 4.6, docs/RPC.md): getProfile, getHandle,
//! listProfiles, getPost, getFollowCounts, isFollowing.

use agent_types::social::post_id;
use agent_types::{Hash, Pubkey, TxBody};
use commonware_runtime::Spawner;
use consensus_pow::PowVerifier;
use serde_json::{json, Value};
use state::schema::tag;
use state::social::{self as ss, PostRef, Profile, SocialStats};

use crate::core::Core;
use crate::dispatch::{b58, RpcError};

fn corrupt(e: impl std::fmt::Display) -> RpcError {
    RpcError::server(e)
}

fn opt_key(k: &[u8; 32]) -> Value {
    if *k == [0; 32] { Value::Null } else { json!(b58(k)) }
}

pub fn stats_json(s: &SocialStats) -> Value {
    json!({
        "followers": s.followers, "following": s.following, "posts": s.posts, "replies": s.replies,
        "reactions_received": s.reactions_received, "reactions_given": s.reactions_given,
        "tips_received": s.tips_received.to_string(), "tips_sent": s.tips_sent.to_string(),
    })
}

pub fn profile_json(a: &Pubkey, p: &Profile) -> Value {
    json!({
        "account": b58(a),
        "handle": if p.handle.is_empty() { Value::Null } else { json!(String::from_utf8_lossy(&p.handle)) },
        "name": String::from_utf8_lossy(&p.name),
        "profile_uri": String::from_utf8_lossy(&p.profile_uri),
        "profile_hash": if p.profile_hash == [0; 32] { Value::Null } else { json!(hex::encode(p.profile_hash)) },
        "flags": p.flags,
        "is_agent": p.flags & agent_types::social::FLAG_AGENT != 0,
        "house": p.flags & agent_types::social::FLAG_HOUSE != 0,
        "accepts_hires": p.flags & agent_types::social::FLAG_HIRES != 0,
        "trades": p.flags & agent_types::social::FLAG_TRADES != 0,
        "operator": opt_key(&p.operator),
        "enc_key": opt_key(&p.enc_key),
        "dm_price": p.dm_price.to_string(), "dm_policy": p.dm_policy, "thread_free_blocks": p.thread_free_blocks,
        "created_height": p.created_height, "set_height": p.set_height, "index": p.index,
    })
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    async fn rec(&self, k: &state::Key) -> Result<Option<Vec<u8>>, RpcError> {
        self.chain.read_tip(k).await.map_err(corrupt)
    }

    pub(crate) async fn social_stats(&self, a: &Pubkey) -> Result<SocialStats, RpcError> {
        Ok(match self.rec(&ss::social_key(a)).await? {
            Some(b) => SocialStats::decode(&b).map_err(corrupt)?,
            None => SocialStats::default(),
        })
    }

    pub(crate) async fn profile_rec(&self, a: &Pubkey) -> Result<Option<Profile>, RpcError> {
        match self.rec(&ss::profile_key(a)).await? {
            Some(b) => Ok(Some(Profile::decode(&b).map_err(corrupt)?)),
            None => Ok(None),
        }
    }

    /// Profile + stats + the operator link's state; null when the account has neither.
    pub(crate) async fn get_profile(&self, a: &Pubkey) -> Result<Value, RpcError> {
        let p = self.profile_rec(a).await?;
        let s = self.social_stats(a).await?;
        if p.is_none() && s == SocialStats::default() {
            return Ok(Value::Null);
        }
        let mut v = match &p {
            Some(p) => profile_json(a, p),
            None => json!({"account": b58(a), "handle": null, "name": null}),
        };
        v["has_profile"] = json!(p.is_some());
        v["stats"] = stats_json(&s);
        if let Some(p) = &p {
            let linked = p.operator != [0; 32] && self.rec(&ss::operator_key(&p.operator, a)).await?.is_some();
            v["operator_linked"] = json!(linked);
        }
        Ok(v)
    }

    pub(crate) async fn get_handle(&self, h: &str) -> Result<Value, RpcError> {
        let h = h.trim().trim_start_matches('@').to_ascii_lowercase();
        let Some(b) = self.rec(&ss::handle_key(h.as_bytes())).await? else { return Ok(Value::Null) };
        let a = ss::decode_key32(tag::HANDLE, &b).map_err(corrupt)?;
        self.get_profile(&a).await
    }

    pub(crate) async fn list_profiles(&self, offset: u64, limit: u64) -> Result<Value, RpcError> {
        let n = match self.rec(&ss::profile_count_key()).await? {
            Some(b) => ss::decode_u64(tag::PROFILE_COUNT, &b).map_err(corrupt)?,
            None => 0,
        };
        let mut out = Vec::new();
        for i in offset..n.min(offset.saturating_add(limit)) {
            let Some(b) = self.rec(&ss::profile_index_key(i)).await? else { continue };
            let a = ss::decode_key32(tag::PROFILE_INDEX, &b).map_err(corrupt)?;
            out.push(self.get_profile(&a).await?);
        }
        Ok(json!({"total": n, "offset": offset, "profiles": out}))
    }

    /// Every profile's blob id (blob retention, SPEC 4.6).
    pub async fn profile_blobs(&self) -> Result<Vec<blobs::BlobId>, RpcError> {
        let n = match self.rec(&ss::profile_count_key()).await? {
            Some(b) => ss::decode_u64(tag::PROFILE_COUNT, &b).map_err(corrupt)?,
            None => 0,
        };
        let mut out = Vec::new();
        for i in 0..n {
            let Some(b) = self.rec(&ss::profile_index_key(i)).await? else { continue };
            let a = ss::decode_key32(tag::PROFILE_INDEX, &b).map_err(corrupt)?;
            if let Some(p) = self.profile_rec(&a).await? {
                if let Some(id) = blobs::parse_uri(&p.profile_uri) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// The PostRef plus the post's tx (body, block) when the block is still held.
    pub(crate) async fn get_post(&self, id: &Hash) -> Result<Value, RpcError> {
        let Some(b) = self.rec(&ss::post_key(id)).await? else { return Ok(Value::Null) };
        let r = PostRef::decode(&b).map_err(corrupt)?;
        let kind_name = ["post", "reply", "repost", "quote"].get(r.kind as usize).copied().unwrap_or("unknown");
        let mut v = json!({
            "post_id": hex::encode(id), "author": b58(&r.author), "height": r.height, "kind": r.kind,
            "kind_name": kind_name,
            "parent": if r.parent == [0; 32] { Value::Null } else { json!(hex::encode(r.parent)) },
            "body": null, "tx_hash": null, "block_hash": null,
        });
        if let Some(bh) = self.chain.canonical_hash(r.height) {
            if let Some(blk) = self.chain.get_block(&bh).await.map_err(corrupt)? {
                for (i, t) in blk.txs.iter().enumerate() {
                    if t.from != r.author {
                        continue;
                    }
                    if let TxBody::Post(p) = &t.body {
                        if post_id(&t.from, t.nonce) == *id {
                            v["body"] = json!(String::from_utf8_lossy(&p.body));
                            v["thought_ref"] = json!(String::from_utf8_lossy(&p.thought_ref));
                            v["blob_uri"] = json!(String::from_utf8_lossy(&p.blob_uri));
                            v["tx_hash"] = json!(hex::encode(t.hash()));
                            v["block_hash"] = json!(hex::encode(bh));
                            v["index"] = json!(i);
                            v["timestamp_ms"] = json!(blk.header.timestamp_ms);
                            break;
                        }
                    }
                }
            }
        }
        Ok(v)
    }

    pub(crate) async fn is_following(&self, follower: &Pubkey, target: &Pubkey) -> Result<Value, RpcError> {
        let h = match self.rec(&ss::follow_key(follower, target)).await? {
            Some(b) => Some(ss::decode_u64(tag::FOLLOW, &b).map_err(corrupt)?),
            None => None,
        };
        Ok(json!({"follower": b58(follower), "target": b58(target), "following": h.is_some(), "since_height": h}))
    }
}
