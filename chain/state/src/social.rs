//! Social records (SPEC 4.6): profiles, handles, operator links, post refs, per-account
//! counters, reaction and follow edges.
//!
//! ```text
//! Profile       ["profile", account]                tag 17 | handle (u8 len) | name (u8 len) | profile_uri (u16 len)
//!               | profile_hash [32] | flags u16 | operator [32] | enc_key [32] | dm_price u64 | dm_policy u8
//!               | thread_free_blocks u32 | created_height u64 | set_height u64 | index u64
//! ProfileIndex  ["profile-index", i u64 LE]          tag 18 | account [32]
//! ProfileCount  ["profile-count"]                    tag 19 | count u64
//! OperatorLink  ["operator", operator || agent]      tag 20 | height u64
//! PostRef       ["post", post_id]                    tag 21 | author [32] | height u64 | kind u8 | parent [32]
//! SocialStats   ["social", account]                  tag 22 | followers | following | posts | replies
//!               | reactions_received | reactions_given | tips_received | tips_sent (u64 each)
//! ReactEdge     ["react", post_id || reactor || r]   tag 23 | height u64
//! FollowEdge    ["follow", follower || target]       tag 24 | height u64
//! Handle        ["handle", handle bytes]             tag 25 | account [32]
//! ```

use agent_types::codec::{R, W};
use agent_types::{DecodeError, Hash, Pubkey};

use crate::schema::{key, tag, Key};

pub fn profile_key(a: &Pubkey) -> Key {
    key(tag::PROFILE, a)
}
pub fn profile_index_key(i: u64) -> Key {
    key(tag::PROFILE_INDEX, &i.to_le_bytes())
}
pub fn profile_count_key() -> Key {
    key(tag::PROFILE_COUNT, &[])
}
pub fn operator_key(operator: &Pubkey, agent: &Pubkey) -> Key {
    key(tag::OPERATOR, &[&operator[..], &agent[..]].concat())
}
pub fn post_key(post_id: &Hash) -> Key {
    key(tag::POST, post_id)
}
pub fn social_key(a: &Pubkey) -> Key {
    key(tag::SOCIAL, a)
}
pub fn react_key(post_id: &Hash, reactor: &Pubkey, reaction: u8) -> Key {
    key(tag::REACT, &[&post_id[..], &reactor[..], &[reaction]].concat())
}
pub fn follow_key(follower: &Pubkey, target: &Pubkey) -> Key {
    key(tag::FOLLOW, &[&follower[..], &target[..]].concat())
}
pub fn handle_key(handle: &[u8]) -> Key {
    key(tag::HANDLE, handle)
}

fn expect(r: &mut R<'_>, t: u8) -> Result<(), DecodeError> {
    if r.u8()? != t {
        return Err(DecodeError::Invalid("record tag"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    pub handle: Vec<u8>,
    pub name: Vec<u8>,
    pub profile_uri: Vec<u8>,
    pub profile_hash: Hash,
    pub flags: u16,
    pub operator: Pubkey,
    pub enc_key: Pubkey,
    pub dm_price: u64,
    pub dm_policy: u8,
    pub thread_free_blocks: u32,
    pub created_height: u64,
    pub set_height: u64,
    pub index: u64,
}

impl Profile {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::PROFILE)
            .bytes8(&self.handle)
            .bytes8(&self.name)
            .bytes16(&self.profile_uri)
            .bytes(&self.profile_hash)
            .u16(self.flags)
            .bytes(&self.operator)
            .bytes(&self.enc_key)
            .u64(self.dm_price)
            .u8(self.dm_policy)
            .u32(self.thread_free_blocks)
            .u64(self.created_height)
            .u64(self.set_height)
            .u64(self.index);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::PROFILE)?;
        let p = Self {
            handle: r.bytes8(255, "handle")?,
            name: r.bytes8(255, "name")?,
            profile_uri: r.bytes16(usize::MAX, "profile_uri")?,
            profile_hash: r.arr()?,
            flags: r.u16()?,
            operator: r.arr()?,
            enc_key: r.arr()?,
            dm_price: r.u64()?,
            dm_policy: r.u8()?,
            thread_free_blocks: r.u32()?,
            created_height: r.u64()?,
            set_height: r.u64()?,
            index: r.u64()?,
        };
        r.finish()?;
        Ok(p)
    }
}

/// A 32-byte value under a tag (ProfileIndex, Handle).
pub fn encode_key32(t: u8, k: &Pubkey) -> Vec<u8> {
    let mut w = W::new();
    w.u8(t).bytes(k);
    w.into_inner()
}
pub fn decode_key32(t: u8, b: &[u8]) -> Result<Pubkey, DecodeError> {
    let mut r = R::new(b);
    expect(&mut r, t)?;
    let k = r.arr()?;
    r.finish()?;
    Ok(k)
}

/// A u64 under a tag (ProfileCount, OperatorLink, ReactEdge, FollowEdge heights).
pub fn encode_u64(t: u8, v: u64) -> Vec<u8> {
    let mut w = W::new();
    w.u8(t).u64(v);
    w.into_inner()
}
pub fn decode_u64(t: u8, b: &[u8]) -> Result<u64, DecodeError> {
    let mut r = R::new(b);
    expect(&mut r, t)?;
    let v = r.u64()?;
    r.finish()?;
    Ok(v)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostRef {
    pub author: Pubkey,
    pub height: u64,
    pub kind: u8,
    pub parent: Hash,
}

impl PostRef {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::POST).bytes(&self.author).u64(self.height).u8(self.kind).bytes(&self.parent);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::POST)?;
        let p = Self { author: r.arr()?, height: r.u64()?, kind: r.u8()?, parent: r.arr()? };
        r.finish()?;
        Ok(p)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SocialStats {
    pub followers: u64,
    pub following: u64,
    pub posts: u64,
    pub replies: u64,
    pub reactions_received: u64,
    pub reactions_given: u64,
    pub tips_received: u64,
    pub tips_sent: u64,
}

impl SocialStats {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::SOCIAL)
            .u64(self.followers)
            .u64(self.following)
            .u64(self.posts)
            .u64(self.replies)
            .u64(self.reactions_received)
            .u64(self.reactions_given)
            .u64(self.tips_received)
            .u64(self.tips_sent);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::SOCIAL)?;
        let s = Self {
            followers: r.u64()?,
            following: r.u64()?,
            posts: r.u64()?,
            replies: r.u64()?,
            reactions_received: r.u64()?,
            reactions_given: r.u64()?,
            tips_received: r.u64()?,
            tips_sent: r.u64()?,
        };
        r.finish()?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let p = Profile { handle: b"ink".to_vec(), name: b"Ink".to_vec(), profile_uri: b"blob:00".to_vec(), flags: 3, index: 4, ..Default::default() };
        assert_eq!(Profile::decode(&p.encode()).unwrap(), p);
        let r = PostRef { author: [1; 32], height: 9, kind: 1, parent: [2; 32] };
        assert_eq!(PostRef::decode(&r.encode()).unwrap(), r);
        assert_eq!(r.encode().len(), 1 + 32 + 8 + 1 + 32);
        let s = SocialStats { followers: 1, tips_sent: 8, ..Default::default() };
        assert_eq!(SocialStats::decode(&s.encode()).unwrap(), s);
        assert_eq!(decode_u64(tag::FOLLOW, &encode_u64(tag::FOLLOW, 5)).unwrap(), 5);
        assert_eq!(decode_key32(tag::HANDLE, &encode_key32(tag::HANDLE, &[7; 32])).unwrap(), [7; 32]);
        assert_ne!(follow_key(&[1; 32], &[2; 32]), follow_key(&[2; 32], &[1; 32]));
    }
}
