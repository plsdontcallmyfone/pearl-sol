//! The Cadre app opens only for `cadre` alone (or `dashboard`) on a terminal. Everything
//! else must be exactly as before: a pipe, `--json`, and `cadre status`. The endpoints point at a closed local port so every run is fast and offline.

use std::process::{Command, Stdio};

const ALT_ON: &str = "\x1b[?1049h";
const ALT_OFF: &str = "\x1b[?1049l";

fn args() -> Vec<&'static str> {
    vec!["--network", "localnet", "--rpc", "http://127.0.0.1:9", "--sol-rpc", "http://127.0.0.1:9", "--pool", "http://127.0.0.1:9"]
}

fn run(bin: &str, extra: &[&str]) -> (String, i32) {
    let out = Command::new(bin).args(args()).args(extra).stdin(Stdio::null()).stderr(Stdio::piped()).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn cadre_piped_runs_status_as_before() {
    let (alone, code) = run(env!("CARGO_BIN_EXE_cadre"), &[]);
    let (status, code2) = run(env!("CARGO_BIN_EXE_cadre"), &["status"]);
    assert!(!alone.contains(ALT_ON), "no full-screen UI on a pipe");
    assert!(alone.contains("[ Chain ]"), "{alone}");
    assert!(status.contains("[ Chain ]"));
    // the nodes are down here, so status fails the same way both times
    assert_eq!((code, code2), (1, 1));
}

#[test]
fn cadre_json_is_status_json() {
    let (alone, _) = run(env!("CARGO_BIN_EXE_cadre"), &["--json"]);
    let (status, _) = run(env!("CARGO_BIN_EXE_cadre"), &["status", "--json"]);
    let a: serde_json::Value = serde_json::from_str(alone.trim()).unwrap();
    let b: serde_json::Value = serde_json::from_str(status.trim()).unwrap();
    assert!(!alone.contains(ALT_ON));
    for k in ["ok", "checks", "counts", "network", "endpoints"] {
        assert!(a.get(k).is_some() && b.get(k).is_some(), "{k}");
    }
    let names = |v: &serde_json::Value| v["checks"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(names(&a), names(&b));
}

#[test]
fn dashboard_on_a_pipe_fails_fast_and_snapshot_draws_the_new_style() {
    // no terminal: an error and exit 1, not a hang and not a half-drawn screen
    let t0 = std::time::Instant::now();
    let (out, code) = run(env!("CARGO_BIN_EXE_cadre"), &["dashboard"]);
    assert_eq!(code, 1);
    assert!(!out.contains(ALT_ON));
    assert!(t0.elapsed() < std::time::Duration::from_secs(20));
    // `snapshot` renders the app's Dashboard view (offline here: every card says what is missing)
    let dir = std::env::temp_dir().join(format!("cadre-snap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let html = dir.join("d.html");
    let (_, code) = run(env!("CARGO_BIN_EXE_cadre"), &["snapshot", html.to_str().unwrap(), "--width", "120", "--height", "40"]);
    assert_eq!(code, 0);
    let h = std::fs::read_to_string(&html).unwrap();
    let t = std::fs::read_to_string(dir.join("d.txt")).unwrap();
    assert!(h.contains("#0a0a0a") && !h.contains("#0000aa"));
    assert!(t.contains("dashboard") && t.contains("chain offline") && t.contains("Wallet"), "{t}");
    assert!(!t.contains("beta version") && !t.contains("$$$"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// On a real terminal (a pty from `script`), `cadre` opens the alternate screen, and
/// ctrl+c leaves it and shows the cursor again.
#[cfg(target_os = "linux")]
#[test]
fn cadre_on_a_terminal_opens_and_restores() {
    if Command::new("script").arg("--version").output().is_err() {
        return;
    }
    let cmd = format!("stty cols 100 rows 40; exec {} {}", env!("CARGO_BIN_EXE_cadre"), args().join(" "));
    let mut child = Command::new("script")
        .args(["-qec", &cmd, "/dev/null"])
        .env("TERM", "xterm-256color")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    use std::io::Write;
    let _ = stdin.write_all(b"\x03");
    let _ = stdin.flush();
    let out = child.wait_with_output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    let on = s.find(ALT_ON).unwrap_or_else(|| panic!("alternate screen entered: {s:?}"));
    let off = s.rfind(ALT_OFF).expect("alternate screen left");
    assert!(off > on);
    assert!(s[off..].contains("\x1b[?25h") || s[..off].rfind("\x1b[?25h").is_some(), "cursor shown again");
    assert!(s.contains("cadre") || s.contains('█'), "the screen was drawn: {s:?}");
}
