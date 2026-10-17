//! Relayer-owned work account: proof bytes, fallback scratch and verification
//! progress for one header at a time. Created by the relayer with a plain system
//! `create_account` (owner = this program; any size, so it can exceed the 10 KiB CPI
//! limit), then `init_work`. Reused across headers (`begin_verify` resets it).
//!
//! Layout (little-endian):
//! ```text
//!    0   8  magic "PSLWORK1"
//!    8  32  authority
//!   40  32  header hash being verified
//!   72   4  proof_len         76   4  proof_cap
//!   80   1  flags             81   1  is_fallback      82   1  flags2
//!   84   2  noise blocks done (A)                     86   2  noise blocks done (B^T)
//!   88   8  gen_b_done (64-byte XOF blocks of B written)
//!   96  32  job_key          128  52  config
//!  180   4  m                184   4  n
//!  188  32  hash_a           220  32  hash_b
//!  256 1744  progress A (done u64, len u64, 54 CVs): first the proof hash, then
//!            (fallback) the hash_a rebuild, then the jackpot state (1092 bytes)
//! 2000 1744  progress B^T
//! 4096  ...  proof bytes (proof_cap), then scratch: B (k*n bytes) for fallback blocks
//!            while hashing B^T; afterwards (and for job blocks) the noised strips
//!            (h*dlen of A, then w*dlen of B^T) between the noise and jackpot steps.
//! ```

use anchor_lang::prelude::*;
use core::cell::RefMut;
use pearl_verify_sbf as pv;
use pv::fallback::Progress;
use pv::header::Header;
use pv::verify::{JobParams, Side};

use crate::{verr, HeaderRecord, LcError};

pub const MAGIC: &[u8; 8] = b"PSLWORK1";
pub const PROOF_OFF: usize = 4096;
const PROG_A: usize = 256;
const PROG_B: usize = 2000;
const PROG_LEN: usize = 16 + 54 * 32;

pub const F_FROZEN: u8 = 1 << 0;
pub const F_PROOF_OK: u8 = 1 << 1;
pub const F_A_OK: u8 = 1 << 2;
pub const F_BT_OK: u8 = 1 << 3;
pub const F_JACKPOT_OK: u8 = 1 << 4;
pub const F_FB_HASH_A: u8 = 1 << 5;
pub const F_FB_HASH_B: u8 = 1 << 6;
pub const F_BOUND: u8 = 1 << 7;
/// flags2 (byte 82)
pub const F2_NOISE_A: u8 = 1 << 0;
pub const F2_NOISE_B: u8 = 1 << 1;
pub const F2_JACKPOT: u8 = 1 << 2;

pub struct Work<'a, 'info> {
    d: RefMut<'a, &'info mut [u8]>,
}

fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(d[o..o + 2].try_into().unwrap())
}
fn rd_u64(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

impl<'a, 'info> Work<'a, 'info> {
    pub fn init(info: &'a AccountInfo<'info>, authority: &Pubkey, proof_cap: u32) -> Result<()> {
        let mut d = info.try_borrow_mut_data()?;
        require!(d.len() >= PROOF_OFF + proof_cap as usize, LcError::BadWork);
        require!(d[..8] == [0u8; 8] || &d[..8] == MAGIC, LcError::BadWork);
        if &d[..8] == MAGIC {
            // Re-init only by the same authority.
            require!(d[8..40] == authority.to_bytes(), LcError::BadWork);
        }
        d[..PROOF_OFF].fill(0);
        d[..8].copy_from_slice(MAGIC);
        d[8..40].copy_from_slice(&authority.to_bytes());
        d[76..80].copy_from_slice(&proof_cap.to_le_bytes());
        Ok(())
    }

    pub fn load(info: &'a AccountInfo<'info>, authority: &Pubkey) -> Result<Self> {
        let d = info.try_borrow_mut_data()?;
        require!(d.len() >= PROOF_OFF && &d[..8] == MAGIC, LcError::BadWork);
        require!(d[8..40] == authority.to_bytes(), LcError::BadWork);
        Ok(Self { d })
    }

    fn flags(&self) -> u8 {
        self.d[80]
    }
    fn set_flag(&mut self, f: u8) {
        self.d[80] |= f;
    }
    fn has(&self, f: u8) -> bool {
        self.flags() & f == f
    }
    fn proof_cap(&self) -> usize {
        rd_u32(&self.d, 76) as usize
    }
    fn proof_len(&self) -> usize {
        rd_u32(&self.d, 72) as usize
    }
    fn is_fallback(&self) -> bool {
        self.d[81] == 1
    }

    pub fn params(&self) -> Result<JobParams> {
        let d = &self.d;
        Ok(JobParams {
            job_key: d[96..128].try_into().unwrap(),
            config: pv::config::MiningConfig::from_bytes(&d[128..180]).map_err(verr)?,
            m: rd_u32(d, 180),
            n: rd_u32(d, 184),
            hash_a: d[188..220].try_into().unwrap(),
            hash_b: d[220..252].try_into().unwrap(),
        })
    }

    /// `cfg52` is the config bytes of `p.config` (fallback config or the job's).
    pub fn begin(&mut self, hash: &[u8; 32], proof_len: u32, fallback: bool, p: &JobParams, cfg52: &[u8; 52]) -> Result<()> {
        require!(proof_len as usize <= self.proof_cap(), LcError::ProofTooLarge);
        let cfg = *cfg52;
        let d = &mut self.d;
        d[40..72].copy_from_slice(hash);
        d[72..76].copy_from_slice(&proof_len.to_le_bytes());
        d[80] = F_BOUND;
        d[81] = fallback as u8;
        d[82] = 0;
        d[84..88].fill(0);
        d[88..96].fill(0);
        d[96..128].copy_from_slice(&p.job_key);
        d[128..180].copy_from_slice(&cfg);
        d[180..184].copy_from_slice(&p.m.to_le_bytes());
        d[184..188].copy_from_slice(&p.n.to_le_bytes());
        d[188..220].copy_from_slice(&p.hash_a);
        d[220..252].copy_from_slice(&p.hash_b);
        d[PROG_A..PROG_A + PROG_LEN].fill(0);
        d[PROG_B..PROG_B + PROG_LEN].fill(0);
        Ok(())
    }

    pub fn write_proof(&mut self, offset: u32, data: &[u8]) -> Result<()> {
        require!(self.has(F_BOUND), LcError::BadStep);
        require!(!self.has(F_FROZEN), LcError::Frozen);
        let end = offset as usize + data.len();
        require!(end <= self.proof_len(), LcError::ProofTooLarge);
        let o = PROOF_OFF + offset as usize;
        self.d[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }

    pub fn check_target(&self, rec: &HeaderRecord) -> Result<()> {
        require!(self.has(F_BOUND) && self.d[40..72] == rec.hash, LcError::WrongTarget2);
        require!(rec.status == crate::STATUS_PENDING, LcError::NotPending);
        Ok(())
    }

    fn proof(&self) -> &[u8] {
        &self.d[PROOF_OFF..PROOF_OFF + self.proof_len()]
    }

    /// Step 1, resumable: `count` 1 KiB chunks of blake3(proof) (progress in the A
    /// slot); with the last chunk, compare with the header, parse and check structure.
    pub fn proof_hash_step(&mut self, h: &Header, count: u64) -> Result<()> {
        require!(!self.has(F_PROOF_OK), LcError::BadStep);
        self.set_flag(F_FROZEN);
        let mut prog = self.load_prog(PROG_A);
        let root = pv::fallback::hash_plain(self.proof(), &mut prog, count);
        let Some(root) = root else {
            self.store_prog(PROG_A, &prog);
            return Ok(());
        };
        require!(root == h.proof_hash(), LcError::VerifyFailed);
        let p = self.params()?;
        let proof = self.proof();
        let v = pv::proof::parse(proof).map_err(verr)?;
        pv::verify::check_structure(proof, &v, &p, h.nonce()).map_err(verr)?;
        self.d[PROG_A..PROG_A + PROG_LEN].fill(0);
        self.set_flag(F_PROOF_OK);
        Ok(())
    }

    fn scratch_range(&self) -> Result<(usize, usize)> {
        let p = self.params()?;
        let start = PROOF_OFF + self.proof_cap();
        let len = p.config.k as usize * p.n as usize;
        require!(self.d.len() >= start + len, LcError::BadWork);
        Ok((start, len))
    }

    pub fn gen_b_step(&mut self, pow_digest: &[u8; 32], count: u64) -> Result<()> {
        require!(self.is_fallback(), LcError::BadStep);
        let (s, len) = self.scratch_range()?;
        let done = rd_u64(&self.d, 88);
        let total = (len / 64) as u64;
        require!(done < total, LcError::BadStep);
        let n = count.min(total - done);
        let xb = pv::fallback::xof(pv::fallback::CTX_B, pow_digest);
        let p = self.params()?;
        // Write blocks [done, done+n) of B, transposed, directly into account data.
        let scratch = &mut self.d[s..s + len];
        pv::fallback::gen_b(&xb, scratch, done, n, p.config.k, p.n);
        self.d[88..96].copy_from_slice(&(done + n).to_le_bytes());
        Ok(())
    }

    fn load_prog(&self, at: usize) -> Box<Progress> {
        let mut p = Box::<Progress>::default();
        p.done = rd_u64(&self.d, at);
        p.tree.len = rd_u64(&self.d, at + 8) as usize;
        for i in 0..p.tree.len {
            let o = at + 16 + 32 * i;
            let b: [u8; 32] = self.d[o..o + 32].try_into().unwrap();
            p.tree.stack[i] = pv::blake3::key_words(&b);
        }
        p
    }

    fn store_prog(&mut self, at: usize, p: &Progress) {
        self.d[at..at + 8].copy_from_slice(&p.done.to_le_bytes());
        self.d[at + 8..at + 16].copy_from_slice(&(p.tree.len as u64).to_le_bytes());
        for i in 0..p.tree.len {
            let o = at + 16 + 32 * i;
            self.d[o..o + 32].copy_from_slice(&pv::blake3::cv_bytes(&p.tree.stack[i]));
        }
    }

    pub fn hash_bt_step(&mut self, count: u64) -> Result<()> {
        require!(self.is_fallback() && !self.has(F_FB_HASH_B), LcError::BadStep);
        let (s, len) = self.scratch_range()?;
        require!(rd_u64(&self.d, 88) == (len / 64) as u64, LcError::BadStep);
        let p = self.params()?;
        let mut prog = self.load_prog(PROG_B);
        let root = pv::fallback::hash_bt(&p.job_key, &self.d[s..s + len], p.config.k, p.n, &mut prog, count);
        self.store_prog(PROG_B, &prog);
        if let Some(r) = root {
            self.d[220..252].copy_from_slice(&r);
            self.set_flag(F_FB_HASH_B);
        }
        Ok(())
    }

    pub fn hash_a_step(&mut self, pow_digest: &[u8; 32], count: u64) -> Result<()> {
        require!(self.is_fallback() && !self.has(F_FB_HASH_A), LcError::BadStep);
        // the A progress slot holds the proof hash until that step is done
        require!(self.has(F_PROOF_OK), LcError::BadStep);
        let p = self.params()?;
        let xa = pv::fallback::xof(pv::fallback::CTX_A, pow_digest);
        let mut prog = self.load_prog(PROG_A);
        let root = pv::fallback::hash_a(&p.job_key, &xa, p.m, p.config.k, &mut prog, count);
        self.store_prog(PROG_A, &prog);
        if let Some(r) = root {
            self.d[188..220].copy_from_slice(&r);
            self.set_flag(F_FB_HASH_A);
        }
        Ok(())
    }

    pub fn opening_step(&mut self, side: Side) -> Result<()> {
        require!(self.has(F_PROOF_OK), LcError::BadStep);
        if self.is_fallback() {
            let need = if side == Side::A { F_FB_HASH_A } else { F_FB_HASH_B };
            require!(self.has(need), LcError::BadStep);
        }
        let p = self.params()?;
        let proof = self.proof();
        let v = pv::proof::parse(proof).map_err(verr)?;
        pv::verify::check_opening(proof, &v, &p, side).map_err(verr)?;
        self.set_flag(if side == Side::A { F_A_OK } else { F_BT_OK });
        Ok(())
    }

    fn noise_layout(&self, p: &JobParams) -> Result<(usize, usize, usize)> {
        let start = PROOF_OFF + self.proof_cap();
        let dlen = p.config.dlen() as usize;
        let (ha, hb) = (p.config.h() as usize * dlen, p.config.w() as usize * dlen);
        require!(self.d.len() >= start + ha + hb, LcError::BadWork);
        Ok((start, ha, hb))
    }

    /// Step 3a, resumable: noise column blocks `[done, done + count)` of one side into
    /// scratch (needs both commitments and that side's opening, so for fallback blocks
    /// B is no longer needed).
    pub fn noise_step(&mut self, side: Side, pow_digest: &[u8; 32], count: u32) -> Result<()> {
        let need = if side == Side::A { F_PROOF_OK | F_A_OK } else { F_PROOF_OK | F_BT_OK };
        require!(self.has(need), LcError::BadStep);
        if self.is_fallback() {
            require!(self.has(F_FB_HASH_A | F_FB_HASH_B), LcError::BadStep);
        }
        let (at, flag) = if side == Side::A { (84, F2_NOISE_A) } else { (86, F2_NOISE_B) };
        require!(self.d[82] & flag == 0, LcError::BadStep);
        let p = self.params()?;
        let (start, ha, hb) = self.noise_layout(&p)?;
        let nb = pv::verify::noise_blocks(&p);
        let done = rd_u16(&self.d, at) as usize;
        let end = (done + count as usize).min(nb);
        let (o, len) = if side == Side::A { (0, ha) } else { (ha, hb) };
        let plen = self.proof_len();
        let d: &mut [u8] = &mut self.d;
        let (head, tail) = d.split_at_mut(start);
        let proof = &head[PROOF_OFF..PROOF_OFF + plen];
        let v = pv::proof::parse(proof).map_err(verr)?;
        pv::verify::noised_side_range(proof, &v, &p, pow_digest, side, done, end, &mut tail[o..o + len]).map_err(verr)?;
        self.d[at..at + 2].copy_from_slice(&(end as u16).to_le_bytes());
        if end == nb {
            self.d[82] |= flag;
        }
        Ok(())
    }

    /// Step 3b, resumable: `count` rank blocks of the jackpot over the stored noised
    /// strips (state in the A slot); with the last block, hash and target. Returns true
    /// when the jackpot is complete and meets the target.
    pub fn jackpot_step(&mut self, pow_digest: &[u8; 32], target_bits: u32, count: u32) -> Result<bool> {
        require!(self.has(F_PROOF_OK | F_A_OK | F_BT_OK), LcError::BadStep);
        require!(!self.has(F_JACKPOT_OK), LcError::BadStep);
        require!(self.d[82] & (F2_NOISE_A | F2_NOISE_B) == F2_NOISE_A | F2_NOISE_B, LcError::BadStep);
        if self.d[82] & F2_JACKPOT == 0 {
            self.d[PROG_A..PROG_A + PROG_LEN].fill(0);
            self.d[82] |= F2_JACKPOT;
        }
        let p = self.params()?;
        let (start, ha, hb) = self.noise_layout(&p)?;
        let mut st = Box::<pv::verify::JackpotState>::default();
        st.read_from(&self.d[PROG_A..PROG_A + pv::verify::JackpotState::BYTES]);
        let done = {
            let a: &[i8] = unsafe { core::slice::from_raw_parts(self.d[start..].as_ptr() as *const i8, ha) };
            let bt: &[i8] = unsafe { core::slice::from_raw_parts(self.d[start + ha..].as_ptr() as *const i8, hb) };
            pv::verify::jackpot_step(a, bt, &p, &mut st, count).map_err(verr)?
        };
        st.to_bytes(&mut self.d[PROG_A..PROG_A + pv::verify::JackpotState::BYTES]);
        if !done {
            return Ok(false);
        }
        pv::verify::jackpot_finish(&st, &p, pow_digest, target_bits).map_err(verr)?;
        self.set_flag(F_JACKPOT_OK);
        Ok(true)
    }
}
