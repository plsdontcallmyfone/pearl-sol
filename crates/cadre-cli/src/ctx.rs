//! Session context: endpoints, keys, and the miner child this session started.
//! Keys are read from files given by flag or env and never printed or written.

use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use clap::Args;
use ed25519_dalek::SigningKey;
use serde_json::Value;

use cadre_node::{read_key_file, HttpNode};

use crate::solana::{Pubkey, SolRpc};
pub use crate::solana::Network;
use crate::term::Ui;

#[derive(Args, Debug, Clone)]
pub struct Opts {
    /// Network profile: endpoint defaults and Solana program ids (docs/DEVNET.md).
    /// `devnet` = the public Cadre devnet, `testbox` = the demo stack on the test box (run
    /// there), `localnet` (or `local`) = scripts/demo.sh on this machine.
    #[arg(long, env = "CADRE_NETWORK", value_enum, default_value_t = Network::Devnet, global = true)]
    pub network: Network,
    /// Agent-chain node JSON-RPC (default: the network's).
    #[arg(long, env = "CADRE_RPC", global = true)]
    pub rpc: Option<String>,
    /// Token-chain (Solana) JSON-RPC (default: the network's).
    #[arg(long, env = "CADRE_SOL_RPC", global = true)]
    pub sol_rpc: Option<String>,
    /// Agent-chain key file (Solana JSON array, or base58 / hex). Never printed.
    #[arg(long, env = "CADRE_KEY", global = true)]
    pub key: Option<PathBuf>,
    /// Solana key file for the SPL side (bridge in, SPL balance). Defaults to --key
    /// (one ed25519 key is a valid address on both chains).
    #[arg(long, env = "CADRE_SOL_KEY", global = true)]
    pub sol_key: Option<PathBuf>,
    /// Watch this agent-chain address read-only (no key needed).
    #[arg(long, env = "CADRE_ADDRESS", global = true)]
    pub address: Option<String>,
    /// Mining pool (services/pool) HTTP endpoint, read for /Mining.
    /// Default: localnet :10663; the public devnet runs no pool.
    #[arg(long, env = "CADRE_POOL", global = true)]
    pub pool: Option<String>,
    /// Agent Credit HTTP endpoint (GET returns 2xx when up). Unset = not connected.
    #[arg(long, env = "CADRE_CREDIT_URL", global = true)]
    pub credit_url: Option<String>,
    /// The Cadre miner binary for `mine start` (default: crates/target/release/pearl-miner).
    #[arg(long, env = "CADRE_MINER_BIN", global = true)]
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
    /// Override one endpoint: NAME=URL (repeatable; `cadre ping --list` shows the names),
    /// or NAME=none to leave it out. Env: CADRE_<NAME>, e.g. CADRE_API.
    #[arg(long, global = true, value_parser = crate::endpoints::parse_endpoint)]
    pub endpoint: Vec<String>,
    /// Also check the 127.0.0.1 services of the box this runs on (gateway, worker,
    /// attester, swarm, desktops, user-agent runtime): for devnet box 0 and the test box.
    #[arg(long, global = true)]
    pub local: bool,
    /// `status` / `ping`: exit 1 on a warning too (default: only on a failure).
    #[arg(long, global = true)]
    pub strict: bool,
}

pub struct Ctx {
    pub opts: Opts,
    /// Resolved endpoints (flag, env, else the network profile).
    pub rpc: String,
    pub sol_rpc: String,
    pub pool: Option<String>,
    /// Every endpoint `status`, `ping` and the read commands use (endpoints.rs).
    pub eps: crate::endpoints::Endpoints,
    pub node: HttpNode,
    pub sol: SolRpc,
    key: Option<SigningKey>,
    sol_key: Option<SigningKey>,
    pub me: Option<Pubkey>,
    pub sol_me: Option<Pubkey>,
    /// A miner this session spawned (`mine start`); only this one is ever stopped.
    pub miner: Mutex<Option<Child>>,
    /// Where steps go: silent for `--json` and the dashboard, the terminal in text mode.
    pub ui: Ui,
}

impl Ctx {
    pub fn new(mut opts: Opts) -> Result<Self> {
        // no --key / --address (or env): the wallet saved by `cadre wallet use`
        if opts.key.is_none() && opts.address.is_none() {
            if let Some(s) = crate::wallet_cfg::load() {
                opts.key = s.key;
                opts.address = s.address;
            }
        }
        let key = opts.key.as_deref().map(read_key_file).transpose()?;
        let sol_key = match opts.sol_key.as_deref() {
            Some(p) => Some(read_key_file(p)?),
            None => key.clone(),
        };
        let me = match (&key, &opts.address) {
            (Some(k), _) => Some(k.verifying_key().to_bytes()),
            (None, Some(a)) => Some(crate::fmt::parse_chain_addr(a)?),
            (None, None) => None,
        };
        let sol_me = sol_key.as_ref().map(|k| k.verifying_key().to_bytes()).or(if key.is_none() { me } else { None });
        crate::solana::set_network(opts.network);
        let p = opts.network.profile();
        let rpc = opts.rpc.clone().unwrap_or_else(|| p.rpc.into());
        let sol_rpc = opts.sol_rpc.clone().unwrap_or_else(|| p.sol_rpc.into());
        let pool = opts.pool.clone().or(p.pool.map(String::from));
        let eps = crate::endpoints::Endpoints::resolve(&opts, &rpc, &sol_rpc, pool.as_deref());
        // the Solana endpoint after overrides (a dedicated RPC on a box); sol_rpc is what may be printed
        let sol = SolRpc::new(&eps.solana);
        let sol_rpc = eps.solana_shown.clone();
        Ok(Self {
            eps,
            node: HttpNode::new(&rpc),
            sol,
            rpc,
            sol_rpc,
            pool,
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
        self.key.as_ref().ok_or_else(|| anyhow!("no key: run `cadre wallet use --key FILE` once (or pass --key FILE / set CADRE_KEY)"))
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
