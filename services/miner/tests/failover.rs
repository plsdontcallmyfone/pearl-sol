//! Several --rpc endpoints: the miner keeps producing blocks when its node goes down.
//! Two JSON-RPC servers over one mock chain on free ports in 10690-10699 (checked with lsof first);
//! server A starts answering 502 with no body (what caddy does when the node behind it is down).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use pearl_miner::{Miner, MinerConfig};
use pearl_node::mock::MockNode;
use pearl_node::{is_transport, FailoverNode, NodeApi};

const EASY: u32 = 0x1e040000;
const CHAIN: u32 = 4243;

/// Both tests bind ports from the same range: one at a time.
static PORTS: Mutex<()> = Mutex::new(());

fn free_ports(n: usize) -> Vec<u16> {
    (10690..=10699)
        .filter(|p| {
            let out = std::process::Command::new("lsof").args(["-ti", &format!(":{p}")]).output();
            matches!(out, Ok(o) if o.stdout.is_empty())
        })
        .take(n)
        .collect()
}

struct Server {
    server: Arc<tiny_http::Server>,
    handle: Option<std::thread::JoinHandle<()>>,
    down: Arc<AtomicBool>,
    served: Arc<AtomicU64>,
}

impl Server {
    fn start(port: u16, mock: MockNode) -> Self {
        let server = Arc::new(tiny_http::Server::http(format!("127.0.0.1:{port}")).expect("bind test port"));
        let (down, served) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
        let (srv, d, s) = (server.clone(), down.clone(), served.clone());
        let handle = std::thread::spawn(move || {
            for mut req in srv.incoming_requests() {
                let mut body = Vec::new();
                let _ = std::io::Read::read_to_end(req.as_reader(), &mut body);
                if d.load(Ordering::SeqCst) {
                    let _ = req.respond(tiny_http::Response::empty(502));
                    continue;
                }
                s.fetch_add(1, Ordering::SeqCst);
                let v = mock.handle_jsonrpc(&body);
                let _ = req.respond(tiny_http::Response::from_string(v.to_string()));
            }
        });
        Self { server, handle: Some(handle), down, served }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("miner-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn miner_fails_over_when_its_node_goes_down() {
    let _g = PORTS.lock().unwrap_or_else(|e| e.into_inner());
    let ports = free_ports(2);
    if ports.len() < 2 {
        eprintln!("need 2 free ports in 10690-10699, skipping");
        return;
    }
    let chain = MockNode::new(CHAIN, EASY);
    let a = Server::start(ports[0], chain.clone());
    let b = Server::start(ports[1], chain.clone());
    let logs = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (l1, l2) = (logs.clone(), logs.clone());
    let node = FailoverNode::new(&[format!("http://127.0.0.1:{}", ports[0]), format!("http://127.0.0.1:{}", ports[1])])
        .unwrap()
        .with_log(Box::new(move |v| l1.lock().unwrap().push(v)));

    // Take node A down once two blocks are in.
    let (watch, down) = (chain.clone(), a.down.clone());
    let killer = std::thread::spawn(move || {
        while watch.height() < 2 {
            std::thread::sleep(Duration::from_millis(20));
        }
        down.store(true, Ordering::SeqCst);
    });

    let dir = tmp("failover");
    let cfg = MinerConfig {
        threads: 2,
        refresh: Duration::from_millis(200),
        stats_every: Duration::from_secs(60),
        results_dir: dir.join("results"),
        max_blocks: Some(5),
        ..MinerConfig::default()
    };
    let mut m = Miner::new(&node, None, SigningKey::from_bytes(&[0x4e; 32]), cfg, Box::new(move |v| l2.lock().unwrap().push(v)));
    m.run(&|| false).unwrap();
    killer.join().unwrap();

    let logs = logs.lock().unwrap();
    let accepted = logs.iter().filter(|v| v["event"] == "block_accepted").count();
    let switches: Vec<&Value> = logs.iter().filter(|v| v["event"] == "rpc_failover").collect();
    assert_eq!(accepted, 5, "{logs:?}");
    assert!(chain.height() >= 5);
    assert_eq!(switches.len(), 1, "{switches:?}");
    assert_eq!(switches[0]["from"], json!(format!("http://127.0.0.1:{}", ports[0])));
    assert_eq!(switches[0]["to"], json!(format!("http://127.0.0.1:{}", ports[1])));
    assert!(switches[0]["error"].as_str().unwrap().contains("HTTP 502"), "{switches:?}");
    assert_eq!(node.current(), format!("http://127.0.0.1:{}", ports[1]));
    assert!(b.served.load(Ordering::SeqCst) > 0);
}

#[test]
fn json_rpc_errors_do_not_fail_over() {
    let _g = PORTS.lock().unwrap_or_else(|e| e.into_inner());
    let ports = free_ports(2);
    if ports.len() < 2 {
        eprintln!("need 2 free ports in 10690-10699, skipping");
        return;
    }
    let chain = MockNode::new(CHAIN, EASY);
    let a = Server::start(ports[0], chain.clone());
    let b = Server::start(ports[1], chain.clone());
    let node = FailoverNode::new(&[format!("http://127.0.0.1:{}", ports[0]), format!("http://127.0.0.1:{}", ports[1])]).unwrap();
    // An unknown method is the node's own answer: returned, not retried elsewhere.
    let e = node.call("noSuchMethod", json!({})).unwrap_err();
    assert!(!is_transport(&e), "{e:#}");
    assert_eq!(b.served.load(Ordering::SeqCst), 0);
    assert_eq!(node.current(), format!("http://127.0.0.1:{}", ports[0]));
    // Both down: the transport error comes back.
    a.down.store(true, Ordering::SeqCst);
    b.down.store(true, Ordering::SeqCst);
    let e = node.get_account(&[1; 32]).unwrap_err();
    assert!(is_transport(&e), "{e:#}");
}
