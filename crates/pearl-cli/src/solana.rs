//! Token-chain (Solana) side: JSON-RPC reads, PDA derivation, raw account layouts of
//! our bridge and storefront programs, and the one write the CLI does there (bridge
//! `deposit`, as a legacy transaction).
//!
//! No solana-sdk: it would double the build. What is here is Solana's public wire
//! format (PDA rule, legacy message, compact-u16) plus our programs' Anchor layouts,
//! whose offsets are written down in token/programs/{bridge,storefront}/src/lib.rs.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub type Pubkey = [u8; 32];

/// Network profile: endpoints and our Solana program ids (`declare_id!`; the devnet ids
/// come from the `devnet` feature build, docs/DEVNET.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Network {
    Devnet,
    Localnet,
}

pub struct Profile {
    pub rpc: &'static str,
    pub sol_rpc: &'static str,
    pub pool: Option<&'static str>,
    pub bridge: &'static str,
    pub storefront: &'static str,
    pub reserve: &'static str,
    pub lightclient: &'static str,
}

pub const LOCALNET: Profile = Profile {
    rpc: "http://127.0.0.1:10630",
    sol_rpc: "http://127.0.0.1:10600",
    pool: Some("http://127.0.0.1:10663"),
    bridge: "7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8",
    storefront: "DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr",
    reserve: "4FkQcoCMJdRpZW7bK3wnaMoLjYdWaJBDB8xTUGskLY6p",
    lightclient: "BUDUW2bGexRGRHhEkBUj8dAvffRuXQQacredxKtRMHK5",
};

pub const DEVNET: Profile = Profile {
    rpc: "https://rpc.64-95-12-116.sslip.io",
    sol_rpc: "https://api.devnet.solana.com",
    pool: None,
    bridge: "3S226LmVvsoA4QrEpwytiuLxREJrQHS7A1UkeDsJGFoN",
    storefront: "9XjvGk2xJdByPL6NmUCQg7RdBLyeDjWjikZBnwuWvAKu",
    reserve: "8m1RBPiiscHWpXRDj4NevSFMN68tbgCC8f4hu6N4hVzi",
    lightclient: "47mavKipT2URcdj9qp2Stv6H5cM41cmzDXiTEcmDie4m",
};

impl Network {
    pub fn profile(self) -> &'static Profile {
        match self {
            Network::Devnet => &DEVNET,
            Network::Localnet => &LOCALNET,
        }
    }
}

static NETWORK: std::sync::OnceLock<Network> = std::sync::OnceLock::new();

/// Set once per process by `Ctx::new` (tests always use localnet).
pub fn set_network(n: Network) {
    let _ = NETWORK.set(n);
}

pub fn profile() -> &'static Profile {
    NETWORK.get().copied().unwrap_or(Network::Localnet).profile()
}

/// token/programs/bridge `declare_id!` of the active network.
pub fn bridge_program() -> Pubkey {
    pk(profile().bridge)
}

/// token/programs/storefront `declare_id!` of the active network.
pub fn storefront_program() -> Pubkey {
    pk(profile().storefront)
}

/// token/programs/reserve `declare_id!` of the active network.
pub fn reserve_program() -> Pubkey {
    pk(profile().reserve)
}
pub const ATA_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
pub const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
pub const SYSTEM_PROGRAM: Pubkey = [0; 32];

/// Bridge `Processed` seed chain ids (bridge lib.rs).
pub const CHAIN_TOKEN: u8 = 0;
pub const CHAIN_AGENT: u8 = 1;

pub fn pk(s: &str) -> Pubkey {
    bs58::decode(s).into_vec().ok().and_then(|v| v.try_into().ok()).expect("valid program id constant")
}

pub fn b58(p: &Pubkey) -> String {
    bs58::encode(p).into_string()
}

/// Solana `find_program_address`: the highest bump whose
/// `sha256(seeds || bump || program || "ProgramDerivedAddress")` is off the curve.
pub fn find_pda(seeds: &[&[u8]], program: &Pubkey) -> (Pubkey, u8) {
    for bump in (0..=255u8).rev() {
        let mut h = Sha256::new();
        for s in seeds {
            h.update(s);
        }
        h.update([bump]);
        h.update(program);
        h.update(b"ProgramDerivedAddress");
        let a: Pubkey = h.finalize().into();
        if curve25519_dalek::edwards::CompressedEdwardsY(a).decompress().is_none() {
            return (a, bump);
        }
    }
    unreachable!("no viable bump (probability 2^-256)")
}

/// Associated token account of `owner` for `mint` under `token_program`.
pub fn ata(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    find_pda(&[owner, token_program, mint], &pk(ATA_PROGRAM)).0
}

pub fn bridge_config_pda() -> Pubkey {
    find_pda(&[b"config"], &bridge_program()).0
}

pub fn storefront_config_pda() -> Pubkey {
    find_pda(&[b"config"], &storefront_program()).0
}

/// Storefront buyback vault (system-owned, data-less): holds the order shares until a buy.
pub fn buyback_vault_pda() -> Pubkey {
    find_pda(&[b"buyback"], &storefront_program()).0
}

/// The mining reserve's token account: ATA(reserve creator PDA, mint, Token-2022).
pub fn reserve_ata(reserve_program: &Pubkey, mint: &Pubkey) -> Pubkey {
    ata(&find_pda(&[b"creator"], reserve_program).0, mint, &pk(TOKEN_2022_PROGRAM))
}

pub fn deposit_pda(nonce: u64) -> Pubkey {
    find_pda(&[b"deposit", &nonce.to_le_bytes()], &bridge_program()).0
}

/// `Processed ["processed", [chain], nonce]`: CHAIN_AGENT = a released withdrawal,
/// CHAIN_TOKEN = the refund record of a Solana deposit.
pub fn processed_pda(chain: u8, nonce: u64) -> Pubkey {
    find_pda(&[b"processed", &[chain], &nonce.to_le_bytes()], &bridge_program()).0
}

/// Anchor instruction discriminator `sha256("global:<name>")[..8]`.
pub fn anchor_disc(name: &str) -> [u8; 8] {
    let h: [u8; 32] = Sha256::digest(format!("global:{name}").as_bytes()).into();
    h[..8].try_into().unwrap()
}

struct Rd<'a>(&'a [u8], usize);

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.0.get(self.1..self.1 + n).ok_or_else(|| anyhow!("account data too short at byte {}", self.1))?;
        self.1 += n;
        Ok(s)
    }
    fn key(&mut self) -> Result<Pubkey> {
        Ok(self.take(32)?.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn keys(&mut self) -> Result<Vec<Pubkey>> {
        let n = self.u32()? as usize;
        ensure!(n <= 64, "vector length {n} too large");
        (0..n).map(|_| self.key()).collect()
    }
}

/// Bridge `Config` (bridge lib.rs, after the 8-byte discriminator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub admin: Pubkey,
    pub mint: Pubkey,
    pub vault: Pubkey,
    pub token_program: Pubkey,
    /// Nonce the next deposit gets (= deposits so far).
    pub deposit_nonce: u64,
    pub threshold: u8,
    pub signers: Vec<Pubkey>,
    /// MODE_SIGNERS 1 | MODE_PROOF 2.
    pub release_modes: u8,
    pub light_client: Pubkey,
}

impl BridgeConfig {
    pub fn parse(d: &[u8]) -> Result<Self> {
        let mut r = Rd(d, 8);
        let admin = r.key()?;
        let mint = r.key()?;
        let vault = r.key()?;
        let token_program = r.key()?;
        let deposit_nonce = r.u64()?;
        let threshold = r.u8()?;
        let _bump = r.u8()?;
        let signers = r.keys()?;
        let release_modes = r.u8()?;
        let light_client = r.key()?;
        Ok(Self { admin, mint, vault, token_program, deposit_nonce, threshold, signers, release_modes, light_client })
    }
}

/// Storefront `Config` (storefront lib.rs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorefrontConfig {
    pub paused: bool,
    pub protocol_fee_bps: u16,
    pub max_slippage_bps: u16,
    /// Smallest SOL a job order may spend: the lowest price of a job, lamports.
    pub min_sol_in: u64,
    pub max_sol_in: u64,
    pub max_dim: u32,
    pub max_blocks_cap: u32,
    pub max_deadline_blocks: u64,
    pub orders: u64,
    /// Lamports spent on all orders so far.
    pub total_sol: u64,
    /// SPL units bought by all orders so far.
    pub total_tokens: u64,
}

impl StorefrontConfig {
    pub fn parse(d: &[u8]) -> Result<Self> {
        let mut r = Rd(d, 8);
        let _admin = r.key()?;
        let _mint = r.key()?;
        let _bridge = r.key()?;
        let paused = r.u8()? != 0;
        let protocol_fee_bps = r.u16()?;
        let max_slippage_bps = r.u16()?;
        let min_sol_in = r.u64()?;
        let max_sol_in = r.u64()?;
        let max_dim = r.u32()?;
        let max_blocks_cap = r.u32()?;
        let max_deadline_blocks = r.u64()?;
        let _threshold = r.u8()?;
        let _attesters = r.keys()?;
        let orders = r.u64()?;
        let total_sol = r.u64()?;
        let total_tokens = r.u64()?;
        Ok(Self {
            paused,
            protocol_fee_bps,
            max_slippage_bps,
            min_sol_in,
            max_sol_in,
            max_dim,
            max_blocks_cap,
            max_deadline_blocks,
            orders,
            total_sol,
            total_tokens,
        })
    }
}

/// Storefront buyback settings and totals (Config fields appended 2026-10-18, storefront
/// lib.rs "Buyback"). A config from before that reads as zeros: share 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Buyback {
    pub mint: Pubkey,
    pub bps: u16,
    /// 0 = the mining reserve, 1 = burn.
    pub dest: u8,
    pub reserve_program: Pubkey,
    /// Zero = permissionless.
    pub cranker: Pubkey,
    /// 0 = no cap.
    pub max_per_call: u64,
    pub min_buy: u64,
    pub min_interval_secs: i64,
    pub last_ts: i64,
    pub accrued: u64,
    pub spent: u64,
    pub tokens: u64,
    pub to_reserve: u64,
    pub burned: u64,
    pub buys: u64,
    pub pending_orders: u64,
    pub pending_first_nonce: u64,
    pub pending_last_nonce: u64,
    /// Shares held for unsettled orders (not crankable; refunded if the order is).
    pub locked: u64,
    /// Shares paid back to buyers of refunded / expired orders.
    pub refunded: u64,
    /// Reserve cap (appended 2026-10-19): with dest reserve, buys go to the reserve while
    /// it holds less, the rest is burned. 0 = no cap (also a config from before the field).
    pub reserve_cap: u64,
}

impl Buyback {
    pub fn parse(d: &[u8]) -> Result<Self> {
        let mut r = Rd(d, 8);
        let _admin = r.key()?;
        let mint = r.key()?;
        r.take(32 + 1 + 2 + 2 + 8 + 8 + 4 + 4 + 8 + 1)?; // bridge .. threshold
        let _attesters = r.keys()?;
        r.take(8 * 3 + 3 + 32)?; // orders, total_sol, total_tokens, bump, trader_bump, finalize_modes, light_client
        if d.len() < r.1 + 171 {
            return Ok(Self { mint, ..Self::default() });
        }
        let mut b = Self {
            mint,
            bps: r.u16()?,
            dest: r.u8()?,
            reserve_program: r.key()?,
            cranker: r.key()?,
            max_per_call: r.u64()?,
            min_buy: r.u64()?,
            min_interval_secs: r.i64()?,
            last_ts: r.i64()?,
            accrued: r.u64()?,
            spent: r.u64()?,
            tokens: r.u64()?,
            to_reserve: r.u64()?,
            burned: r.u64()?,
            buys: r.u64()?,
            pending_orders: r.u64()?,
            pending_first_nonce: r.u64()?,
            pending_last_nonce: r.u64()?,
            locked: 0,
            refunded: 0,
            reserve_cap: 0,
        };
        if d.len() >= r.1 + 16 {
            b.locked = r.u64()?;
            b.refunded = r.u64()?;
        }
        if d.len() >= r.1 + 8 {
            b.reserve_cap = r.u64()?;
        }
        Ok(b)
    }

    pub fn dest_name(&self) -> &'static str {
        match self.dest {
            0 => "reserve",
            1 => "burn",
            _ => "unknown",
        }
    }
}

/// Anchor event discriminator `sha256("event:<Name>")[..8]`.
pub fn event_disc(name: &str) -> [u8; 8] {
    let h: [u8; 32] = Sha256::digest(format!("event:{name}").as_bytes()).into();
    h[..8].try_into().unwrap()
}

/// Storefront `BuybackExecuted` event (one crank buy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuybackExecuted {
    pub buy: u64,
    pub venue: u8,
    pub sol_in: u64,
    pub tokens_out: u64,
    pub routed: u64,
    pub destination: u8,
    pub destination_account: Pubkey,
    pub quote: u64,
    pub first_nonce: u64,
    pub last_nonce: u64,
    pub orders: u64,
    pub available_before: u64,
    pub carry: u64,
    pub total_spent: u64,
    pub total_tokens: u64,
    pub total_to_reserve: u64,
    pub total_burned: u64,
}

/// Storefront `BuybackAccrued` event (one order's share).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuybackAccrued {
    pub order: Pubkey,
    pub nonce: u64,
    pub lamports: u64,
    pub vault_available: u64,
    pub total_accrued: u64,
}

/// Storefront `BuybackToReserve`: the part of buy `buy` sent to the mining reserve under the cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuybackToReserve {
    pub buy: u64,
    pub tokens: u64,
    pub reserve_before: u64,
    pub reserve_after: u64,
    pub reserve_cap: u64,
}

/// Storefront `BuybackBurned`: the part of buy `buy` burned (reason 0 above cap, 1 destination burn).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuybackBurned {
    pub buy: u64,
    pub tokens: u64,
    pub reason: u8,
    pub reserve_before: u64,
    pub reserve_cap: u64,
    pub supply_after: u64,
    pub total_burned: u64,
}

pub enum StoreEvent {
    Executed(BuybackExecuted),
    Accrued(BuybackAccrued),
    ToReserve(BuybackToReserve),
    Burned(BuybackBurned),
}

/// Decodes a `Program data:` payload (event disc + Borsh fields) if it is a buyback event.
pub fn decode_store_event(b: &[u8]) -> Option<StoreEvent> {
    if b.len() < 8 {
        return None;
    }
    let mut r = Rd(b, 8);
    if b[..8] == event_disc("BuybackExecuted") {
        let e = BuybackExecuted {
            buy: r.u64().ok()?,
            venue: r.u8().ok()?,
            sol_in: r.u64().ok()?,
            tokens_out: r.u64().ok()?,
            routed: r.u64().ok()?,
            destination: r.u8().ok()?,
            destination_account: r.key().ok()?,
            quote: r.u64().ok()?,
            first_nonce: r.u64().ok()?,
            last_nonce: r.u64().ok()?,
            orders: r.u64().ok()?,
            available_before: r.u64().ok()?,
            carry: r.u64().ok()?,
            total_spent: r.u64().ok()?,
            total_tokens: r.u64().ok()?,
            total_to_reserve: r.u64().ok()?,
            total_burned: r.u64().ok()?,
        };
        return Some(StoreEvent::Executed(e));
    }
    if b[..8] == event_disc("BuybackAccrued") {
        let e = BuybackAccrued { order: r.key().ok()?, nonce: r.u64().ok()?, lamports: r.u64().ok()?, vault_available: r.u64().ok()?, total_accrued: r.u64().ok()? };
        return Some(StoreEvent::Accrued(e));
    }
    if b[..8] == event_disc("BuybackToReserve") {
        let e = BuybackToReserve { buy: r.u64().ok()?, tokens: r.u64().ok()?, reserve_before: r.u64().ok()?, reserve_after: r.u64().ok()?, reserve_cap: r.u64().ok()? };
        return Some(StoreEvent::ToReserve(e));
    }
    if b[..8] == event_disc("BuybackBurned") {
        let e = BuybackBurned {
            buy: r.u64().ok()?,
            tokens: r.u64().ok()?,
            reason: r.u8().ok()?,
            reserve_before: r.u64().ok()?,
            reserve_cap: r.u64().ok()?,
            supply_after: r.u64().ok()?,
            total_burned: r.u64().ok()?,
        };
        return Some(StoreEvent::Burned(e));
    }
    None
}

/// Mint supply (SPL Token / Token-2022 base layout: supply u64 at 36).
pub fn mint_supply(d: &[u8]) -> Result<u64> {
    let mut r = Rd(d, 36);
    r.u64()
}

/// Bridge `Deposit`: `nonce u64, sender [32], recipient [32], amount u64, slot u64, bump u8`.
pub fn deposit_amount(d: &[u8]) -> Result<u64> {
    let mut r = Rd(d, 8 + 8 + 32 + 32);
    r.u64()
}

pub struct SolRpc {
    pub url: String,
    agent: ureq::Agent,
}

impl SolRpc {
    pub fn new(url: &str) -> Self {
        Self { url: url.to_string(), agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build() }
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let v: Value = match self.agent.post(&self.url).send_json(body) {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(code, r)) => r.into_json().unwrap_or_else(|_| json!({"error": {"message": format!("HTTP {code}")}})),
            Err(e) => bail!("solana {method}: {e}"),
        };
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            bail!("solana {method}: {}", e.get("message").and_then(|m| m.as_str()).unwrap_or(&e.to_string()));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    fn decode_data(v: &Value) -> Result<Option<Vec<u8>>> {
        if v.is_null() {
            return Ok(None);
        }
        let d = v["data"][0].as_str().ok_or_else(|| anyhow!("account data not base64"))?;
        Ok(Some(base64::engine::general_purpose::STANDARD.decode(d)?))
    }

    pub fn account(&self, a: &Pubkey) -> Result<Option<Vec<u8>>> {
        let v = self.call("getAccountInfo", json!([b58(a), {"encoding": "base64", "commitment": "confirmed"}]))?;
        Self::decode_data(&v["value"])
    }

    /// Data of each account, None where it does not exist (100 per call).
    pub fn accounts(&self, keys: &[Pubkey]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut out = Vec::with_capacity(keys.len());
        for chunk in keys.chunks(100) {
            let ks: Vec<String> = chunk.iter().map(b58).collect();
            let v = self.call("getMultipleAccounts", json!([ks, {"encoding": "base64", "commitment": "confirmed"}]))?;
            for a in v["value"].as_array().ok_or_else(|| anyhow!("getMultipleAccounts: no value"))? {
                out.push(Self::decode_data(a)?);
            }
        }
        Ok(out)
    }

    pub fn balance(&self, a: &Pubkey) -> Result<u64> {
        let v = self.call("getBalance", json!([b58(a), {"commitment": "confirmed"}]))?;
        v["value"].as_u64().ok_or_else(|| anyhow!("getBalance: no value"))
    }

    /// SPL amount of a token account; None if it does not exist.
    pub fn token_balance(&self, a: &Pubkey) -> Result<Option<u64>> {
        match self.call("getTokenAccountBalance", json!([b58(a), {"commitment": "confirmed"}])) {
            Ok(v) => Ok(Some(v["value"]["amount"].as_str().ok_or_else(|| anyhow!("token balance has no amount"))?.parse()?)),
            Err(e) if e.to_string().contains("could not find account") || e.to_string().contains("Invalid param") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn latest_blockhash(&self) -> Result<[u8; 32]> {
        let v = self.call("getLatestBlockhash", json!([{"commitment": "confirmed"}]))?;
        let s = v["value"]["blockhash"].as_str().ok_or_else(|| anyhow!("no blockhash"))?;
        bs58::decode(s).into_vec()?.try_into().map_err(|_| anyhow!("blockhash is not 32 bytes"))
    }

    pub fn send(&self, tx: &[u8]) -> Result<String> {
        let b = base64::engine::general_purpose::STANDARD.encode(tx);
        let v = self.call("sendTransaction", json!([b, {"encoding": "base64", "preflightCommitment": "confirmed"}]))?;
        v.as_str().map(String::from).ok_or_else(|| anyhow!("sendTransaction returned {v}"))
    }

    /// Wait until `sig` is confirmed; Err if it failed or `timeout` passes.
    pub fn confirm(&self, sig: &str, timeout: Duration) -> Result<u64> {
        let t0 = Instant::now();
        loop {
            let v = self.call("getSignatureStatuses", json!([[sig], {"searchTransactionHistory": true}]))?;
            let s = &v["value"][0];
            if !s.is_null() {
                if !s["err"].is_null() {
                    bail!("solana tx {sig} failed: {}", s["err"]);
                }
                let c = s["confirmationStatus"].as_str().unwrap_or("");
                if c == "confirmed" || c == "finalized" {
                    return Ok(s["slot"].as_u64().unwrap_or(0));
                }
            }
            ensure!(t0.elapsed() < timeout, "solana tx {sig} not confirmed after {:?}", timeout);
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

#[derive(Debug, Clone)]
pub struct Meta {
    pub key: Pubkey,
    pub signer: bool,
    pub writable: bool,
}

impl Meta {
    pub fn w(key: Pubkey) -> Self {
        Self { key, signer: false, writable: true }
    }
    pub fn r(key: Pubkey) -> Self {
        Self { key, signer: false, writable: false }
    }
    pub fn ws(key: Pubkey) -> Self {
        Self { key, signer: true, writable: true }
    }
}

#[derive(Debug, Clone)]
pub struct Ix {
    pub program: Pubkey,
    pub accounts: Vec<Meta>,
    pub data: Vec<u8>,
}

/// Solana compact-u16 (shortvec).
pub fn shortvec(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut b = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            return;
        }
    }
}

/// A signed legacy transaction, `payer` the only signer and fee payer.
pub fn legacy_tx(payer: &SigningKey, ixs: &[Ix], blockhash: &[u8; 32]) -> Result<Vec<u8>> {
    let me = payer.verifying_key().to_bytes();
    // unique keys with merged flags; payer first, programs read-only
    let mut keys: Vec<Meta> = vec![Meta::ws(me)];
    let mut add = |m: Meta| match keys.iter_mut().find(|k| k.key == m.key) {
        Some(k) => {
            k.signer |= m.signer;
            k.writable |= m.writable;
        }
        None => keys.push(m),
    };
    for ix in ixs {
        for a in &ix.accounts {
            add(a.clone());
        }
        add(Meta::r(ix.program));
    }
    for k in &keys {
        ensure!(!k.signer || k.key == me, "only the payer can sign here, {} also needs to", b58(&k.key));
    }
    let class = |m: &Meta| match (m.signer, m.writable) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    };
    keys.sort_by_key(class); // stable: payer stays first
    let n_ro_signed = keys.iter().filter(|k| class(k) == 1).count();
    let n_ro = keys.iter().filter(|k| class(k) == 3).count();
    let mut msg = vec![1u8, n_ro_signed as u8, n_ro as u8];
    shortvec(keys.len(), &mut msg);
    for k in &keys {
        msg.extend_from_slice(&k.key);
    }
    msg.extend_from_slice(blockhash);
    shortvec(ixs.len(), &mut msg);
    let idx = |p: &Pubkey| keys.iter().position(|k| k.key == *p).unwrap() as u8;
    for ix in ixs {
        msg.push(idx(&ix.program));
        shortvec(ix.accounts.len(), &mut msg);
        for a in &ix.accounts {
            msg.push(idx(&a.key));
        }
        shortvec(ix.data.len(), &mut msg);
        msg.extend_from_slice(&ix.data);
    }
    let sig = payer.sign(&msg);
    let mut tx = vec![];
    shortvec(1, &mut tx);
    tx.extend_from_slice(&sig.to_bytes());
    tx.extend_from_slice(&msg);
    ensure!(tx.len() <= 1232, "transaction is {} bytes (max 1232)", tx.len());
    Ok(tx)
}

/// Bridge `deposit(amount, agent_recipient)` with `sender = payer` (DepositIx account order).
pub fn deposit_ix(cfg: &BridgeConfig, sender: &Pubkey, amount_spl: u64, agent_recipient: &Pubkey) -> Ix {
    let mut data = anchor_disc("deposit").to_vec();
    data.extend_from_slice(&amount_spl.to_le_bytes());
    data.extend_from_slice(agent_recipient);
    Ix {
        program: bridge_program(),
        accounts: vec![
            Meta::ws(*sender),
            Meta::ws(*sender),
            Meta::w(bridge_config_pda()),
            Meta::r(cfg.mint),
            Meta::w(ata(sender, &cfg.mint, &cfg.token_program)),
            Meta::w(cfg.vault),
            Meta::w(deposit_pda(cfg.deposit_nonce)),
            Meta::r(cfg.token_program),
            Meta::r(SYSTEM_PROGRAM),
        ],
        data,
    }
}

/// Build and sign the bridge `deposit` locking `amount_spl` for `agent_recipient`:
/// (signed legacy tx, the deposit nonce it takes). Checks the SPL balance first.
pub fn sign_deposit(rpc: &SolRpc, key: &SigningKey, amount_spl: u64, agent_recipient: &Pubkey) -> Result<(Vec<u8>, u64)> {
    let cfg = BridgeConfig::parse(&rpc.account(&bridge_config_pda())?.context("bridge is not initialised on this cluster")?)?;
    let me = key.verifying_key().to_bytes();
    let have = rpc.token_balance(&ata(&me, &cfg.mint, &cfg.token_program))?.unwrap_or(0);
    ensure!(have >= amount_spl, "SPL balance {} is below {}", have, amount_spl);
    let ix = deposit_ix(&cfg, &me, amount_spl, agent_recipient);
    let tx = legacy_tx(key, &[ix], &rpc.latest_blockhash()?)?;
    Ok((tx, cfg.deposit_nonce))
}

/// Lock `amount_spl` in the bridge vault for `agent_recipient`; returns
/// (signature, deposit nonce, slot).
#[allow(dead_code)]
pub fn bridge_deposit(rpc: &SolRpc, key: &SigningKey, amount_spl: u64, agent_recipient: &Pubkey) -> Result<(String, u64, u64)> {
    let (tx, nonce) = sign_deposit(rpc, key, amount_spl, agent_recipient)?;
    let sig = rpc.send(&tx)?;
    let slot = rpc.confirm(&sig, Duration::from_secs(60))?;
    Ok((sig, nonce, slot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pda_and_ata_known_values() {
        // Vectors from @solana/web3.js findProgramAddressSync (scripts/demo/node_modules).
        let (a, bump) = find_pda(&[b"config"], &bridge_program());
        assert_eq!((b58(&a).as_str(), bump), ("3bFavSyLdta8R3fqG156PReV7J8xqA95oELVdRvjpQvf", 255));
        assert_eq!(b58(&processed_pda(CHAIN_AGENT, 3)), "B7axfUiHmBbsDsixZGCLUYDX354P1fSZ1HRFPbQm3LK7");
        let owner = pk("4Nd1mBQtrMJVYVfKf2PJy9NZUZdTAsp7D4xWLs4gDB4T");
        let usdc = pk("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
        let spl = pk("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
        assert_eq!(b58(&ata(&owner, &usdc, &spl)), "F8biqkCRK2tHR6EncrcXDGgVTkGRrtojqyW39w41Qspn");
    }

    #[test]
    fn shortvec_encoding() {
        for (n, want) in [(0usize, vec![0u8]), (127, vec![0x7f]), (128, vec![0x80, 0x01]), (16383, vec![0xff, 0x7f]), (16384, vec![0x80, 0x80, 0x01])] {
            let mut v = vec![];
            shortvec(n, &mut v);
            assert_eq!(v, want, "{n}");
        }
    }

    #[test]
    fn legacy_tx_layout() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let me = key.verifying_key().to_bytes();
        let cfg = BridgeConfig {
            admin: [1; 32],
            mint: [2; 32],
            vault: [3; 32],
            token_program: [4; 32],
            deposit_nonce: 5,
            threshold: 1,
            signers: vec![],
            release_modes: 3,
            light_client: [0; 32],
        };
        let tx = legacy_tx(&key, &[deposit_ix(&cfg, &me, 1000, &[9; 32])], &[8; 32]).unwrap();
        assert_eq!(tx[0], 1); // one signature
        let msg = &tx[65..];
        // 1 signer (payer = sender), 0 read-only signed, read-only unsigned: mint,
        // token program, system program, bridge program.
        assert_eq!(&msg[..3], &[1, 0, 4]);
        let nkeys = msg[3] as usize;
        assert_eq!(nkeys, 9); // payer, config, ata, vault, deposit, mint, token, system, bridge
        assert_eq!(&msg[4..36], &me);
        let sig = ed25519_dalek::Signature::from_bytes(tx[1..65].try_into().unwrap());
        key.verifying_key().verify_strict(msg, &sig).unwrap();
        // instruction data: disc || amount || recipient
        assert!(msg.windows(8).any(|w| w == anchor_disc("deposit")));
    }

    #[test]
    fn bridge_config_roundtrip() {
        let mut d = vec![0u8; 8];
        for b in [1u8, 2, 3, 4] {
            d.extend_from_slice(&[b; 32]);
        }
        d.extend_from_slice(&7u64.to_le_bytes());
        d.extend_from_slice(&[1, 254]);
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&[5; 32]);
        d.push(2);
        d.extend_from_slice(&[6; 32]);
        let c = BridgeConfig::parse(&d).unwrap();
        assert_eq!((c.mint, c.vault, c.deposit_nonce, c.signers.len(), c.release_modes), ([2; 32], [3; 32], 7, 1, 2));
        assert!(BridgeConfig::parse(&d[..100]).is_err());
    }
}
