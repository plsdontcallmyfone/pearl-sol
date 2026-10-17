//! Proof-mode settlement (`run --settle`): what the M-of-N relayer does by signatures,
//! done by light-client proof once the light client holds the needed final header.
//!
//! - Withdrawals: for each final chain withdrawal not yet Processed on Solana, when the
//!   bridge has MODE_PROOF on, `bridge.release_proof` with the node's historical MMR
//!   proof (getWithdrawalProof `at_height` = the light client's final height) against
//!   that FINAL light-client header's `withdraw_root`, so the light client need not sit
//!   at the node's final tip. The header's block hash must equal the node's at that
//!   height. Fallback (an old node without `at_height`): the node's current-root proof
//!   against any final light-client header carrying that root.
//! - Orders: for each Pending storefront Order whose chain job is Completed / Expired,
//!   when the storefront has MODE_PROOF on and the closing block is final in the light
//!   client, `finalize_order_proof` (CompleteJob tx + tx_root path + ed25519 precompile
//!   over the evaluator's result signature) or `expire_order_proof`. The caller is the
//!   relay's payer, which must be the buyer, the storefront admin or an attester.

use anyhow::{anyhow, Context, Result};
use lightclient::client;
use pearl_verify_sbf as pv;
use serde_json::json;
use solana_client::rpc_config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_client::rpc_filter::{Memcmp, RpcFilterType};
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;

use crate::chain::{self, Node};
use crate::sol::Sol;

pub const BRIDGE_ID: Pubkey = solana_sdk::pubkey!("7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8");
pub const STOREFRONT_ID: Pubkey = solana_sdk::pubkey!("DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr");
const ATA_ID: Pubkey = solana_sdk::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const SYSTEM_ID: Pubkey = solana_sdk::pubkey!("11111111111111111111111111111111");
const IX_SYSVAR: Pubkey = solana_sdk::pubkey!("Sysvar1nstructions1111111111111111111111111");
pub const MODE_SIGNERS: u8 = 1;
pub const MODE_PROOF: u8 = 2;
const CHAIN_AGENT: u8 = 1;
/// storefront::layout
const ORDER_LEN: u64 = 1070;
const ORDER_STATUS: usize = 9;
const ORDER_NONCE: usize = 12;
const ORDER_JOB_ID: usize = 20;
const STATUS_PENDING: u8 = 1;

fn disc(name: &str) -> [u8; 8] {
    chain::sha256(&[format!("global:{name}").as_bytes()])[..8].try_into().unwrap()
}

fn vecu8(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

pub fn bridge_config_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"config"], &BRIDGE_ID).0
}
pub fn processed_pda(nonce: u64) -> Pubkey {
    Pubkey::find_program_address(&[b"processed", &[CHAIN_AGENT], &nonce.to_le_bytes()], &BRIDGE_ID).0
}
pub fn sf_config_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"config"], &STOREFRONT_ID).0
}
pub fn order_pda(nonce: u64) -> Pubkey {
    Pubkey::find_program_address(&[b"order", &nonce.to_le_bytes()], &STOREFRONT_ID).0
}
fn ata(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[owner.as_ref(), token_program.as_ref(), mint.as_ref()], &ATA_ID).0
}

/// A modes byte appended in M2 (MODE_SIGNERS if missing or 0).
fn modes_at(d: &[u8], at: usize) -> u8 {
    match d.get(at) {
        Some(&m) if m != 0 => m,
        _ => MODE_SIGNERS,
    }
}

pub struct BridgeCfg {
    pub mint: Pubkey,
    pub vault: Pubkey,
    pub token_program: Pubkey,
    pub release_modes: u8,
}

pub fn bridge_cfg(d: &[u8]) -> Result<BridgeCfg> {
    let k = |o: usize| Pubkey::new_from_array(d[o..o + 32].try_into().unwrap());
    let n = u32::from_le_bytes(d[146..150].try_into()?) as usize;
    Ok(BridgeCfg { mint: k(40), vault: k(72), token_program: k(104), release_modes: modes_at(d, 150 + 32 * n) })
}

/// Storefront Config.finalize_modes (offsets as in services/relayer decodeStorefrontConfig).
pub fn sf_finalize_modes(d: &[u8]) -> Result<u8> {
    let at = 8 + 96 + 1 + 2 + 2 + 8 + 8 + 4 + 4 + 8;
    let n = u32::from_le_bytes(d.get(at + 1..at + 5).context("storefront config")?.try_into()?) as usize;
    Ok(modes_at(d, at + 1 + 4 + 32 * n + 24 + 2))
}

pub fn release_proof_ix(payer: Pubkey, b: &BridgeCfg, nonce: u64, entry: &[u8], location: u64, proof: &[u8], lc_header: Pubkey, recipient_token: Pubkey) -> Instruction {
    let mut data = disc("release_proof").to_vec();
    data.extend_from_slice(&nonce.to_le_bytes());
    vecu8(&mut data, entry);
    data.extend_from_slice(&location.to_le_bytes());
    vecu8(&mut data, proof);
    Instruction {
        program_id: BRIDGE_ID,
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new_readonly(bridge_config_pda(), false),
            AccountMeta::new_readonly(b.mint, false),
            AccountMeta::new(b.vault, false),
            AccountMeta::new(recipient_token, false),
            AccountMeta::new(processed_pda(nonce), false),
            AccountMeta::new_readonly(lc_header, false),
            AccountMeta::new_readonly(b.token_program, false),
            AccountMeta::new_readonly(SYSTEM_ID, false),
        ],
        data,
    }
}

fn create_ata_idempotent(payer: Pubkey, owner: Pubkey, mint: Pubkey, token_program: Pubkey) -> Instruction {
    Instruction {
        program_id: ATA_ID,
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(ata(&owner, &mint, &token_program), false),
            AccountMeta::new_readonly(owner, false),
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new_readonly(SYSTEM_ID, false),
            AccountMeta::new_readonly(token_program, false),
        ],
        data: vec![1],
    }
}

fn close_order_proof_metas(caller: Pubkey, nonce: u64, lc_header: Pubkey) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(caller, true),
        AccountMeta::new_readonly(sf_config_pda(), false),
        AccountMeta::new(order_pda(nonce), false),
        AccountMeta::new_readonly(IX_SYSVAR, false),
        AccountMeta::new_readonly(lc_header, false),
    ]
}

#[allow(clippy::too_many_arguments)]
pub fn finalize_order_proof_ix(caller: Pubkey, nonce: u64, lc_header: Pubkey, deliverer: [u8; 32], result_hash: [u8; 32], result_uri: &str, tx: &[u8], path: &[u8], pair: u8) -> Instruction {
    let mut data = disc("finalize_order_proof").to_vec();
    data.extend_from_slice(&deliverer);
    data.extend_from_slice(&result_hash);
    vecu8(&mut data, result_uri.as_bytes());
    vecu8(&mut data, tx);
    vecu8(&mut data, path);
    data.push(pair);
    Instruction { program_id: STOREFRONT_ID, accounts: close_order_proof_metas(caller, nonce, lc_header), data }
}

pub fn expire_order_proof_ix(caller: Pubkey, nonce: u64, lc_header: Pubkey, tx: &[u8], path: &[u8]) -> Instruction {
    let mut data = disc("expire_order_proof").to_vec();
    vecu8(&mut data, tx);
    vecu8(&mut data, path);
    Instruction { program_id: STOREFRONT_ID, accounts: close_order_proof_metas(caller, nonce, lc_header), data }
}

/// `b"pearl-sol:result:v1" || job_id || deliverer || result_hash` (storefront result_message).
pub fn result_message(job_id: &[u8; 32], deliverer: &[u8; 32], result_hash: &[u8; 32]) -> Vec<u8> {
    [b"pearl-sol:result:v1".as_slice(), job_id, deliverer, result_hash].concat()
}

pub struct Settle {
    /// Withdrawals below this nonce are known released.
    next_withdrawal: u64,
}

impl Settle {
    pub fn new() -> Self {
        Self { next_withdrawal: 0 }
    }

    /// A FINAL light-client header at height in [lo, final_height] whose header bytes
    /// satisfy `ok`, newest first. Returns its account address and height.
    fn final_header_where(sol: &Sol, lo: u64, ok: impl Fn(u64, &[u8; 256]) -> bool) -> Result<Option<(Pubkey, u64)>> {
        let cfg = sol.config()?;
        let canon = sol.canon()?;
        let floor = cfg.final_height.saturating_sub(lightclient::CANON_LEN as u64 - 1).max(lo);
        let mut h = cfg.final_height;
        while h >= floor && h > 0 {
            let hash = canon[(h as usize) % lightclient::CANON_LEN];
            let addr = client::header_addr(&hash);
            if let Some(d) = sol.account(&addr)? {
                if let Some((_, hdr)) = lightclient::read_final_header(&d) {
                    if ok(h, &hdr) {
                        return Ok(Some((addr, h)));
                    }
                }
            }
            h -= 1;
        }
        Ok(None)
    }

    pub fn tick(&mut self, node: &Node, sol: &mut Sol) -> Result<()> {
        self.withdrawals(node, sol)?;
        self.orders(node, sol)
    }

    fn withdrawals(&mut self, node: &Node, sol: &mut Sol) -> Result<()> {
        let Some(cd) = sol.account(&bridge_config_pda())? else { return Ok(()) };
        let b = bridge_cfg(&cd)?;
        if b.release_modes & MODE_PROOF == 0 {
            return Ok(());
        }
        let ev = node.rpc.call("getBridgeEvents", json!({"from_nonce": self.next_withdrawal, "limit": 100}))?;
        let mut all_done = true;
        for w in ev["withdrawals"].as_array().cloned().unwrap_or_default() {
            let n = chain::num(&w["nonce"])?;
            if sol.account(&processed_pda(n))?.is_some() {
                if all_done {
                    self.next_withdrawal = n + 1;
                }
                continue;
            }
            all_done = false;
            if w["final"] != true {
                continue;
            }
            let wh = chain::num(&w["height"])?;
            let lc_final = sol.config()?.final_height;
            if lc_final < wh {
                continue; // the withdrawal's block is not final in the light client yet
            }
            // Prove against the header the light client actually has final.
            let (p, lc_header, at) = match node.withdrawal_proof_at(n, lc_final) {
                Ok(p) if p["final"] == true => {
                    let root: [u8; 32] = hex::decode(p["withdraw_root"].as_str().context("withdraw_root")?)?.try_into().map_err(|_| anyhow!("root"))?;
                    let bh: [u8; 32] = hex::decode(p["root_block_hash"].as_str().context("root_block_hash")?)?.try_into().map_err(|_| anyhow!("hash"))?;
                    let found = Self::final_header_where(sol, lc_final, |h, hdr| {
                        h == lc_final && pv::blake3::hash(hdr) == bh && pv::header::Header(hdr).withdraw_root() == root
                    })?;
                    let Some((lc_header, at)) = found else {
                        eprintln!("withdrawal {n}: light-client final header {lc_final} is not the node's (fork?)");
                        continue;
                    };
                    (p, lc_header, at)
                }
                Ok(_) => continue,
                Err(e) => {
                    // Older node without at_height: current-root proof, any final header with that root.
                    eprintln!("withdrawal {n}: historical proof at {lc_final} refused ({e:#}); trying the current root");
                    let p = node.withdrawal_proof(n)?;
                    if p["final"] != true {
                        continue;
                    }
                    let root: [u8; 32] = hex::decode(p["withdraw_root"].as_str().context("withdraw_root")?)?.try_into().map_err(|_| anyhow!("root"))?;
                    let Some((lc_header, at)) = Self::final_header_where(sol, wh, |_, h| pv::header::Header(h).withdraw_root() == root)? else {
                        continue;
                    };
                    (p, lc_header, at)
                }
            };
            let entry = hex::decode(p["entry"].as_str().context("entry")?)?;
            let recipient = Pubkey::new_from_array(entry[40..72].try_into()?);
            let proof = chain::b64d(p["proof"].as_str().context("proof")?)?;
            let loc = chain::num(&p["location"])?;
            let pk = sol.pk();
            let ixs = [
                create_ata_idempotent(pk, recipient, b.mint, b.token_program),
                release_proof_ix(pk, &b, n, &entry, loc, &proof, lc_header, ata(&recipient, &b.mint, &b.token_program)),
            ];
            match sol.send(&ixs, &[]) {
                Ok(sig) => println!("released withdrawal {n} by proof against final header {at} ({sig})"),
                Err(e) => eprintln!("release_proof {n}: {e:#}"),
            }
        }
        Ok(())
    }

    fn orders(&mut self, node: &Node, sol: &mut Sol) -> Result<()> {
        let Some(cd) = sol.account(&sf_config_pda())? else { return Ok(()) };
        if sf_finalize_modes(&cd)? & MODE_PROOF == 0 {
            return Ok(());
        }
        let cfg = RpcProgramAccountsConfig {
            filters: Some(vec![RpcFilterType::DataSize(ORDER_LEN), RpcFilterType::Memcmp(Memcmp::new_raw_bytes(ORDER_STATUS, vec![STATUS_PENDING]))]),
            account_config: RpcAccountInfoConfig { encoding: Some(solana_account_decoder_client_types::UiAccountEncoding::Base64), ..Default::default() },
            ..Default::default()
        };
        let accs = sol.rpc.get_program_accounts_with_config(&STOREFRONT_ID, cfg)?;
        let lc_final = sol.config()?.final_height;
        for (_, a) in accs {
            let d = &a.data;
            let nonce = u64::from_le_bytes(d[ORDER_NONCE..ORDER_NONCE + 8].try_into()?);
            let job_id: [u8; 32] = d[ORDER_JOB_ID..ORDER_JOB_ID + 32].try_into()?;
            let job = node.rpc.call("getJob", json!({"job_id": hex::encode(job_id)}))?;
            let status = job["status"].as_str().unwrap_or("");
            if status != "completed" && status != "expired" {
                continue;
            }
            let closed = chain::num(&job["closed_height"])?;
            if closed == 0 || closed > lc_final {
                continue; // closing block not final in the light client yet
            }
            let Some(hd) = node.header_at(closed)? else { continue };
            let Some((lc_header, _)) = Self::final_header_where(sol, closed, |h, b| h == closed && pv::blake3::hash(b) == hd.hash)? else {
                continue;
            };
            let txs = node.block_txs(&hd.hash)?;
            let (_, paths) = chain::tx_root_paths(&txs);
            let pk = sol.pk();
            let mut done = false;
            for (i, t) in txs.iter().enumerate() {
                let Ok(v) = pv::chaintx::TxView::any_chain(t) else { continue };
                let ixs = if status == "completed" {
                    let Ok((jid, off, count)) = pv::chaintx::complete_job(&v) else { continue };
                    if jid != job_id || count == 0 {
                        continue;
                    }
                    let deliverer: [u8; 32] = bs58::decode(job["deliverer"].as_str().context("deliverer")?).into_vec()?.try_into().map_err(|_| anyhow!("deliverer"))?;
                    let rh: [u8; 32] = hex::decode(job["result_hash"].as_str().context("result_hash")?)?.try_into().map_err(|_| anyhow!("result_hash"))?;
                    let epk: [u8; 32] = t[off..off + 32].try_into()?;
                    let esig: [u8; 64] = t[off + 32..off + 96].try_into()?;
                    let msg = result_message(&job_id, &deliverer, &rh);
                    vec![
                        solana_sdk::ed25519_instruction::new_ed25519_instruction_with_signature(&msg, &esig, &epk),
                        finalize_order_proof_ix(pk, nonce, lc_header, deliverer, rh, job["result_uri"].as_str().unwrap_or(""), t, &paths[i], 0),
                    ]
                } else {
                    match pv::chaintx::expire_job(&v) {
                        Ok(jid) if jid == job_id => vec![expire_order_proof_ix(pk, nonce, lc_header, t, &paths[i])],
                        _ => continue,
                    }
                };
                match sol.send(&ixs, &[]) {
                    Ok(sig) => println!("{} order {nonce} by proof against final header {closed} ({sig})", if status == "completed" { "finalized" } else { "expired" }),
                    Err(e) => eprintln!("order {nonce} proof: {e:#}"),
                }
                done = true;
                break;
            }
            if !done {
                eprintln!("order {nonce}: no {status} tx for its job in block {closed}");
            }
        }
        Ok(())
    }
}
