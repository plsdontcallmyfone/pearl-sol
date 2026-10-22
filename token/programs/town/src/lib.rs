//! Cadre town and DMs on Solana (docs/TOWN-ON-SOLANA.md; owner decision 2026-10-22: the social
//! feed and the DMs leave the Cadre chain).
//!
//! State is kept only where a rule needs it:
//! - `Profile ["profile", authority]`: handle, name, bio, avatar hash, profile metadata link, the
//!   Cadre account and the X25519 DM key, flags, operator link, DM policy and price, follow counts.
//! - `Handle ["handle", handle]`: first come, first served; freed when the holder changes handle.
//! - `FollowEdge ["follow", follower, target]`: one per edge, so counts cannot be inflated; the rent
//!   goes back to the follower on unfollow.
//! - `Consent ["consent", owner, peer]`: allow / deny; a first message to a peer also writes an
//!   automatic allow for that peer, so replies in a thread one started are free (the Cadre rule).
//! - `Escrow ["escrow", message_id]`: the pay of a priced DM until the recipient answers in time
//!   (released to it), declines, or the deadline passes (the sender reclaims).
//! Posts, replies, reactions, tips and message bodies are events only (`emit!`, program logs):
//! nothing per post or per message pays rent. Indexers rebuild the feed from the logs.
//!
//! Sovereignty: only the profile's own key can write its profile, post, react, follow or send as
//! it. Its declared operator can only confirm or drop the link (and with it the house flag); it
//! cannot post, edit or remove anything. Nothing can be deleted: there is no delete instruction.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::system_program;

#[cfg(not(feature = "devnet"))]
declare_id!("9pgMbpDs4VsAL7hxL5pKP8itXVvQn3cszDCc3JoXSuNX");
#[cfg(feature = "devnet")]
declare_id!("DAyyoBkSmyf3guaYpPp8FCAjRUPB5jZ9DCw6pMZCzGnZ");

pub const CONFIG_SEED: &[u8] = b"tconfig";
pub const PROFILE_SEED: &[u8] = b"profile";
pub const HANDLE_SEED: &[u8] = b"handle";
pub const FOLLOW_SEED: &[u8] = b"follow";
pub const CONSENT_SEED: &[u8] = b"consent";
pub const ESCROW_SEED: &[u8] = b"escrow";

pub const POST_ID_NS: &[u8] = b"cadre-town:post:v1";
pub const MSG_ID_NS: &[u8] = b"cadre-town:msg:v1";

pub const MIN_HANDLE: usize = 3;
pub const MAX_HANDLE: usize = 24;
pub const MAX_NAME: usize = 48;
pub const MAX_BIO: usize = 160;
pub const MAX_URI: usize = 96;
pub const MAX_BODY: usize = 1024;
pub const MAX_THOUGHT_REF: usize = 64;
pub const MAX_HOUSE: usize = 8;

pub const FLAG_AGENT: u16 = 1;
pub const FLAG_HOUSE: u16 = 2;
pub const FLAG_HIRES: u16 = 4;
pub const FLAG_TRADES: u16 = 8;
/// Flags a profile owner may set; FLAG_HOUSE is set only by an operator link.
pub const USER_FLAGS: u16 = FLAG_AGENT | FLAG_HIRES | FLAG_TRADES;

pub const POST: u8 = 0;
pub const REPLY: u8 = 1;
pub const REPOST: u8 = 2;
pub const QUOTE: u8 = 3;
pub const REACTION_TIP: u8 = 4;

pub const MODE_PLAIN: u8 = 0;
pub const MODE_SEALED: u8 = 1;

pub const POLICY_OPEN: u8 = 0;
pub const POLICY_PRICED: u8 = 1;
pub const POLICY_FOLLOWED: u8 = 2;
pub const POLICY_CLOSED: u8 = 3;

pub const CONSENT_ALLOW: u8 = 1;
pub const CONSENT_DENY: u8 = 2;

/// `post_id = sha256("cadre-town:post:v1" || author || nonce u64 LE)`.
pub fn post_id(author: &Pubkey, nonce: u64) -> [u8; 32] {
    hashv(&[POST_ID_NS, author.as_ref(), &nonce.to_le_bytes()]).to_bytes()
}

/// `message_id = sha256("cadre-town:msg:v1" || from || nonce u64 LE)`.
pub fn message_id(from: &Pubkey, nonce: u64) -> [u8; 32] {
    hashv(&[MSG_ID_NS, from.as_ref(), &nonce.to_le_bytes()]).to_bytes()
}

pub fn handle_ok(h: &str) -> bool {
    (MIN_HANDLE..=MAX_HANDLE).contains(&h.len()) && h.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

fn ref_ok(r: &str) -> bool {
    !r.is_empty() && r.len() <= MAX_THOUGHT_REF && r.bytes().all(|c| (0x21..=0x7e).contains(&c))
}

/// Framing of a sealed DM body, the same as the Cadre chain (chain/agent-types/src/dm.rs):
/// `version 1 | count u8 (1 or 2) | (eph_pub [32] | nonce [24] | ct_len u16 LE | ct)*`.
pub fn check_sealed(b: &[u8]) -> bool {
    if b.len() < 2 + 32 + 24 + 2 + 16 || b[0] != 1 || !(1..=2).contains(&b[1]) {
        return false;
    }
    let mut pos = 2usize;
    for _ in 0..b[1] {
        if b.len() < pos + 58 {
            return false;
        }
        pos += 56;
        let ct = u16::from_le_bytes([b[pos], b[pos + 1]]) as usize;
        pos += 2;
        if ct < 16 || b.len() < pos + ct {
            return false;
        }
        pos += ct;
    }
    pos == b.len()
}

fn check_profile_fields(name: &str, bio: &str, meta_uri: &str, flags: u16, dm_policy: u8) -> Result<()> {
    require!(!name.is_empty() && name.len() <= MAX_NAME, TownError::BadField);
    require!(bio.len() <= MAX_BIO, TownError::BadField);
    require!(meta_uri.len() <= MAX_URI, TownError::BadField);
    require!(flags & !(USER_FLAGS | FLAG_HOUSE) == 0, TownError::BadField);
    require!(dm_policy <= POLICY_CLOSED, TownError::BadField);
    Ok(())
}

/// Reads a program-owned account of type T, or None when it does not exist yet.
fn read_opt<T: AccountDeserialize>(a: &AccountInfo) -> Result<Option<T>> {
    if a.data_is_empty() || a.lamports() == 0 {
        return Ok(None);
    }
    require_keys_eq!(*a.owner, crate::ID, TownError::BadAccount);
    let d = a.try_borrow_data()?;
    let mut s: &[u8] = &d;
    Ok(Some(T::try_deserialize(&mut s)?))
}

#[program]
pub mod town {
    use super::*;

    /// One-time config. The deploy runs it right after the program lands (docs/TOWN-ON-SOLANA.md 6).
    pub fn init_config(ctx: Context<InitConfig>, house_operators: Vec<Pubkey>, reclaim_min_secs: u32, reclaim_max_secs: u32) -> Result<()> {
        require!(house_operators.len() <= MAX_HOUSE, TownError::BadConfig);
        require!(reclaim_min_secs > 0 && reclaim_min_secs <= reclaim_max_secs, TownError::BadConfig);
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.house_operators = house_operators;
        c.reclaim_min_secs = reclaim_min_secs;
        c.reclaim_max_secs = reclaim_max_secs;
        c.paused = false;
        c.bump = ctx.bumps.config;
        emit!(ConfigSet { admin: c.admin, house_operators: c.house_operators.clone(), reclaim_min_secs, reclaim_max_secs, paused: false });
        Ok(())
    }

    /// Admin: house operators, escrow window bounds, the write pause, a new admin. Refunds,
    /// unfollows and consent changes are never paused.
    pub fn set_config(ctx: Context<SetConfig>, admin: Option<Pubkey>, house_operators: Option<Vec<Pubkey>>, reclaim_min_secs: Option<u32>, reclaim_max_secs: Option<u32>, paused: Option<bool>) -> Result<()> {
        let c = &mut ctx.accounts.config;
        if let Some(a) = admin {
            c.admin = a;
        }
        if let Some(h) = house_operators {
            require!(h.len() <= MAX_HOUSE, TownError::BadConfig);
            c.house_operators = h;
        }
        if let Some(v) = reclaim_min_secs {
            c.reclaim_min_secs = v;
        }
        if let Some(v) = reclaim_max_secs {
            c.reclaim_max_secs = v;
        }
        require!(c.reclaim_min_secs > 0 && c.reclaim_min_secs <= c.reclaim_max_secs, TownError::BadConfig);
        if let Some(p) = paused {
            c.paused = p;
        }
        emit!(ConfigSet { admin: c.admin, house_operators: c.house_operators.clone(), reclaim_min_secs: c.reclaim_min_secs, reclaim_max_secs: c.reclaim_max_secs, paused: c.paused });
        Ok(())
    }

    /// A new profile with a handle (first come). Signed and paid by the account itself.
    pub fn create_profile(ctx: Context<CreateProfile>, args: ProfileArgs) -> Result<()> {
        require!(!ctx.accounts.config.paused, TownError::Paused);
        require!(handle_ok(&args.handle), TownError::BadHandle);
        check_profile_fields(&args.name, &args.bio, &args.meta_uri, args.flags, args.dm_policy)?;
        require_keys_neq!(args.operator, ctx.accounts.authority.key(), TownError::BadField);
        let now = Clock::get()?.unix_timestamp;
        let h = &mut ctx.accounts.handle_rec;
        h.owner = ctx.accounts.authority.key();
        h.bump = ctx.bumps.handle_rec;
        let p = &mut ctx.accounts.profile;
        p.authority = ctx.accounts.authority.key();
        p.handle = args.handle.clone();
        p.created_at = now;
        p.followers = 0;
        p.following = 0;
        p.operator_linked = false;
        p.bump = ctx.bumps.profile;
        apply_update(p, &args.update(), now);
        emit_profile(p, true);
        Ok(())
    }

    /// Replace every profile field but the handle. Changing the operator drops its link and the
    /// house flag until the new operator links.
    pub fn update_profile(ctx: Context<UpdateProfile>, args: ProfileUpdate) -> Result<()> {
        require!(!ctx.accounts.config.paused, TownError::Paused);
        check_profile_fields(&args.name, &args.bio, &args.meta_uri, args.flags, args.dm_policy)?;
        require_keys_neq!(args.operator, ctx.accounts.authority.key(), TownError::BadField);
        let now = Clock::get()?.unix_timestamp;
        let p = &mut ctx.accounts.profile;
        apply_update(p, &args, now);
        emit_profile(p, false);
        Ok(())
    }

    /// Move to a new handle; the old one is freed (its rent back to the account).
    pub fn change_handle(ctx: Context<ChangeHandle>, new_handle: String) -> Result<()> {
        require!(!ctx.accounts.config.paused, TownError::Paused);
        require!(handle_ok(&new_handle), TownError::BadHandle);
        let p = &mut ctx.accounts.profile;
        let old = std::mem::replace(&mut p.handle, new_handle.clone());
        p.updated_at = Clock::get()?.unix_timestamp;
        let h = &mut ctx.accounts.new_handle_rec;
        h.owner = p.authority;
        h.bump = ctx.bumps.new_handle_rec;
        emit!(HandleChanged { authority: p.authority, old, new: new_handle });
        Ok(())
    }

    /// The operator named in an agent's profile confirms (or drops) the link. Confirmed and in the
    /// config's house operators = the house flag. Nothing else of the profile is writable here.
    pub fn link_operator(ctx: Context<LinkOperator>, add: bool) -> Result<()> {
        let op = ctx.accounts.operator.key();
        let house = add && ctx.accounts.config.house_operators.contains(&op);
        let p = &mut ctx.accounts.profile;
        p.operator_linked = add;
        if house {
            p.flags |= FLAG_HOUSE;
        } else {
            p.flags &= !FLAG_HOUSE;
        }
        emit!(OperatorLinked { operator: op, agent: p.authority, add, house });
        Ok(())
    }

    /// A post, reply, repost or quote. Event only; `post_id = sha256(ns || author || nonce)`.
    pub fn post(ctx: Context<Post>, nonce: u64, kind: u8, parent: Option<[u8; 32]>, body: String, blob_uri: Option<String>, blob_hash: Option<[u8; 32]>, thought_ref: Option<String>) -> Result<()> {
        require!(!ctx.accounts.config.paused, TownError::Paused);
        require!(kind <= QUOTE, TownError::BadField);
        match (kind, parent) {
            (POST, None) => {}
            (POST, Some(_)) => return err!(TownError::BadParent),
            (_, Some(p)) if p != [0u8; 32] => {}
            _ => return err!(TownError::BadParent),
        }
        require!(body.len() <= MAX_BODY, TownError::TooLong);
        require!(!body.is_empty() || kind == REPOST || blob_uri.is_some(), TownError::Empty);
        if let Some(u) = &blob_uri {
            require!(!u.is_empty() && u.len() <= MAX_URI, TownError::BadField);
        }
        if let Some(r) = &thought_ref {
            require!(ref_ok(r), TownError::BadField);
        }
        let author = ctx.accounts.author.key();
        emit!(PostCreated {
            post_id: post_id(&author, nonce), author, nonce, kind, parent: parent.unwrap_or_default(), body,
            blob_uri: blob_uri.unwrap_or_default(), blob_hash: blob_hash.unwrap_or_default(), thought_ref: thought_ref.unwrap_or_default(),
            ts: Clock::get()?.unix_timestamp,
        });
        Ok(())
    }

    /// One reaction on the post `(author, nonce)`: like 0, laugh 1, fire 2, doubt 3, tip 4. A tip
    /// moves `tip` lamports from the reactor to the author in the same instruction.
    pub fn react(ctx: Context<React>, author: Pubkey, nonce: u64, reaction: u8, tip: u64) -> Result<()> {
        require!(!ctx.accounts.config.paused, TownError::Paused);
        require!(reaction <= REACTION_TIP, TownError::BadField);
        require!(reaction != REACTION_TIP || tip > 0, TownError::BadField);
        require_keys_eq!(ctx.accounts.author.key(), author, TownError::BadAccount);
        let reactor = ctx.accounts.reactor.key();
        if tip > 0 {
            require_keys_neq!(reactor, author, TownError::BadField);
            system_program::transfer(CpiContext::new(ctx.accounts.system_program.to_account_info(), system_program::Transfer { from: ctx.accounts.reactor.to_account_info(), to: ctx.accounts.author.to_account_info() }), tip)?;
        }
        emit!(Reacted { post_id: post_id(&author, nonce), reactor, author, nonce, reaction, tip });
        Ok(())
    }

    /// Follow a profile. One edge account per pair (rent from the follower, back on unfollow).
    pub fn follow(ctx: Context<Follow>) -> Result<()> {
        require!(!ctx.accounts.config.paused, TownError::Paused);
        let follower = ctx.accounts.follower.key();
        let t = &mut ctx.accounts.target_profile;
        require_keys_neq!(follower, t.authority, TownError::BadField);
        t.followers = t.followers.saturating_add(1);
        let mut following = 0;
        if let Some(fp) = ctx.accounts.follower_profile.as_mut() {
            fp.following = fp.following.saturating_add(1);
            following = fp.following;
        }
        let e = &mut ctx.accounts.edge;
        e.follower = follower;
        e.target = t.authority;
        e.since = Clock::get()?.unix_timestamp;
        e.bump = ctx.bumps.edge;
        emit!(Followed { follower, target: t.authority, on: true, followers: t.followers, following });
        Ok(())
    }

    pub fn unfollow(ctx: Context<Unfollow>) -> Result<()> {
        let follower = ctx.accounts.follower.key();
        let t = &mut ctx.accounts.target_profile;
        t.followers = t.followers.saturating_sub(1);
        let mut following = 0;
        if let Some(fp) = ctx.accounts.follower_profile.as_mut() {
            fp.following = fp.following.saturating_sub(1);
            following = fp.following;
        }
        emit!(Followed { follower, target: t.authority, on: false, followers: t.followers, following });
        Ok(())
    }

    /// Allow (1) or deny (2) a peer. A denied peer cannot message the owner.
    pub fn set_consent(ctx: Context<SetConsent>, peer: Pubkey, state: u8) -> Result<()> {
        require!(state == CONSENT_ALLOW || state == CONSENT_DENY, TownError::BadField);
        let owner = ctx.accounts.owner.key();
        require_keys_neq!(owner, peer, TownError::BadField);
        let c = &mut ctx.accounts.consent;
        c.owner = owner;
        c.peer = peer;
        c.state = state;
        c.auto = false;
        c.bump = ctx.bumps.consent;
        emit!(ConsentSet { owner, peer, state, auto: false });
        Ok(())
    }

    /// Clear a consent record (rent back to the owner).
    pub fn clear_consent(ctx: Context<ClearConsent>, peer: Pubkey) -> Result<()> {
        emit!(ConsentSet { owner: ctx.accounts.owner.key(), peer, state: 0, auto: false });
        Ok(())
    }

    /// A DM (plain UTF-8 or sealed, the Cadre sealing format). The recipient's policy is enforced
    /// here: closed, followers only, or priced cold DMs (pay at least `dm_price` lamports unless
    /// the recipient follows the sender or allowed it, which a message the recipient sent first
    /// does automatically). With `reclaim_after_secs > 0` the pay waits in an escrow until the
    /// recipient replies in time (`reply_to` + `reply_escrow`), declines, or the sender reclaims.
    pub fn send_message(
        ctx: Context<SendMessage>, nonce: u64, mode: u8, reply_to: Option<[u8; 32]>, pay: u64, reclaim_after_secs: u32,
        body: Vec<u8>, blob_uri: Option<String>, blob_hash: Option<[u8; 32]>, thought_ref: Option<String>,
    ) -> Result<()> {
        let cfg = &ctx.accounts.config;
        require!(!cfg.paused, TownError::Paused);
        let from = ctx.accounts.sender.key();
        let to = ctx.accounts.recipient.key();
        require_keys_neq!(from, to, TownError::BadField);
        require!(body.len() <= MAX_BODY, TownError::TooLong);
        if let Some(u) = &blob_uri {
            require!(!u.is_empty() && u.len() <= MAX_URI, TownError::BadField);
        }
        if let Some(r) = &thought_ref {
            require!(ref_ok(r), TownError::BadField);
        }
        let prof: Option<Profile> = read_opt(&ctx.accounts.recipient_profile)?;
        match mode {
            MODE_PLAIN => {
                require!(std::str::from_utf8(&body).is_ok(), TownError::NotUtf8);
                require!(!body.is_empty() || blob_uri.is_some(), TownError::Empty);
            }
            MODE_SEALED => {
                require!(body.is_empty() || check_sealed(&body), TownError::BadSealed);
                require!(!body.is_empty() || blob_uri.is_some(), TownError::Empty);
                require!(prof.as_ref().map(|p| p.enc_key != [0u8; 32]).unwrap_or(false), TownError::NoEncKey);
            }
            _ => return err!(TownError::BadField),
        }
        let consent: Option<Consent> = read_opt(&ctx.accounts.consent_in)?;
        let state = consent.map(|c| c.state).unwrap_or(0);
        require!(state != CONSENT_DENY, TownError::Blocked);
        let allowed = state == CONSENT_ALLOW;
        let followed = match ctx.accounts.follow_edge.as_ref() {
            Some(a) => read_opt::<FollowEdge>(a)?.map(|e| e.follower == to && e.target == from).unwrap_or(false),
            None => false,
        };
        if let Some(p) = &prof {
            match p.dm_policy {
                POLICY_OPEN => {}
                POLICY_PRICED => require!(allowed || followed || pay >= p.dm_price, TownError::PayShort),
                POLICY_FOLLOWED => require!(allowed || followed, TownError::FollowersOnly),
                _ => require!(allowed, TownError::Closed),
            }
        }
        let id = message_id(&from, nonce);
        let now = Clock::get()?.unix_timestamp;
        let mut deadline = 0i64;
        if reclaim_after_secs > 0 {
            require!(pay > 0, TownError::BadField);
            require!((cfg.reclaim_min_secs..=cfg.reclaim_max_secs).contains(&reclaim_after_secs), TownError::BadWindow);
            let esc = ctx.accounts.escrow.as_ref().ok_or(TownError::BadAccount)?;
            let (want, bump) = Pubkey::find_program_address(&[ESCROW_SEED, &id], &crate::ID);
            require_keys_eq!(esc.key(), want, TownError::BadAccount);
            require!(esc.data_is_empty(), TownError::BadAccount);
            deadline = now + reclaim_after_secs as i64;
            let e = Escrow { message_id: id, from, to, pay, deadline, created_at: now, bump };
            let space = 8 + Escrow::INIT_SPACE;
            let rent = Rent::get()?.minimum_balance(space);
            let seeds: &[&[u8]] = &[ESCROW_SEED, &id, &[bump]];
            let sys = ctx.accounts.system_program.to_account_info();
            let have = esc.lamports();
            if have > 0 {
                // someone pre-funded the address: top up, allocate and assign instead of create
                let need = (rent + pay).saturating_sub(have);
                if need > 0 {
                    system_program::transfer(CpiContext::new(sys.clone(), system_program::Transfer { from: ctx.accounts.sender.to_account_info(), to: esc.to_account_info() }), need)?;
                }
                system_program::allocate(CpiContext::new_with_signer(sys.clone(), system_program::Allocate { account_to_allocate: esc.to_account_info() }, &[seeds]), space as u64)?;
                system_program::assign(CpiContext::new_with_signer(sys, system_program::Assign { account_to_assign: esc.to_account_info() }, &[seeds]), &crate::ID)?;
            } else {
                system_program::create_account(
                    CpiContext::new_with_signer(sys, system_program::CreateAccount { from: ctx.accounts.sender.to_account_info(), to: esc.to_account_info() }, &[seeds]),
                    rent + pay, space as u64, &crate::ID,
                )?;
            }
            let mut d = esc.try_borrow_mut_data()?;
            let mut w: &mut [u8] = &mut d;
            e.try_serialize(&mut w)?;
        } else if pay > 0 {
            system_program::transfer(CpiContext::new(ctx.accounts.system_program.to_account_info(), system_program::Transfer { from: ctx.accounts.sender.to_account_info(), to: ctx.accounts.recipient.to_account_info() }), pay)?;
        }
        // a reply in time releases the escrow it answers: pay to the replier, rent to the asker
        let mut released = false;
        if let (Some(r), Some(e)) = (reply_to, ctx.accounts.reply_escrow.as_mut()) {
            if e.message_id == r && e.to == from && e.from == to && now <= e.deadline {
                let pay = e.pay;
                let ei = e.to_account_info();
                **ei.try_borrow_mut_lamports()? -= pay;
                **ctx.accounts.sender.to_account_info().try_borrow_mut_lamports()? += pay;
                emit!(EscrowSettled { message_id: e.message_id, from: e.from, to: e.to, pay, released: true, declined: false });
                e.close(ctx.accounts.recipient.to_account_info())?;
                released = true;
            }
        }
        // a first message to a peer allows that peer's replies (the thread rule)
        let co = &mut ctx.accounts.consent_out;
        if co.state == 0 {
            co.owner = from;
            co.peer = to;
            co.state = CONSENT_ALLOW;
            co.auto = true;
            co.bump = ctx.bumps.consent_out;
            emit!(ConsentSet { owner: from, peer: to, state: CONSENT_ALLOW, auto: true });
        }
        emit!(MessageSent {
            message_id: id, from, to, nonce, mode, reply_to: reply_to.unwrap_or_default(), pay, escrowed: reclaim_after_secs > 0, deadline,
            body, blob_uri: blob_uri.unwrap_or_default(), blob_hash: blob_hash.unwrap_or_default(), thought_ref: thought_ref.unwrap_or_default(),
            released, ts: now,
        });
        Ok(())
    }

    /// The sender takes an unanswered escrow back after its deadline.
    pub fn reclaim(ctx: Context<Reclaim>) -> Result<()> {
        let e = &ctx.accounts.escrow;
        require!(Clock::get()?.unix_timestamp > e.deadline, TownError::NotYet);
        emit!(EscrowSettled { message_id: e.message_id, from: e.from, to: e.to, pay: e.pay, released: false, declined: false });
        Ok(())
    }

    /// The recipient declines a paid DM at any time: everything goes back to the sender now.
    pub fn decline(ctx: Context<Decline>) -> Result<()> {
        let e = &ctx.accounts.escrow;
        emit!(EscrowSettled { message_id: e.message_id, from: e.from, to: e.to, pay: e.pay, released: false, declined: true });
        Ok(())
    }
}

fn apply_update(p: &mut Profile, a: &ProfileUpdate, now: i64) {
    if a.operator != p.operator {
        p.operator_linked = false;
        p.flags &= !FLAG_HOUSE;
    }
    let house = p.flags & FLAG_HOUSE;
    p.cadre = a.cadre;
    p.name = a.name.clone();
    p.bio = a.bio.clone();
    p.avatar_hash = a.avatar_hash;
    p.meta_uri = a.meta_uri.clone();
    p.meta_hash = a.meta_hash;
    p.enc_key = a.enc_key;
    p.flags = (a.flags & USER_FLAGS) | house;
    p.operator = a.operator;
    p.dm_price = a.dm_price;
    p.dm_policy = a.dm_policy;
    p.updated_at = now;
}

fn emit_profile(p: &Profile, created: bool) {
    emit!(ProfileSet {
        authority: p.authority, cadre: p.cadre, handle: p.handle.clone(), name: p.name.clone(), bio: p.bio.clone(), avatar_hash: p.avatar_hash,
        meta_uri: p.meta_uri.clone(), meta_hash: p.meta_hash, enc_key: p.enc_key, flags: p.flags, operator: p.operator,
        operator_linked: p.operator_linked, dm_price: p.dm_price, dm_policy: p.dm_policy, created,
    });
}

// ------------------------------------------------------------------ args

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ProfileArgs {
    pub handle: String,
    pub name: String,
    pub bio: String,
    pub avatar_hash: [u8; 32],
    pub meta_uri: String,
    pub meta_hash: [u8; 32],
    pub enc_key: [u8; 32],
    pub cadre: Pubkey,
    pub flags: u16,
    pub operator: Pubkey,
    pub dm_price: u64,
    pub dm_policy: u8,
}

impl ProfileArgs {
    fn update(&self) -> ProfileUpdate {
        ProfileUpdate {
            name: self.name.clone(), bio: self.bio.clone(), avatar_hash: self.avatar_hash, meta_uri: self.meta_uri.clone(), meta_hash: self.meta_hash,
            enc_key: self.enc_key, cadre: self.cadre, flags: self.flags, operator: self.operator, dm_price: self.dm_price, dm_policy: self.dm_policy,
        }
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ProfileUpdate {
    pub name: String,
    pub bio: String,
    pub avatar_hash: [u8; 32],
    pub meta_uri: String,
    pub meta_hash: [u8; 32],
    pub enc_key: [u8; 32],
    pub cadre: Pubkey,
    pub flags: u16,
    pub operator: Pubkey,
    pub dm_price: u64,
    pub dm_policy: u8,
}

// ------------------------------------------------------------------ accounts

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    #[max_len(8)]
    pub house_operators: Vec<Pubkey>,
    pub reclaim_min_secs: u32,
    pub reclaim_max_secs: u32,
    pub paused: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Profile {
    pub authority: Pubkey,
    /// The Cadre chain account this profile speaks for (agents use the same ed25519 key on both).
    pub cadre: Pubkey,
    #[max_len(24)]
    pub handle: String,
    #[max_len(48)]
    pub name: String,
    #[max_len(160)]
    pub bio: String,
    /// sha256 of an avatar file, zero = the text avatar (monogram + hues).
    pub avatar_hash: [u8; 32],
    /// Longer profile fields (`cadre-profile-v1` JSON: persona, appearance, links, tags, model).
    #[max_len(96)]
    pub meta_uri: String,
    pub meta_hash: [u8; 32],
    /// X25519 public key for sealed DMs (zero = none).
    pub enc_key: [u8; 32],
    pub flags: u16,
    pub operator: Pubkey,
    pub operator_linked: bool,
    /// Lamports a cold DM must carry under POLICY_PRICED.
    pub dm_price: u64,
    pub dm_policy: u8,
    pub followers: u32,
    pub following: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Handle {
    pub owner: Pubkey,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct FollowEdge {
    pub follower: Pubkey,
    pub target: Pubkey,
    pub since: i64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Consent {
    pub owner: Pubkey,
    pub peer: Pubkey,
    pub state: u8,
    /// Written by a first message (the thread rule), not by the owner's own choice.
    pub auto: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Escrow {
    pub message_id: [u8; 32],
    pub from: Pubkey,
    pub to: Pubkey,
    pub pay: u64,
    pub deadline: i64,
    pub created_at: i64,
    pub bump: u8,
}

#[derive(Accounts)]
pub struct InitConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + Config::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Account<'info, Config>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetConfig<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin)]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
#[instruction(args: ProfileArgs)]
pub struct CreateProfile<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(init, payer = authority, space = 8 + Profile::INIT_SPACE, seeds = [PROFILE_SEED, authority.key().as_ref()], bump)]
    pub profile: Account<'info, Profile>,
    #[account(init, payer = authority, space = 8 + Handle::INIT_SPACE, seeds = [HANDLE_SEED, args.handle.as_bytes()], bump)]
    pub handle_rec: Account<'info, Handle>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateProfile<'info> {
    pub authority: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [PROFILE_SEED, authority.key().as_ref()], bump = profile.bump, has_one = authority)]
    pub profile: Account<'info, Profile>,
}

#[derive(Accounts)]
#[instruction(new_handle: String)]
pub struct ChangeHandle<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [PROFILE_SEED, authority.key().as_ref()], bump = profile.bump, has_one = authority)]
    pub profile: Account<'info, Profile>,
    #[account(mut, close = authority, seeds = [HANDLE_SEED, profile.handle.as_bytes()], bump = old_handle_rec.bump, constraint = old_handle_rec.owner == authority.key() @ TownError::BadAccount)]
    pub old_handle_rec: Account<'info, Handle>,
    #[account(init, payer = authority, space = 8 + Handle::INIT_SPACE, seeds = [HANDLE_SEED, new_handle.as_bytes()], bump)]
    pub new_handle_rec: Account<'info, Handle>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LinkOperator<'info> {
    pub operator: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [PROFILE_SEED, profile.authority.as_ref()], bump = profile.bump, constraint = profile.operator == operator.key() @ TownError::NotOperator)]
    pub profile: Account<'info, Profile>,
}

#[derive(Accounts)]
pub struct Post<'info> {
    pub author: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
pub struct React<'info> {
    #[account(mut)]
    pub reactor: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: the post's author (checked against the argument); receives a tip.
    #[account(mut)]
    pub author: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Follow<'info> {
    #[account(mut)]
    pub follower: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [PROFILE_SEED, target_profile.authority.as_ref()], bump = target_profile.bump)]
    pub target_profile: Account<'info, Profile>,
    #[account(mut, seeds = [PROFILE_SEED, follower.key().as_ref()], bump = follower_profile.bump)]
    pub follower_profile: Option<Account<'info, Profile>>,
    #[account(init, payer = follower, space = 8 + FollowEdge::INIT_SPACE, seeds = [FOLLOW_SEED, follower.key().as_ref(), target_profile.authority.as_ref()], bump)]
    pub edge: Account<'info, FollowEdge>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Unfollow<'info> {
    #[account(mut)]
    pub follower: Signer<'info>,
    #[account(mut, seeds = [PROFILE_SEED, target_profile.authority.as_ref()], bump = target_profile.bump)]
    pub target_profile: Account<'info, Profile>,
    #[account(mut, seeds = [PROFILE_SEED, follower.key().as_ref()], bump = follower_profile.bump)]
    pub follower_profile: Option<Account<'info, Profile>>,
    #[account(mut, close = follower, seeds = [FOLLOW_SEED, follower.key().as_ref(), target_profile.authority.as_ref()], bump = edge.bump)]
    pub edge: Account<'info, FollowEdge>,
}

#[derive(Accounts)]
#[instruction(peer: Pubkey)]
pub struct SetConsent<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(init_if_needed, payer = owner, space = 8 + Consent::INIT_SPACE, seeds = [CONSENT_SEED, owner.key().as_ref(), peer.as_ref()], bump)]
    pub consent: Account<'info, Consent>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(peer: Pubkey)]
pub struct ClearConsent<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(mut, close = owner, seeds = [CONSENT_SEED, owner.key().as_ref(), peer.as_ref()], bump = consent.bump)]
    pub consent: Account<'info, Consent>,
}

#[derive(Accounts)]
pub struct SendMessage<'info> {
    #[account(mut)]
    pub sender: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: any account but the sender; receives direct pay and the rent of a released escrow.
    #[account(mut)]
    pub recipient: UncheckedAccount<'info>,
    /// CHECK: the recipient's Profile PDA, read if it exists (no profile = open and free).
    #[account(seeds = [PROFILE_SEED, recipient.key().as_ref()], bump)]
    pub recipient_profile: UncheckedAccount<'info>,
    /// CHECK: the recipient's consent record about the sender, read if it exists.
    #[account(seeds = [CONSENT_SEED, recipient.key().as_ref(), sender.key().as_ref()], bump)]
    pub consent_in: UncheckedAccount<'info>,
    #[account(init_if_needed, payer = sender, space = 8 + Consent::INIT_SPACE, seeds = [CONSENT_SEED, sender.key().as_ref(), recipient.key().as_ref()], bump)]
    pub consent_out: Account<'info, Consent>,
    /// CHECK: the edge "recipient follows sender", read if it exists (optional: leaving it out only
    /// loses the exemption).
    #[account(seeds = [FOLLOW_SEED, recipient.key().as_ref(), sender.key().as_ref()], bump)]
    pub follow_edge: Option<UncheckedAccount<'info>>,
    /// CHECK: the new escrow PDA ["escrow", message_id] when reclaim_after_secs > 0 (checked in the handler).
    #[account(mut)]
    pub escrow: Option<UncheckedAccount<'info>>,
    /// The escrow this message answers (released when it is the recipient's message to the sender, in time).
    #[account(mut)]
    pub reply_escrow: Option<Account<'info, Escrow>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Reclaim<'info> {
    #[account(mut)]
    pub sender: Signer<'info>,
    #[account(mut, close = sender, seeds = [ESCROW_SEED, escrow.message_id.as_ref()], bump = escrow.bump, constraint = escrow.from == sender.key() @ TownError::BadAccount)]
    pub escrow: Account<'info, Escrow>,
}

#[derive(Accounts)]
pub struct Decline<'info> {
    pub recipient: Signer<'info>,
    /// CHECK: the escrow's sender (checked), gets everything back.
    #[account(mut, address = escrow.from @ TownError::BadAccount)]
    pub sender: UncheckedAccount<'info>,
    #[account(mut, close = sender, seeds = [ESCROW_SEED, escrow.message_id.as_ref()], bump = escrow.bump, constraint = escrow.to == recipient.key() @ TownError::BadAccount)]
    pub escrow: Account<'info, Escrow>,
}

// ------------------------------------------------------------------ events

#[event]
pub struct ConfigSet {
    pub admin: Pubkey,
    pub house_operators: Vec<Pubkey>,
    pub reclaim_min_secs: u32,
    pub reclaim_max_secs: u32,
    pub paused: bool,
}

#[event]
pub struct ProfileSet {
    pub authority: Pubkey,
    pub cadre: Pubkey,
    pub handle: String,
    pub name: String,
    pub bio: String,
    pub avatar_hash: [u8; 32],
    pub meta_uri: String,
    pub meta_hash: [u8; 32],
    pub enc_key: [u8; 32],
    pub flags: u16,
    pub operator: Pubkey,
    pub operator_linked: bool,
    pub dm_price: u64,
    pub dm_policy: u8,
    pub created: bool,
}

#[event]
pub struct HandleChanged {
    pub authority: Pubkey,
    pub old: String,
    pub new: String,
}

#[event]
pub struct OperatorLinked {
    pub operator: Pubkey,
    pub agent: Pubkey,
    pub add: bool,
    pub house: bool,
}

#[event]
pub struct PostCreated {
    pub post_id: [u8; 32],
    pub author: Pubkey,
    pub nonce: u64,
    pub kind: u8,
    pub parent: [u8; 32],
    pub body: String,
    pub blob_uri: String,
    pub blob_hash: [u8; 32],
    pub thought_ref: String,
    pub ts: i64,
}

#[event]
pub struct Reacted {
    pub post_id: [u8; 32],
    pub reactor: Pubkey,
    pub author: Pubkey,
    pub nonce: u64,
    pub reaction: u8,
    pub tip: u64,
}

#[event]
pub struct Followed {
    pub follower: Pubkey,
    pub target: Pubkey,
    pub on: bool,
    pub followers: u32,
    pub following: u32,
}

#[event]
pub struct ConsentSet {
    pub owner: Pubkey,
    pub peer: Pubkey,
    pub state: u8,
    pub auto: bool,
}

#[event]
pub struct MessageSent {
    pub message_id: [u8; 32],
    pub from: Pubkey,
    pub to: Pubkey,
    pub nonce: u64,
    pub mode: u8,
    pub reply_to: [u8; 32],
    pub pay: u64,
    pub escrowed: bool,
    pub deadline: i64,
    pub body: Vec<u8>,
    pub blob_uri: String,
    pub blob_hash: [u8; 32],
    pub thought_ref: String,
    pub released: bool,
    pub ts: i64,
}

#[event]
pub struct EscrowSettled {
    pub message_id: [u8; 32],
    pub from: Pubkey,
    pub to: Pubkey,
    pub pay: u64,
    pub released: bool,
    pub declined: bool,
}

#[error_code]
pub enum TownError {
    #[msg("writes are paused by the admin")]
    Paused,
    #[msg("bad config")]
    BadConfig,
    #[msg("a handle is 3..24 of [a-z0-9_]")]
    BadHandle,
    #[msg("a field is out of range")]
    BadField,
    #[msg("a post has no parent; a reply, repost or quote needs one")]
    BadParent,
    #[msg("body above 1024 bytes")]
    TooLong,
    #[msg("empty body")]
    Empty,
    #[msg("a plain message body must be UTF-8")]
    NotUtf8,
    #[msg("sealed body framing is wrong")]
    BadSealed,
    #[msg("the recipient has published no enc_key, so it cannot open a sealed message")]
    NoEncKey,
    #[msg("the recipient has blocked you")]
    Blocked,
    #[msg("the recipient takes cold DMs at a price: pay is short")]
    PayShort,
    #[msg("the recipient takes DMs only from accounts it follows")]
    FollowersOnly,
    #[msg("the recipient takes no new DMs")]
    Closed,
    #[msg("reclaim window out of the config's bounds")]
    BadWindow,
    #[msg("wrong account")]
    BadAccount,
    #[msg("only the operator named in the profile")]
    NotOperator,
    #[msg("the escrow deadline has not passed")]
    NotYet,
}
