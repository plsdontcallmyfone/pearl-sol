//! agent-node: run a node, mine fallback blocks over RPC, generate tx load, make keys.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use agent_node::config::{parse_key, GenesisFile, NodeFile};
use agent_node::node::{run, ActorConfig, PeerCount};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use commonware_cryptography::{ed25519, Signer as _};
use commonware_runtime::{tokio as cw_tokio, Runner as _, Supervisor as _};
use consensus_pow::{Chain, MockVerifier, PearlVerifier, PowVerifier};

#[derive(Parser)]
#[command(name = "agent-node", about = "pearl-sol agent-chain node")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a node from a node.json.
    Run {
        #[arg(long)]
        config: PathBuf,
        /// Network mode, overrides node.json `net`: static (discovery over `peers`) or open.
        #[arg(long)]
        net: Option<String>,
        /// Open mode seeds, comma separated `host:port` or `key@host:port` (overrides `seeds`).
        #[arg(long, value_delimiter = ',')]
        seeds: Option<Vec<String>>,
        /// Override node.json p2p_listen.
        #[arg(long)]
        p2p_listen: Option<String>,
        /// Open mode: address to advertise (overrides p2p_external).
        #[arg(long)]
        p2p_external: Option<String>,
    },
    /// Mine fallback blocks against a node's RPC.
    Mine {
        #[arg(long, default_value = "http://127.0.0.1:10630")]
        rpc: String,
        /// Payout key (base58 or hex).
        #[arg(long)]
        miner: String,
        /// Blocks to mine (0 = forever).
        #[arg(long, default_value_t = 0)]
        blocks: u64,
        #[arg(long, default_value = "fallback")]
        job_mode: String,
    },
    /// Fund sender accounts via attested unlocks and send transfers.
    Load {
        #[arg(long, default_value = "http://127.0.0.1:10630")]
        rpc: String,
        /// Attester seed (hex, 32 bytes).
        #[arg(long)]
        attester_seed: String,
        #[arg(long, default_value_t = 4)]
        senders: u8,
        #[arg(long, default_value_t = 50)]
        per_sender: u64,
        #[arg(long, default_value_t = 1_000_000)]
        base_nonce: u64,
    },
    /// Print the public key (base58) of a 32-byte hex seed.
    Pubkey {
        seed: String,
    },
}

fn seed32(s: &str) -> Result<[u8; 32]> {
    hex::decode(s.trim().trim_start_matches("0x"))?.try_into().map_err(|_| anyhow::anyhow!("seed must be 32 bytes hex"))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    match Cli::parse().cmd {
        Cmd::Run { config, net, seeds, p2p_listen, p2p_external } => run_node(config, net, seeds, p2p_listen, p2p_external),
        Cmd::Mine { rpc, miner, blocks, job_mode } => {
            let (n, secs) = agent_node::miner::mine(&rpc, parse_key(&miner)?, blocks, &job_mode)?;
            println!("mined {n} blocks in {secs:.2} s ({:.2} blocks/s)", n as f64 / secs.max(1e-9));
            Ok(())
        }
        Cmd::Load { rpc, attester_seed, senders, per_sender, base_nonce } => {
            let t = std::time::Instant::now();
            let h = agent_node::miner::load(&rpc, seed32(&attester_seed)?, senders, per_sender, base_nonce)?;
            println!("sent {} txs in {:.2} s", h.len(), t.elapsed().as_secs_f64());
            Ok(())
        }
        Cmd::Pubkey { seed } => {
            println!("{}", bs58::encode(agent_types::tx::pubkey_from_seed(&seed32(&seed)?)).into_string());
            Ok(())
        }
    }
}

enum NetMode {
    Static { boots: Vec<(ed25519::PublicKey, SocketAddr)>, peers: Vec<ed25519::PublicKey> },
    Open(Box<NodeFile>),
}

fn run_node(
    path: PathBuf,
    net: Option<String>,
    seeds: Option<Vec<String>>,
    p2p_listen: Option<String>,
    p2p_external: Option<String>,
) -> Result<()> {
    let mut nf = NodeFile::load(&path)?;
    if let Some(n) = net {
        nf.net = n;
    }
    if let Some(s) = seeds {
        nf.seeds = s.into_iter().filter(|s| !s.trim().is_empty()).collect();
    }
    if let Some(l) = p2p_listen {
        nf.p2p_listen = l;
    }
    if p2p_external.is_some() {
        nf.p2p_external = p2p_external;
    }
    let gf = GenesisFile::load(&PathBuf::from(&nf.genesis))?;
    let genesis = gf.to_genesis()?;
    let signer = agent_types::tx::private_key(&seed32(&nf.p2p_key)?);
    let me = signer.public_key();
    let listen: SocketAddr = nf.p2p_listen.parse().context("p2p_listen")?;
    let rpc_addr: SocketAddr = nf.rpc_listen.parse().context("rpc_listen")?;
    let ws_addr: SocketAddr = nf.ws_listen.parse().context("ws_listen")?;
    let mode = match nf.net.as_str() {
        "static" => {
            let mut peers = Vec::new();
            let mut boots = Vec::new();
            for p in &nf.peers {
                let k = parse_key(&p.key)?;
                let pk: ed25519::PublicKey = commonware_codec::ReadExt::read(&mut &k[..]).context("peer key")?;
                boots.push((pk.clone(), p.addr.parse().context("peer addr")?));
                peers.push(pk);
            }
            if !peers.contains(&me) {
                peers.push(me.clone());
            }
            NetMode::Static { boots, peers }
        }
        "open" => {
            // validate now (seeds resolve, limits parse) so a bad config fails before the runtime starts
            nf.open_config(signer.clone(), b"check")?;
            NetMode::Open(Box::new(nf.clone()))
        }
        other => anyhow::bail!("unknown net mode {other:?} (static or open)"),
    };
    let cfg = cw_tokio::Config::new().with_storage_directory(&nf.data_dir);
    let runner = cw_tokio::Runner::new(cfg);
    let use_mock = nf.verifier == "mock";
    let blob_cfg = nf.blobs.config()?;
    let tip_interval = Duration::from_millis(nf.tip_interval_ms);
    runner.start(|context| async move {
        let res = if use_mock {
            start(context, genesis, MockVerifier, signer, me, listen, mode, rpc_addr, ws_addr, tip_interval, blob_cfg).await
        } else {
            start(context, genesis, PearlVerifier, signer, me, listen, mode, rpc_addr, ws_addr, tip_interval, blob_cfg).await
        };
        if let Err(e) = res {
            eprintln!("node failed: {e:#}");
            std::process::exit(1);
        }
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn start<V: PowVerifier>(
    context: cw_tokio::Context,
    genesis: consensus_pow::Genesis,
    verifier: V,
    signer: ed25519::PrivateKey,
    me: ed25519::PublicKey,
    listen: SocketAddr,
    mode: NetMode,
    rpc_addr: SocketAddr,
    ws_addr: SocketAddr,
    tip_interval: Duration,
    blob_cfg: Option<blobs::BlobConfig>,
) -> Result<()> {
    let chain = Chain::open(context.child("chain"), "node", genesis, verifier).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let genesis_hash = chain.genesis_hash();
    tracing::info!(genesis = %hex::encode(genesis_hash), height = chain.tip_height(), "chain open");
    let height = chain.tip_height();
    let mut core = rpc::Core::new(chain);
    if let Some(bc) = blob_cfg {
        let store = blobs::BlobStore::open(context.child("blobs"), "node-blobs", bc, height).await.map_err(|e| anyhow::anyhow!("blob store: {e}"))?;
        tracing::info!(count = store.len(), bytes = store.total_bytes(), "blob store open");
        core = core.with_blobs(store);
    }
    let (tx, rx) = futures::channel::mpsc::channel(1024);
    let http = tokio::spawn(rpc::server::serve_http(rpc_addr, tx.clone()));
    let ws = tokio::spawn(rpc::server::serve_ws(ws_addr, tx));
    match mode {
        NetMode::Static { boots, peers } => {
            tracing::info!(%listen, peers = peers.len(), "p2p: static discovery");
            let started = net::start_discovery(context.child("p2p"), signer, listen, listen, boots, peers);
            let oracle = started.oracle;
            let cfg = ActorConfig { tip_interval, ..Default::default() };
            run(context.child("node"), core, me, oracle.clone(), oracle, started.channels, rx, cfg).await;
        }
        NetMode::Open(nf) => {
            let mut ocfg = nf.open_config(signer, &net::open_namespace(&genesis_hash))?;
            let addrs_file = PathBuf::from(&nf.data_dir).join("p2p-addrs.json");
            ocfg.known = agent_node::config::load_addrs(&addrs_file);
            tracing::info!(%listen, seeds = ?nf.seeds, max_outbound = ocfg.max_outbound, max_inbound = ocfg.max_inbound,
                allow_private = ocfg.allow_private, "p2p: open membership");
            let started = net::start_open(context.child("p2p"), context.child("transport"), ocfg);
            let oracle = started.oracle;
            let o2 = oracle.clone();
            let o4 = oracle.clone();
            let cfg = ActorConfig {
                tip_interval,
                peer_count: Some(PeerCount(std::sync::Arc::new(move || o2.connected_count()))),
                peer_info: Some(std::sync::Arc::new(move || agent_node::node::open_peers_json(&o4))),
            };
            let o3 = oracle.clone();
            let saver = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    agent_node::config::save_addrs(&addrs_file, &o3.known_addrs());
                }
            });
            run(context.child("node"), core, me, oracle.clone(), oracle, started.channels, rx, cfg).await;
            saver.abort();
        }
    }
    http.abort();
    ws.abort();
    Ok(())
}
