//! Node-local job index (2026-10-22, devnet box 0 CPU; docs/RPC.md listJobs, SPEC 4.5
//! "Segment order"). Not consensus: it only answers queries about the tip state faster.
//!
//! Before it, `listJobs` read two records per job for the newest 10,000 jobs on every call
//! (index entry + record, 20,000 state reads), the miner's orphan sweep paged it about 40
//! times a minute, and every auto-choice template read the newest 4096 jobs again. The index
//! holds, for the tip:
//! - every job's id and a status/kind byte, by job index (about 40 bytes a job);
//! - the full record of every live job (Open or Delivered) and of every ticket;
//! - which state keys belong to which job (record key and index-entry key).
//!
//! It is brought up to date from [`consensus_pow::Chain::take_touched`]: the keys written by
//! every block connected to or disconnected from the canonical chain since the last update.
//! Re-reading exactly those keys at the tip is right across any reorg, because a key no such
//! block wrote holds the same value at the old and the new tip. Built lazily (first query),
//! by one full scan; rebuilt from scratch only if the chain's tracking was off.

use std::collections::{BTreeMap, HashMap};

use agent_types::Hash;
use commonware_runtime::Spawner;
use consensus_pow::{ChainError, PowVerifier};
use state::schema::{self, JobRecord, JobStatus, Key, TicketExt};

use crate::core::Core;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Record(u64),
    Entry(u64),
}

/// Kind codes in the status byte (high nibble).
const KIND_JOB: u8 = 0;
const KIND_TICKET: u8 = 1;
const KIND_SEGMENT: u8 = 2;

fn kind_code(j: &JobRecord) -> u8 {
    match (j.is_ticket(), j.segment().is_some()) {
        (true, _) => KIND_TICKET,
        (_, true) => KIND_SEGMENT,
        _ => KIND_JOB,
    }
}

pub fn kind_from_name(s: &str) -> Option<u8> {
    match s {
        "job" => Some(KIND_JOB),
        "ticket" => Some(KIND_TICKET),
        "segment" => Some(KIND_SEGMENT),
        _ => None,
    }
}

#[derive(Default)]
pub struct JobIndex {
    /// Tip the index describes (None: not built).
    pub at: Option<Hash>,
    ids: Vec<Hash>,
    /// `status | kind << 4` by job index.
    flags: Vec<u8>,
    slots: HashMap<Key, Slot>,
    /// Live jobs and every ticket, by job index.
    records: BTreeMap<u64, JobRecord>,
    pub rebuilds: u64,
    pub updates: u64,
    pub reads: u64,
}

/// What a listJobs call matches.
#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    /// open | delivered | completed | expired | live | mineable
    pub status: Option<String>,
    pub kind: Option<u8>,
}

impl JobIndex {
    pub fn len(&self) -> u64 {
        self.ids.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn live_records(&self) -> usize {
        self.records.len()
    }

    pub fn id(&self, i: u64) -> Option<Hash> {
        self.ids.get(i as usize).copied()
    }

    /// The in-memory record of job `i` (live jobs and tickets only).
    pub fn record(&self, i: u64) -> Option<&JobRecord> {
        self.records.get(&i)
    }

    /// Does job `i` match `f` (without its record for closed statuses)? `next` is the next
    /// block height (mineable).
    pub fn matches(&self, i: u64, f: &JobFilter, next: u64) -> bool {
        let Some(&b) = self.flags.get(i as usize) else { return false };
        if f.kind.is_some_and(|k| k != b >> 4) {
            return false;
        }
        let status = b & 0x0f;
        match f.status.as_deref() {
            None | Some("all") => true,
            Some("live") => status <= JobStatus::Delivered as u8,
            Some("mineable") => self.records.get(&i).is_some_and(|j| executor::check_mineable(j, next).is_ok()),
            Some(s) => JobStatus::from_u8(status).map(|st| st.name() == s).unwrap_or(false),
        }
    }

    /// Mineable jobs among the newest `scan` (the template's window) and the tickets of the
    /// segments among them.
    pub fn template_candidates(&self, next: u64, scan: u64) -> (Vec<JobRecord>, HashMap<Hash, TicketExt>) {
        let lo = self.len().saturating_sub(scan);
        let mut jobs = Vec::new();
        let mut tickets = HashMap::new();
        for (_, j) in self.records.range(lo..) {
            if executor::check_mineable(j, next).is_ok() && j.pearl_job().is_ok() {
                if let Some((tid, _, _)) = j.segment() {
                    if let std::collections::hash_map::Entry::Vacant(e) = tickets.entry(tid) {
                        if let Some(t) = self.ticket_by_id(&tid) {
                            e.insert(t);
                        }
                    }
                }
                jobs.push(j.clone());
            }
        }
        (jobs, tickets)
    }

    fn ticket_by_id(&self, id: &Hash) -> Option<TicketExt> {
        match self.slots.get(&schema::job_key(id)) {
            Some(Slot::Record(i)) => self.records.get(i).and_then(|j| j.ticket().copied()),
            _ => None,
        }
    }

    fn forget(&mut self, i: u64) {
        if let Some(id) = self.ids.get(i as usize) {
            self.slots.remove(&schema::job_key(id));
        }
        self.records.remove(&i);
    }

    fn set_record(&mut self, i: u64, j: Option<JobRecord>) {
        match j {
            Some(j) => {
                self.flags[i as usize] = j.status as u8 | kind_code(&j) << 4;
                if j.status.is_live() || j.is_ticket() {
                    self.records.insert(i, j);
                } else {
                    self.records.remove(&i);
                }
            }
            None => {
                // an index entry whose record is missing (never on a sound chain)
                self.flags[i as usize] = JobStatus::Expired as u8;
                self.records.remove(&i);
            }
        }
    }
}

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    async fn read_job_entry(&mut self, i: u64) -> Result<Option<Hash>, ChainError> {
        self.jobs.reads += 1;
        match self.chain.read_tip(&schema::job_index_key(i)).await? {
            Some(b) => Ok(Some(schema::decode_job_index(&b).map_err(|e| ChainError::Internal(e.to_string()))?)),
            None => Ok(None),
        }
    }

    async fn read_job_record(&mut self, id: &Hash) -> Result<Option<JobRecord>, ChainError> {
        self.jobs.reads += 1;
        match self.chain.read_tip(&schema::job_key(id)).await? {
            Some(b) => Ok(Some(JobRecord::decode(&b).map_err(|e| ChainError::Internal(e.to_string()))?)),
            None => Ok(None),
        }
    }

    async fn job_count_at_tip(&self) -> Result<u64, ChainError> {
        Ok(match self.chain.read_tip(&schema::counters_key()).await? {
            Some(b) => schema::Counters::decode(&b).map_err(|e| ChainError::Internal(e.to_string()))?.job_count,
            None => 0,
        })
    }

    /// Bring the job index to the tip (building it on first use).
    pub async fn sync_jobs(&mut self) -> Result<(), ChainError> {
        let tip = self.chain.tip().hash;
        if self.jobs.at.is_none() {
            return self.rebuild_jobs().await;
        }
        let touched = self.chain.take_touched();
        if self.jobs.at == Some(tip) && touched.is_empty() {
            return Ok(());
        }
        let count = self.job_count_at_tip().await?;
        let mut reload: Vec<u64> = Vec::new();
        // a reorg to a branch with fewer jobs
        while self.jobs.len() > count {
            let i = self.jobs.len() - 1;
            self.jobs.forget(i);
            self.jobs.slots.remove(&schema::job_index_key(i));
            self.jobs.ids.pop();
            self.jobs.flags.pop();
        }
        for k in &touched {
            match self.jobs.slots.get(k).copied() {
                Some(Slot::Record(i)) => reload.push(i),
                Some(Slot::Entry(i)) => {
                    // the job at index i differs on the new branch
                    let id = self.read_job_entry(i).await?.unwrap_or([0; 32]);
                    if self.jobs.ids[i as usize] != id {
                        self.jobs.forget(i);
                        self.jobs.ids[i as usize] = id;
                        self.jobs.slots.insert(schema::job_key(&id), Slot::Record(i));
                    }
                    reload.push(i);
                }
                None => {}
            }
        }
        for i in self.jobs.len()..count {
            let id = self.read_job_entry(i).await?.unwrap_or([0; 32]);
            self.jobs.ids.push(id);
            self.jobs.flags.push(JobStatus::Expired as u8);
            self.jobs.slots.insert(schema::job_key(&id), Slot::Record(i));
            self.jobs.slots.insert(schema::job_index_key(i), Slot::Entry(i));
            reload.push(i);
        }
        reload.sort_unstable();
        reload.dedup();
        for i in reload {
            let id = self.jobs.ids[i as usize];
            let j = self.read_job_record(&id).await?;
            self.jobs.set_record(i, j);
        }
        self.jobs.at = Some(tip);
        self.jobs.updates += 1;
        Ok(())
    }

    /// Full scan of every job at the tip.
    pub async fn rebuild_jobs(&mut self) -> Result<(), ChainError> {
        self.chain.track_touched();
        let _ = self.chain.take_touched();
        let tip = self.chain.tip().hash;
        let count = self.job_count_at_tip().await?;
        let reads = self.jobs.reads;
        let (rebuilds, updates) = (self.jobs.rebuilds, self.jobs.updates);
        self.jobs = JobIndex { reads, rebuilds: rebuilds + 1, updates, ..Default::default() };
        for i in 0..count {
            let id = self.read_job_entry(i).await?.unwrap_or([0; 32]);
            self.jobs.ids.push(id);
            self.jobs.flags.push(JobStatus::Expired as u8);
            self.jobs.slots.insert(schema::job_key(&id), Slot::Record(i));
            self.jobs.slots.insert(schema::job_index_key(i), Slot::Entry(i));
            let j = self.read_job_record(&id).await?;
            self.jobs.set_record(i, j);
        }
        self.jobs.at = Some(tip);
        tracing::info!(jobs = count, live = self.jobs.live_records(), "job index built");
        Ok(())
    }

    /// Record of job `i` at the tip: from the index when it holds it, else one state read.
    pub async fn job_by_index(&mut self, i: u64) -> Result<Option<JobRecord>, ChainError> {
        if let Some(j) = self.jobs.record(i) {
            return Ok(Some(j.clone()));
        }
        let Some(id) = self.jobs.id(i) else { return Ok(None) };
        self.read_job_record(&id).await
    }
}

/// listJobs without a cursor looks at the newest this many jobs (as before the index).
pub const LIST_WINDOW: u64 = 10_000;

impl<E: commonware_storage::Context + Spawner, V: PowVerifier> Core<E, V> {
    /// listJobs (docs/RPC.md): `status` (open, delivered, completed, expired, live, mineable,
    /// all), `kind` (job, ticket, segment), `limit` (<= 200). Paging: `offset` over the newest
    /// 10,000 jobs, oldest first (as before), or a cursor over every job: `after` = a job index
    /// (ascending from the next one) or `before` (descending). `next_cursor` is the index to pass
    /// for the next page (null at the end). Answered from the job index: only the returned page
    /// is decoded, closed jobs are read from state only when they are on the page.
    pub async fn list_jobs(&mut self, p: &serde_json::Value) -> Result<serde_json::Value, crate::RpcError> {
        use crate::dispatch::{p_str, p_u64, srv};
        let status = p_str(p, &["status"]).map(|s| s.to_ascii_lowercase());
        let kind = match p_str(p, &["kind"]) {
            None | Some("all") => None,
            Some(k) => Some(kind_from_name(&k.to_ascii_lowercase()).ok_or_else(|| crate::RpcError::params("kind is job, ticket or segment"))?),
        };
        let limit = p_u64(p, "limit").unwrap_or(50).min(200) as usize;
        let offset = p_u64(p, "offset").unwrap_or(0) as usize;
        let (after, before) = (p_u64(p, "after"), p_u64(p, "before"));
        srv(self.sync_jobs().await)?;
        let next = self.chain.tip_height() + 1;
        let count = self.jobs.len();
        let f = JobFilter { status, kind };
        let m = |ix: &JobIndex, i: u64| ix.matches(i, &f, next);
        let (page, total, next_cursor): (Vec<u64>, usize, Option<u64>) = if let Some(b) = before {
            let total = (0..count).filter(|&i| m(&self.jobs, i)).count();
            let mut it = (0..b.min(count)).rev().filter(|&i| m(&self.jobs, i));
            let page: Vec<u64> = it.by_ref().take(limit).collect();
            let more = it.next().is_some();
            let nc = if more { page.last().copied() } else { None };
            (page, total, nc)
        } else if let Some(a) = after {
            let total = (0..count).filter(|&i| m(&self.jobs, i)).count();
            let mut it = (a.saturating_add(1)..count).filter(|&i| m(&self.jobs, i));
            let page: Vec<u64> = it.by_ref().take(limit).collect();
            let more = it.next().is_some();
            let nc = if more { page.last().copied() } else { None };
            (page, total, nc)
        } else {
            let all: Vec<u64> = (count.saturating_sub(LIST_WINDOW)..count).filter(|&i| m(&self.jobs, i)).collect();
            let page: Vec<u64> = all.iter().skip(offset).take(limit).copied().collect();
            let nc = if offset + page.len() < all.len() { page.last().copied() } else { None };
            (page, all.len(), nc)
        };
        let mut jobs = Vec::with_capacity(page.len());
        for i in page {
            if let Some(j) = srv(self.job_by_index(i).await)? {
                jobs.push(crate::dispatch::job_json(&j, next));
            }
        }
        Ok(serde_json::json!({"total": total, "jobs": jobs, "next_cursor": next_cursor}))
    }

    /// getNodeStats: RPC calls and time by method since start, the state read cache, the job
    /// index and the pool eviction counters (node-local diagnostics).
    pub fn node_stats(&self, now_ms: u64) -> serde_json::Value {
        let cache = self.chain.store().cache();
        let (entries, bytes, budget) = cache.size();
        let ld = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed);
        let mut methods: Vec<(&String, &(u64, u64))> = self.rpc_stats.iter().collect();
        methods.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
        let rpc: serde_json::Map<String, serde_json::Value> = methods
            .into_iter()
            .map(|(k, (n, us))| (k.clone(), serde_json::json!({"calls": n, "total_ms": us / 1000, "avg_us": us / (*n).max(1)})))
            .collect();
        serde_json::json!({
            "uptime_ms": now_ms.saturating_sub(self.started_ms),
            "rpc": rpc,
            "state_cache": {"hits": ld(&cache.hits), "misses": ld(&cache.misses), "bypass": ld(&cache.bypass),
                            "entries": entries, "bytes": bytes, "budget_bytes": budget},
            "job_index": {"built": self.jobs.at.is_some(), "jobs": self.jobs.len(), "records": self.jobs.live_records(),
                          "rebuilds": self.jobs.rebuilds, "updates": self.jobs.updates, "state_reads": self.jobs.reads},
            "mempool": {"size": self.mempool.len(), "failing": self.tx_strikes.len(), "evicted": self.evicted,
                        "evict_after_tips": crate::core::EVICT_AFTER_TIPS},
        })
    }
}
