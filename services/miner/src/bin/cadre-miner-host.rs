//! cadre-miner-host: Chrome native messaging host (`ag.cadre.miner`) that lets the Cadre Wallet
//! start, stop and watch a pearl-miner on this machine. Chrome runs it once per
//! `sendNativeMessage`: one length-prefixed JSON request on stdin, one reply on stdout.
//!
//! Requests: {"cmd":"status"} | {"cmd":"start","payout":B58,"threads":N,"rpc":[URL..],"pool":URL?} |
//! {"cmd":"stop"}. The miner only gets the payout ADDRESS (`--payout-address`): no key leaves the
//! wallet. State under CADRE_HOME (default ~/.cadre): wallet-miner.pid, wallet-miner.json (the
//! running config), wallet-miner.log (the miner's JSON lines), wallet-miner/ (its results/models).
//! Only a process that is still this host's pearl-miner is ever signalled (checked with `ps`).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Log bytes read back for status (the newest events).
const TAIL_BYTES: u64 = 256 * 1024;
/// The log is started fresh above this size.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

fn main() {
    let reply = match read_message().and_then(|req| handle(&req)) {
        Ok(v) => v,
        Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
    };
    let _ = write_message(&reply);
}

fn read_message() -> Result<Value> {
    let mut len = [0u8; 4];
    io::stdin().read_exact(&mut len).context("no request on stdin")?;
    let n = u32::from_ne_bytes(len) as usize;
    if n > 64 * 1024 {
        bail!("request too large ({n} bytes)");
    }
    let mut buf = vec![0u8; n];
    io::stdin().read_exact(&mut buf)?;
    Ok(serde_json::from_slice(&buf)?)
}

fn write_message(v: &Value) -> Result<()> {
    let body = serde_json::to_vec(v)?;
    let mut out = io::stdout().lock();
    out.write_all(&(body.len() as u32).to_ne_bytes())?;
    out.write_all(&body)?;
    out.flush()?;
    Ok(())
}

struct Paths {
    home: PathBuf,
    miner: PathBuf,
    pid: PathBuf,
    config: PathBuf,
    log: PathBuf,
    work: PathBuf,
}

fn paths() -> Result<Paths> {
    // Chrome starts the host with no CADRE_HOME, so an installed host (CADRE_HOME/bin/) finds
    // its home from its own path.
    let installed = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().filter(|d| d.ends_with("bin")).and_then(|d| d.parent()).map(Path::to_path_buf))
        .filter(|h| h.join("bin/pearl-miner").is_file());
    let home = match std::env::var_os("CADRE_HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => match installed {
            Some(h) => h,
            None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".cadre"),
        },
    };
    // pearl-miner next to this binary (the installer puts both in ~/.cadre/bin), else CADRE_HOME/bin
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("pearl-miner")));
    let miner = beside.filter(|p| p.is_file()).unwrap_or_else(|| home.join("bin/pearl-miner"));
    Ok(Paths {
        pid: home.join("wallet-miner.pid"),
        config: home.join("wallet-miner.json"),
        log: home.join("wallet-miner.log"),
        work: home.join("wallet-miner"),
        miner,
        home,
    })
}

fn handle(req: &Value) -> Result<Value> {
    let p = paths()?;
    match req["cmd"].as_str().unwrap_or("") {
        "status" => status(&p),
        "start" => start(&p, req),
        "stop" => {
            let stopped = stop(&p)?;
            let mut v = status(&p)?;
            v["stopped"] = json!(stopped);
            Ok(v)
        }
        other => bail!("unknown cmd {other:?}"),
    }
}

/// The pid from the pid file when that process is still our pearl-miner.
fn running_pid(p: &Paths) -> Option<u32> {
    let pid: u32 = fs::read_to_string(&p.pid).ok()?.trim().parse().ok()?;
    let out = Command::new("ps").args(["-p", &pid.to_string(), "-o", "command="]).output().ok()?;
    let cmd = String::from_utf8_lossy(&out.stdout);
    let ours = cmd.contains(&*p.miner.to_string_lossy()) && cmd.contains("--payout-address");
    ours.then_some(pid)
}

fn status(p: &Paths) -> Result<Value> {
    let pid = running_pid(p);
    let config: Value = fs::read(&p.config).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    Ok(json!({
        "ok": true,
        "host_version": VERSION,
        "miner_installed": p.miner.is_file(),
        "miner_path": p.miner,
        "cores": cores,
        "running": pid.is_some(),
        "pid": pid,
        "config": config,
        "log": summarize_log(&p.log, pid.is_some()),
    }))
}

fn start(p: &Paths, req: &Value) -> Result<Value> {
    if !p.miner.is_file() {
        bail!("pearl-miner is not installed at {} (run the Cadre miner installer)", p.miner.display());
    }
    let payout = req["payout"].as_str().context("start needs payout")?;
    let decoded = bs58::decode(payout).into_vec().context("payout is not base58")?;
    if decoded.len() != 32 {
        bail!("payout must be a 32-byte address");
    }
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let threads = req["threads"].as_u64().unwrap_or(((cores / 2).max(1)) as u64).clamp(1, cores as u64);
    let rpc: Vec<String> = req["rpc"].as_array().map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if rpc.is_empty() {
        bail!("start needs at least one rpc URL");
    }
    let pool = req["pool"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
    for u in rpc.iter().chain(pool.iter()) {
        if !(u.starts_with("https://") || u.starts_with("http://")) || u.contains(char::is_whitespace) {
            bail!("not an http(s) URL: {u}");
        }
    }

    // One wallet miner at a time: a new start replaces the old one.
    stop(p)?;
    fs::create_dir_all(&p.work)?;
    if fs::metadata(&p.log).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = fs::rename(&p.log, p.home.join("wallet-miner.log.1"));
    }
    let log = OpenOptions::new().create(true).append(true).open(&p.log)?;

    let mut cmd = Command::new(&p.miner);
    cmd.arg("--payout-address").arg(payout).arg("--threads").arg(threads.to_string()).arg("--rpc").arg(rpc.join(","));
    if let Some(url) = &pool {
        cmd.arg("--pool").arg(url);
    }
    cmd.current_dir(&p.work)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        // own process group: Chrome closing this host must not take the miner with it
        .process_group(0);
    let child = cmd.spawn().with_context(|| format!("starting {}", p.miner.display()))?;
    fs::write(&p.pid, child.id().to_string())?;
    let started = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let config = json!({"payout": payout, "threads": threads, "rpc": rpc, "pool": pool, "started_at": started, "pid": child.id()});
    fs::write(&p.config, serde_json::to_vec_pretty(&config)?)?;
    // let it log its start line (or fail fast on a bad endpoint)
    std::thread::sleep(Duration::from_millis(600));
    status(p)
}

/// Stops the wallet's miner if it is running. Returns whether one was stopped.
fn stop(p: &Paths) -> Result<bool> {
    let Some(pid) = running_pid(p) else {
        let _ = fs::remove_file(&p.pid);
        return Ok(false);
    };
    let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
    for _ in 0..30 {
        if running_pid(p).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if running_pid(p).is_some() {
        let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
    }
    let _ = fs::remove_file(&p.pid);
    Ok(true)
}

/// What the wallet shows from the miner's own log lines since its last start.
fn summarize_log(path: &Path, running: bool) -> Value {
    let Ok(mut f) = File::open(path) else { return Value::Null };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)));
    let mut text = String::new();
    let _ = f.read_to_string(&mut text);
    let events: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect();
    let since = events.iter().rposition(|e| e["event"] == "start").unwrap_or(0);
    let run = &events[since..];
    let last = |name: &str| run.iter().rev().find(|e| e["event"] == name).cloned().unwrap_or(Value::Null);
    let blocks: Vec<Value> = run
        .iter()
        .filter(|e| e["event"] == "block_accepted" || (e["event"] == "pool_block" && e["accepted"] == true))
        .rev()
        .take(20)
        .cloned()
        .collect();
    let errors: Vec<Value> = run
        .iter()
        .filter(|e| e["error"].is_string())
        .rev()
        .take(5)
        .cloned()
        .collect();
    // non-JSON lines (a panic, a clap error) when the miner is not running
    let raw_tail: Vec<&str> = if running { vec![] } else { text.lines().rev().filter(|l| !l.starts_with('{')).take(5).collect() };
    json!({
        "start": last("start"),
        "stats": last("stats"),
        "job": last("job_loaded"),
        "stop": last("stop"),
        "blocks": blocks,
        "errors": errors,
        "raw_tail": raw_tail,
    })
}
