//! Session context: endpoints, keys, and the miner child this session started.
//! Keys are read from files given by flag or env and never printed or written.

use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use clap::Args;
use ed25519_dalek::SigningKey;
use serde_json::Value;

use pearl_node::{read_key_file, HttpNode};

use crate::solana::{Pubkey, SolRpc};
use crate::term::Ui;

#[derive(Args, Debug, Clone)]
pub struct Opts {
    /// Agent-chain node JSON-RPC.
    #[arg(long, env = "PEARL_RPC", default_value = "http://127.0.0.1:10630", global = true)]
    pub rpc: String,
    /// Token-chain (Solana) JSON-RPC.
    #[arg(long, env = "PEARL_SOL_RPC", default_value = "http://127.0.0.1:10600", global = true)]
    pub sol_rpc: String,
    /// Agent-chain key file (Solana JSON array, or base58 / hex). Never printed.
    #[arg(long, env = "PEARL_KEY", global = true)]
    pub key: Option<PathBuf>,
    /// Solana key file for the SPL side (bridge in, SPL balance). Defaults to --key
    /// (one ed25519 key is a valid address on both chains).
    #[arg(long, env = "PEARL_SOL_KEY", global = true)]
    pub sol_key: Option<PathBuf>,
    /// Watch this agent-chain address read-only (no key needed).
    #[arg(long, env = "PEARL_ADDRESS", global = true)]
    pub address: Option<String>,
    /// Mining pool (services/pool) HTTP endpoint, read for /Mining.
    #[arg(long, env = "PEARL_POOL", default_value = "http://127.0.0.1:10663", global = true)]
    pub pool: String,
    /// Agent Credit HTTP endpoint (GET returns 2xx when up). Unset = not connected.
    #[arg(long, env = "PEARL_CREDIT_URL", global = true)]
    pub credit_url: Option<String>,
    /// pearl-miner binary for `mine start` (default: crates/target/release/pearl-miner).
    #[arg(long, env = "PEARL_MINER_BIN", global = true)]
    pub miner_bin: Option<PathBuf>,
    /// Print JSON instead of text (plain mode).
    #[arg(long, global = true)]
    pub json: bool,
    /// Wait for the effect: tx included (and for bridge in, unlocked on the chain).
    #[arg(long, global = true)]
    pub wait: bool,
    /// Text mode: return once the tx is submitted instead of following it to the end
    /// (inclusion, K-deep finality, unlock / release, job completion).
    #[arg(long, global = true)]
    pub no_wait: bool,
}

pub struct Ctx {
    pub opts: Opts,
    pub node: HttpNode,
    pub sol: SolRpc,
    key: Option<SigningKey>,
    sol_key: Option<SigningKey>,
    pub me: Option<Pubkey>,
    pub sol_me: Option<Pubkey>,
    /// A pearl-miner this session spawned (`mine start`); only this one is ever stopped.
    pub miner: Mutex<Option<Child>>,
    /// Where steps go: silent for `--json` and the dashboard, the terminal in text mode.
    pub ui: Ui,
}

impl Ctx {
    pub fn new(opts: Opts) -> Result<Self> {
        let key = opts.key.as_deref().map(read_key_file).transpose()?;
        let sol_key = match opts.sol_key.as_deref() {
            Some(p) => Some(read_key_file(p)?),
            None => key.clone(),
        };
        let me = match (&key, &opts.address) {
            (Some(k), _) => Some(k.verifying_key().to_bytes()),
            (None, Some(a)) => Some(pearl_job::spec::parse_key32(a)?),
            (None, None) => None,
        };
        let sol_me = sol_key.as_ref().map(|k| k.verifying_key().to_bytes()).or(if key.is_none() { me } else { None });
        Ok(Self {
            node: HttpNode::new(&opts.rpc),
            sol: SolRpc::new(&opts.sol_rpc),
            opts,
            key,
            sol_key,
            me,
            sol_me,
            miner: Mutex::new(None),
            ui: Ui::silent(),
        })
    }

    pub fn key(&self) -> Result<&SigningKey> {
        self.key.as_ref().ok_or_else(|| anyhow!("no key: pass --key FILE or set PEARL_KEY (keys/localnet/ has throwaway localnet keys)"))
    }

    pub fn sol_key(&self) -> Result<&SigningKey> {
        self.sol_key.as_ref().ok_or_else(|| anyhow!("no Solana key: pass --sol-key FILE (or --key)"))
    }

    /// Text mode follows a write to its end (inclusion, finality, ...) unless --no-wait.
    pub fn follow(&self) -> bool {
        self.ui.is_plain() && !self.opts.no_wait
    }

    /// Wait for inclusion: --wait (any mode) or text mode without --no-wait.
    pub fn wait_inclusion(&self) -> bool {
        self.opts.wait || self.follow()
    }

    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    /// Agent-chain JSON-RPC call.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.node.call(method, params)
    }

    /// PID of the miner this session started, if still running.
    pub fn miner_pid(&self) -> Option<u32> {
        let mut g = self.miner.lock().ok()?;
        let child = g.as_mut()?;
        match child.try_wait() {
            Ok(None) => Some(child.id()),
            _ => {
                *g = None;
                None
            }
        }
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        // The miner is a child of this session: stop it with the session.
        if let Ok(mut g) = self.miner.lock() {
            if let Some(mut c) = g.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}
