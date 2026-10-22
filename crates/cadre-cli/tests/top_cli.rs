//! `cadre top` against a local HTTP server that serves the leaderboards fixture captured from the
//! live devnet API (tests/fixtures/leaderboards.json): text mode prints every board, --json keeps
//! the API's boards, a bad window fails before any request.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

const FIXTURE: &str = include_str!("fixtures/leaderboards.json");

fn serve() -> (String, std::thread::JoinHandle<Vec<String>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = std::thread::spawn(move || {
        let mut paths = vec![];
        l.set_nonblocking(false).unwrap();
        for s in l.incoming().take(2) {
            let mut s = s.unwrap();
            let mut first = String::new();
            let mut r = BufReader::new(s.try_clone().unwrap());
            r.read_line(&mut first).unwrap();
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            paths.push(first.split_whitespace().nth(1).unwrap_or("").to_string());
            let _ = write!(s, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{FIXTURE}", FIXTURE.len());
        }
        paths
    });
    (format!("api=http://127.0.0.1:{port}/v1"), h)
}

fn run(ep: &str, extra: &[&str]) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_cadre")).args(["--network", "localnet", "--endpoint", ep]).args(extra).stdin(Stdio::null()).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn top_lists_every_board_in_text_and_json() {
    let (ep, h) = serve();
    let (text, err, code) = run(&ep, &["top", "--window", "all", "--side", "all"]);
    assert_eq!(code, 0, "{err}");
    for want in ["Best traders", "Market earnings", "Miners", "Evaluators", "@tide_ledger"] {
        assert!(text.contains(want), "{want}\n{text}");
    }
    assert!(!text.contains('\u{2014}'));
    let (json, _, code) = run(&ep, &["--json", "top"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(json.trim()).unwrap();
    assert_eq!(v["boards"].as_array().unwrap().len(), 9);
    let paths = h.join().unwrap();
    assert_eq!(paths[0], "/v1/leaderboards?window=all&side=all&limit=3");
    assert_eq!(paths[1], "/v1/leaderboards?window=7d&side=organic&limit=3");
}

#[test]
fn top_refuses_a_bad_window() {
    let (_, err, code) = run("api=http://127.0.0.1:9/v1", &["top", "miners", "--window", "1y"]);
    assert_ne!(code, 0);
    assert!(err.contains("window is 24h, 7d or all"), "{err}");
}
