//! Node API state: the chain, the mempool, block templates and subscribers.
//!
//! The node actor owns one [`Core`] and calls it for RPC requests, gossip and sync
//! deliveries. Side effects for the network are queued in [`Core::outbox`].

use std::collections::{HashMap, VecDeque};

use agent_types::{Block, Hash, Pubkey, Tx};
use commonware_runtime::Spawner;
use consensus_pow::{Accept, Chain, ChainError, ChainEvent, JobChoice, PowVerifier, Template};
use mempool::Mempool;
use serde_json::{json, Value};
use state::schema::{self, Account};
use tracing::{debug, info};

/// Something the actor must send to peers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outbound {
    Block(Vec<u8>),
    Tx(Vec<u8>),
    /// Ask the sync layer for this header (headers-first backfill).
    FetchHeader(Hash),
    FetchBody(Hash),
}

/// WebSocket subscriber: topics and a channel of JSON notification strings.
pub struct Subscriber {
    pub topics: Vec<String>,
    pub tx: futures::channel::mpsc::UnboundedSender<String>,
}

pub struct Core<E: commonware_storage::Context + Spawner, V: PowVerifier> {
    pub chain: Chain<E, V>,
    pub mempool: Mempool,
    pub templates: VecDeque<(String, Template)>,
    pub outbox: Vec<Outbound>,
    pub subscribers: Vec<Subscriber>,
    pub peers: usize,
    pub syncing: bool,
    /// Counters for throughput reporting.
    pub blocks_accepted: u64,
    pub txs_accepted: u64,
}

pub const MAX_TEMPLATES: usize = 64;

#[derive(Debug, Clone)]
pub struct SubmitResult {
    pub accepted: bool,
    pub hash: Hash,
    pub height: u64,
    pub tip: bool,
    pub reason: Option<String>,
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    pub fn new(chain: Chain<E, V>) -> Self {
        let chain_id = chain.chain_id();
        Self {
            chain,
            mempool: Mempool::new(chain_id, 50_000),
            templates: VecDeque::new(),
            outbox: Vec::new(),
            subscribers: Vec::new(),
            peers: 0,
            syncing: false,
            blocks_accepted: 0,
            txs_accepted: 0,
        }
    }

    pub async fn account(&self, a: &Pubkey) -> Result<Account, ChainError> {
        Ok(match self.chain.read_tip(&schema::account_key(a)).await? {
            Some(b) => Account::decode(&b).map_err(|e| ChainError::Internal(e.to_string()))?,
            None => Account::default(),
        })
    }

    /// Admit a tx from RPC or gossip. `relay` queues it for peers.
    pub async fn add_tx(&mut self, tx: Tx, relay: bool) -> Result<Hash, String> {
        let acc = self.account(&tx.from).await.map_err(|e| e.to_string())?;
        let min_fee = self.chain.tip().config.min_fee;
        let bytes = tx.encode();
        let h = self.mempool.insert(tx, acc.nonce, min_fee).map_err(|e| e.to_string())?;
        self.txs_accepted += 1;
        if relay {
            self.outbox.push(Outbound::Tx(bytes));
        }
        self.notify("txs", json!({"tx_hash": hex::encode(h)}));
        Ok(h)
    }

    /// Accept a block from a miner (`relay` = gossip it) or a peer.
    pub async fn add_block(&mut self, block: Block, now_ms: u64, relay: bool) -> SubmitResult {
        let hash = block.hash();
        let height = block.header.height;
        let bytes = if relay { Some(block.encode()) } else { None };
        match self.chain.insert_block(block, now_ms).await {
            Ok((Accept::Added { tip }, events)) => {
                self.blocks_accepted += 1;
                if let Some(b) = bytes {
                    self.outbox.push(Outbound::Block(b));
                }
                self.on_events(events).await;
                SubmitResult { accepted: true, hash, height, tip, reason: None }
            }
            Ok((Accept::Known, _)) => SubmitResult { accepted: true, hash, height, tip: false, reason: Some("already known".into()) },
            Ok((Accept::Orphan { missing }, _)) => {
                self.outbox.push(Outbound::FetchHeader(missing));
                SubmitResult { accepted: true, hash, height, tip: false, reason: Some("orphan: parent unknown, fetching".into()) }
            }
            Err(e) => SubmitResult { accepted: false, hash, height, tip: false, reason: Some(e.to_string()) },
        }
    }

    /// Mempool and subscriber upkeep after chain events.
    pub async fn on_events(&mut self, events: Vec<ChainEvent>) {
        let mut tip_moved = false;
        for ev in events {
            match ev {
                ChainEvent::NewTip { hash, height, reorg_depth, disconnected, connected } => {
                    tip_moved = true;
                    for d in &disconnected {
                        if let Some(n) = self.chain.live_node(d) {
                            for t in n.txs.iter().cloned().collect::<Vec<_>>() {
                                self.mempool.reinsert(t);
                            }
                        }
                    }
                    for c in &connected {
                        if let Some(n) = self.chain.live_node(c) {
                            for t in n.txs.iter() {
                                self.mempool.remove(&t.hash());
                            }
                        }
                    }
                    let (job_id, miner, tx_count) = match self.chain.live_node(&hash) {
                        Some(n) => (n.header.job_id, n.header.miner, n.txs.len()),
                        None => ([0; 32], [0; 32], 0),
                    };
                    if reorg_depth > 0 {
                        info!(height, depth = reorg_depth, "tip moved to another branch");
                    } else {
                        debug!(height, "new tip");
                    }
                    self.notify(
                        "heads",
                        json!({"hash": hex::encode(hash), "height": height, "job_id": hex::encode(job_id),
                               "miner": bs58::encode(miner).into_string(), "tx_count": tx_count, "reorg_depth": reorg_depth}),
                    );
                }
                ChainEvent::Finalized { hash, height } => {
                    self.notify("finalized", json!({"hash": hex::encode(hash), "height": height}));
                }
                ChainEvent::WithdrawalFinal(w) => {
                    let v = crate::dispatch::withdrawal_json(&self.chain, &w);
                    self.notify("withdrawals", v);
                }
            }
        }
        if tip_moved {
            self.prune_mempool().await;
            self.templates.retain(|(_, t)| self.chain.live_node(&t.header.prev_hash).is_some());
        }
    }

    async fn prune_mempool(&mut self) {
        let mut nonces = HashMap::new();
        for s in self.mempool.senders() {
            if let Ok(a) = self.account(&s).await {
                nonces.insert(s, a.nonce);
            }
        }
        self.mempool.prune(&nonces);
    }

    /// Mempool txs in block order for the tip.
    pub async fn candidate_txs(&self) -> Vec<Tx> {
        let mut nonces = HashMap::new();
        for s in self.mempool.senders() {
            if let Ok(a) = self.account(&s).await {
                nonces.insert(s, a.nonce);
            }
        }
        let max = self.chain.tip().config.max_block_txs as usize;
        self.mempool.select(&nonces, max)
    }

    pub async fn template(&mut self, miner: Pubkey, choice: JobChoice, now_ms: u64) -> Result<(String, Template), ChainError> {
        let txs = self.candidate_txs().await;
        let t = self.chain.build_template(miner, choice, &txs, now_ms).await?;
        let id = template_id(&t);
        if !self.templates.iter().any(|(i, _)| *i == id) {
            self.templates.push_back((id.clone(), t.clone()));
            while self.templates.len() > MAX_TEMPLATES {
                self.templates.pop_front();
            }
        }
        Ok((id, t))
    }

    /// Txs for a mined header: from the named template or any template with the same
    /// parent and tx_root.
    pub fn template_txs(&self, header: &agent_types::Header, template_id: Option<&str>) -> Option<Vec<Tx>> {
        let m = |t: &Template| t.header.prev_hash == header.prev_hash && t.header.tx_root == header.tx_root;
        if let Some(id) = template_id {
            if let Some((_, t)) = self.templates.iter().find(|(i, _)| i == id) {
                if m(t) {
                    return Some(t.txs.clone());
                }
            }
        }
        self.templates.iter().rev().find(|(_, t)| m(t)).map(|(_, t)| t.txs.clone())
    }

    pub fn notify(&mut self, topic: &str, data: Value) {
        if self.subscribers.is_empty() {
            return;
        }
        let msg = json!({"jsonrpc": "2.0", "method": "notify", "params": {"topic": topic, "data": data}}).to_string();
        self.subscribers.retain(|s| {
            if !s.topics.iter().any(|t| t == topic) {
                return !s.tx.is_closed();
            }
            s.tx.unbounded_send(msg.clone()).is_ok()
        });
    }

    pub fn take_outbox(&mut self) -> Vec<Outbound> {
        std::mem::take(&mut self.outbox)
    }
}

/// `blake3(header with nonce, proof_hash and timestamp zeroed)`, hex.
pub fn template_id(t: &Template) -> String {
    let mut h = t.header;
    h.nonce = 0;
    h.proof_hash = [0; 32];
    h.timestamp_ms = 0;
    hex::encode(h.hash())
}
