//! The one background poller of the Cadre app. A single scheduler thread decides what is
//! due and runs each read in a worker (at most one in flight per source), so a slow
//! Solana read never holds up the chain tip. Every view draws from what it sends; no
//! view fetches on its own and nothing is refetched just because a view was opened.
//!
//! Sources and periods: the agent chain (tip, jobs, blocks, the wallet) every 4 s with
//! the Solana side every 20 s or when a withdrawal appears; town posts and trades every
//! 10 s; the agent list, DM params and models every 60 s; the inbox every 10 s once DMs
//! are live and there is an address; health checks every 60 s once the Status view has
//! been opened (they ping every component). `Kick::Now` runs every source at once
//! (`r` in a view, and after a command that writes).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::net::{self, Msg, Net};
use crate::ctx::Ctx;
use crate::snapshot::{self, Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Chain,
    Feed,
    Agents,
    Dm,
    Models,
    Inbox,
    Status,
}

impl Source {
    pub const ALL: [Source; 7] = [Source::Chain, Source::Feed, Source::Agents, Source::Dm, Source::Models, Source::Inbox, Source::Status];

    pub fn period(self) -> Duration {
        Duration::from_secs(match self {
            Source::Chain => 4,
            Source::Feed => 10,
            Source::Agents | Source::Dm | Source::Models | Source::Status => 60,
            Source::Inbox => 10,
        })
    }
}

/// What the views ask the poller for (set from the UI thread).
#[derive(Default)]
pub struct Wants {
    /// The Status view has been opened: keep the health checks fresh.
    pub status: AtomicBool,
    /// DMs are live and there is an address: read the inbox.
    pub inbox: AtomicBool,
    /// The model list arrived (stop asking).
    pub have_model: AtomicBool,
}

pub enum Kick {
    /// Everything now.
    Now,
    /// One source now.
    One(Source),
}

pub struct Handle {
    pub kick: Sender<Kick>,
    pub wants: Arc<Wants>,
}

/// The newest town posts and trades, raw from cadre-api.
#[derive(Debug, Clone)]
pub struct Feed {
    pub posts: Result<Vec<Value>, String>,
    pub trades: Result<Vec<Value>, String>,
}

/// Start the poller. `me` is the address whose inbox is read.
pub fn start(ctx: Arc<Ctx>, net: Net, me: Option<String>, tx: Sender<Msg>) -> Handle {
    let (kick_tx, kick_rx) = std::sync::mpsc::channel();
    let wants = Arc::new(Wants::default());
    let w = wants.clone();
    std::thread::Builder::new().name("cadre-poll".into()).spawn(move || run(ctx, net, me, tx, kick_rx, w)).expect("poller thread");
    Handle { kick: kick_tx, wants }
}

fn run(ctx: Arc<Ctx>, net: Net, me: Option<String>, tx: Sender<Msg>, kick: Receiver<Kick>, wants: Arc<Wants>) {
    let inflight: Arc<Mutex<std::collections::HashSet<Source>>> = Default::default();
    let mut last: std::collections::HashMap<Source, Instant> = Default::default();
    let prev: Arc<Mutex<Option<Snapshot>>> = Default::default();
    let last_sol: Arc<Mutex<Option<Instant>>> = Default::default();
    let mut force: Vec<Source> = Source::ALL.to_vec();
    loop {
        let now = Instant::now();
        for s in Source::ALL {
            let due = force.contains(&s) || last.get(&s).is_none_or(|t| now.duration_since(*t) >= s.period());
            let enabled = match s {
                Source::Status => wants.status.load(Ordering::Relaxed),
                Source::Inbox => wants.inbox.load(Ordering::Relaxed) && me.is_some(),
                Source::Models => !wants.have_model.load(Ordering::Relaxed),
                _ => true,
            };
            if !due || !enabled {
                continue;
            }
            {
                let mut f = inflight.lock().unwrap();
                if f.contains(&s) {
                    continue;
                }
                f.insert(s);
            }
            last.insert(s, now);
            let (ctx, net, tx, inflight, prev, last_sol, me) = (ctx.clone(), net.clone(), tx.clone(), inflight.clone(), prev.clone(), last_sol.clone(), me.clone());
            let _ = tx.send(Msg::Loading(s, true));
            std::thread::spawn(move || {
                let alive = fetch(s, &ctx, &net, me.as_deref(), &tx, &prev, &last_sol);
                inflight.lock().unwrap().remove(&s);
                if alive {
                    let _ = tx.send(Msg::Loading(s, false));
                }
            });
        }
        force.clear();
        match kick.recv_timeout(Duration::from_millis(500)) {
            Ok(Kick::Now) => force = Source::ALL.to_vec(),
            Ok(Kick::One(s)) => force.push(s),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// One read; returns false when the UI is gone.
fn fetch(s: Source, ctx: &Ctx, net: &Net, me: Option<&str>, tx: &Sender<Msg>, prev: &Mutex<Option<Snapshot>>, last_sol: &Mutex<Option<Instant>>) -> bool {
    let api = net.api.clone();
    let msg = match s {
        Source::Chain => {
            let p = prev.lock().unwrap().clone();
            let sol_due = last_sol.lock().unwrap().is_none_or(|t| t.elapsed() >= Duration::from_secs(20));
            let sn = snapshot::gather_next(ctx, p.as_ref(), sol_due);
            if sn.sol_read {
                *last_sol.lock().unwrap() = Some(Instant::now());
            }
            *prev.lock().unwrap() = Some(sn.clone());
            Msg::Snap(Box::new(sn))
        }
        Source::Feed => {
            let get = |path: &str, key: &str| -> Result<Vec<Value>, String> {
                let a = api.as_ref().ok_or("no cadre-api on this network")?;
                net::get(&format!("{a}{path}")).map(|v| v[key].as_array().cloned().unwrap_or_default()).map_err(|e| format!("{e:#}"))
            };
            let (posts, trades) = std::thread::scope(|sc| {
                let p = sc.spawn(|| get("/town?limit=30", "posts"));
                let t = sc.spawn(|| get("/trading/trades?limit=30", "trades"));
                (p.join().unwrap(), t.join().unwrap())
            });
            Msg::Feed(Box::new(Feed { posts, trades }))
        }
        Source::Agents => Msg::Agents(net.agents().map_err(|e| format!("{e:#}"))),
        Source::Models => Msg::Models(net.models().map_err(|e| format!("{e:#}"))),
        Source::Dm => {
            let r = match &api {
                Some(a) => net::get(&format!("{a}/dm/params")).map_err(|e| format!("{e:#}")),
                None => Err("no cadre-api on this network".into()),
            };
            Msg::DmParams(r)
        }
        Source::Inbox => {
            let (Some(a), Some(me)) = (&api, me) else { return true };
            Msg::Inbox(net::get(&format!("{a}/inbox/{me}?limit=50")).map_err(|e| format!("{e:#}")))
        }
        Source::Status => Msg::Status(Box::new(crate::health::run_all(ctx))),
    };
    tx.send(msg).is_ok()
}
