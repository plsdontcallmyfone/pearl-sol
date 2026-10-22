//! [`FailoverNode`]: several node RPC endpoints of the same chain behind one [`NodeApi`].
//!
//! Calls go to the current endpoint. When it cannot be reached (a [`TransportError`]: connect
//! refused, timeout, a proxy 5xx with no JSON-RPC body) the call is retried on the next endpoint,
//! which then becomes current, so a miner keeps working while one public node is down. A JSON-RPC
//! error from a node that answered is returned as is: every node of a chain would give the same
//! answer. Block templates are node-local (submitBlock looks the template up on the node that made
//! it), so the client stays on one endpoint until that endpoint fails, instead of rotating.
//!
//! [`TransportError`]: crate::rpc::TransportError

use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{ensure, Result};
use serde_json::{json, Value};

use pearl_job::pow::MinedBlock;

use crate::rpc::{is_transport, HttpNode};
use crate::{Account, JobInfo, NodeApi, SubmitOutcome, Template};

pub type SwitchLog = Box<dyn Fn(Value) + Send + Sync>;

pub struct FailoverNode {
    nodes: Vec<HttpNode>,
    cur: AtomicUsize,
    log: Option<SwitchLog>,
}

impl FailoverNode {
    /// One endpoint per URL, in order; the first is current.
    pub fn new(urls: &[String]) -> Result<Self> {
        ensure!(!urls.is_empty(), "no node RPC URL given");
        Ok(Self { nodes: urls.iter().map(|u| HttpNode::new(u)).collect(), cur: AtomicUsize::new(0), log: None })
    }

    /// Called with `{"event": "rpc_failover", "from", "to", "error"}` on every switch.
    pub fn with_log(mut self, log: SwitchLog) -> Self {
        self.log = Some(log);
        self
    }

    pub fn urls(&self) -> Vec<&str> {
        self.nodes.iter().map(|n| n.url.as_str()).collect()
    }

    /// The endpoint calls go to now.
    pub fn current(&self) -> &str {
        &self.nodes[self.cur.load(Ordering::Relaxed) % self.nodes.len()].url
    }

    /// Run `f` on the current endpoint, then on each other one in turn while the failure is a
    /// transport failure. The first endpoint that answers becomes current.
    fn with<T>(&self, f: impl Fn(&HttpNode) -> Result<T>) -> Result<T> {
        let n = self.nodes.len();
        let start = self.cur.load(Ordering::Relaxed) % n;
        let mut last = None;
        for step in 0..n {
            let i = (start + step) % n;
            match f(&self.nodes[i]) {
                Ok(v) => {
                    if i != start && self.cur.compare_exchange(start, i, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                        if let Some(log) = &self.log {
                            let err = last.as_ref().map(|e: &anyhow::Error| format!("{e:#}"));
                            log(json!({"event": "rpc_failover", "from": self.nodes[start].url, "to": self.nodes[i].url, "error": err}));
                        }
                    }
                    return Ok(v);
                }
                Err(e) if is_transport(&e) && n > 1 => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.expect("at least one endpoint was tried"))
    }
}

impl NodeApi for FailoverNode {
    fn block_template(&self, payout: &[u8; 32]) -> Result<Template> {
        self.with(|n| n.block_template(payout))
    }
    fn fallback_template(&self, payout: &[u8; 32]) -> Result<Template> {
        self.with(|n| n.fallback_template(payout))
    }
    fn submit_weak_block(&self, t: &Template, b: &MinedBlock) -> Result<SubmitOutcome> {
        self.with(|n| n.submit_weak_block(t, b))
    }
    fn submit_block(&self, t: &Template, b: &MinedBlock) -> Result<SubmitOutcome> {
        // Another node does not hold this template, so it answers with a rejection (no template
        // with this parent and tx_root); the miner takes a fresh template from it next.
        self.with(|n| n.submit_block(t, b))
    }
    fn get_job(&self, job_id: &[u8; 32]) -> Result<Option<JobInfo>> {
        self.with(|n| n.get_job(job_id))
    }
    fn get_account(&self, account: &[u8; 32]) -> Result<Account> {
        self.with(|n| n.get_account(account))
    }
    fn send_tx(&self, wire: &[u8]) -> Result<String> {
        self.with(|n| n.send_tx(wire))
    }
    fn get_blob(&self, id: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        self.with(|n| n.get_blob(id))
    }
    fn put_blob(&self, data: &[u8]) -> Result<String> {
        self.with(|n| n.put_blob(data))
    }
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.with(|n| n.call(method, params.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_an_endpoint() {
        assert!(FailoverNode::new(&[]).is_err());
    }

    #[test]
    fn refused_connections_are_transport_errors_and_all_fail() {
        // Port 9 (discard) is closed on test machines: connection refused on both.
        let f = FailoverNode::new(&["http://127.0.0.1:9".into(), "http://127.0.0.1:9/b".into()]).unwrap();
        let e = f.call("getChainInfo", json!({})).unwrap_err();
        assert!(is_transport(&e), "{e:#}");
        assert_eq!(f.current(), "http://127.0.0.1:9");
    }
}
