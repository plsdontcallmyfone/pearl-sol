//! What the entry screen reads and sends, all off the UI thread:
//! - the gateway's models (`GET <api>/openai/models`, or the box-local gateway),
//! - a chat completion, streamed (SSE), paid from the user's prepaid balance with the
//!   wallet signature (cadre-auth-v2, SPEC 5.4) of `--key`, or an API key in CADRE_API_KEY,
//! - the agents one can talk to (`GET <api>/profiles`), an agent's profile and posts,
//! - DMs (SPEC 4.10): whether they are live, what one costs (gate + builder), and sending
//!   one (SendMessage signed with `--key`).
//! Nothing here is invented: every figure shown comes from one of these answers.

use std::io::{BufRead, BufReader, Read};
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Where to reach the gateway and who pays.
#[derive(Clone)]
pub struct Net {
    /// cadre-api base ending in /v1 (None when the network has none).
    pub api: Option<String>,
    /// A gateway reached directly (`--endpoint gateway=...` or `--local`), base without /v1.
    pub gateway: Option<String>,
    pub key: Option<SigningKey>,
    /// `cadre_sk_...` from CADRE_API_KEY (never printed).
    pub api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    /// The gateway's server key (the signature audience).
    pub server: String,
}

#[derive(Debug, Clone)]
pub struct AgentEntry {
    pub account: String,
    pub handle: String,
    pub name: String,
    pub house: bool,
    /// GitHub username from the profile's links (exact casing), shown instead of the handle
    pub gh: Option<String>,
}

impl AgentEntry {
    /// How the agent is named on screen: @<GitHub username> when the profile links one, else @handle.
    pub fn at(&self) -> String {
        format!("@{}", self.gh.as_deref().unwrap_or(&self.handle))
    }
}

/// The GitHub username in a profile's links (https://github.com/<user>), if any.
pub fn github_of(links: &Value) -> Option<String> {
    links.as_array()?.iter().filter_map(|l| l.as_str()).find_map(|l| {
        let u = l.trim_end_matches('/').strip_prefix("https://github.com/")?;
        (!u.is_empty() && !u.contains('/') && u.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then(|| u.to_string())
    })
}

/// Messages from the worker threads to the UI loop.
#[derive(Debug)]
pub enum Msg {
    Models(Result<Model, String>),
    Agents(Result<Vec<AgentEntry>, String>),
    DmLive(bool),
    /// (request number, text so far is extended by this piece)
    Chunk(u64, String),
    /// (request number, receipt line or the error)
    Done(u64, Result<String, String>),
    /// (handle, lines of the card, or the error)
    Profile(String, Result<Vec<(String, u8)>, String>),
    /// (handle, the cost line and the quote, or the error)
    DmQuote(String, Result<DmQuote, String>),
    /// (handle, sent line, or the error)
    DmSent(String, Result<String, String>),
    /// The poller started (true) or finished (false) a read.
    Loading(super::poll::Source, bool),
    /// A new read of the chain, the wallet and the Solana side.
    Snap(Box<crate::snapshot::Snapshot>),
    /// Town posts and trades.
    Feed(Box<super::poll::Feed>),
    /// `/v1/dm/params`.
    DmParams(Result<Value, String>),
    /// `/v1/inbox/{me}`.
    Inbox(Result<Value, String>),
    /// Health checks of everything.
    Status(Box<crate::health::Report>),
    /// (request number, a command's result) for the Lookup view.
    CmdDone(u64, Result<crate::commands::Out, String>),
    /// (peer account, `/v1/threads/{me}/{peer}`) for the Inbox view.
    Thread(String, Result<Value, String>),
    /// The Send tab's transfer finished.
    Sent(Result<String, String>),
    /// The services market (Market view).
    Market(Box<Result<crate::market::Snap, String>>),
    /// (offer or hire, its detail cards) for the Market view.
    MarketDetail((crate::market::Pane, u64), Result<crate::commands::Out, String>),
    /// The Trade view's read (trade.rs).
    Trade(Box<super::trade::TradeData>),
    /// The Boards view's read (boards_view.rs).
    Boards(Box<Result<serde_json::Value, String>>),
}

#[derive(Debug, Clone)]
pub struct DmQuote {
    pub to: String,
    pub body: String,
    pub fee: u64,
    pub pay: u64,
}

fn agent(t: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(t).user_agent(concat!("cadre-cli/", env!("CARGO_PKG_VERSION"))).build()
}

/// The GitHub org whose members are Cadre's agents (owner 2026-10-22: the agents list is only
/// the agents in our org). CADRE_GITHUB_ORG overrides; empty turns the filter off.
pub fn github_org() -> String {
    std::env::var("CADRE_GITHUB_ORG").unwrap_or_else(|_| "cadredotag".into())
}

/// Public members of the org (GitHub logins, exact casing), cached 6 h in ~/.cadre/org-members.json;
/// a stale cache is used when GitHub cannot be reached. Empty when unknown.
pub fn org_members() -> Vec<String> {
    let org = github_org();
    if org.is_empty() {
        return vec![];
    }
    let path = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cadre/org-members.json"));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let cached: Option<Value> = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).filter(|v: &Value| v["org"] == org.as_str());
    let logins = |v: &Value| -> Vec<String> { v["logins"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(String::from)).collect() };
    if let Some(c) = &cached {
        if now.saturating_sub(c["at"].as_u64().unwrap_or(0)) < 6 * 3600 {
            return logins(c);
        }
    }
    match get(&format!("https://api.github.com/orgs/{org}/members?per_page=100")) {
        Ok(v) => {
            let l: Vec<String> = v.as_array().into_iter().flatten().filter_map(|m| m["login"].as_str().map(String::from)).collect();
            if let Some(p) = &path {
                let _ = std::fs::create_dir_all(p.parent().unwrap());
                let _ = std::fs::write(p, serde_json::to_vec(&json!({"org": org, "at": now, "logins": l})).unwrap_or_default());
            }
            l
        }
        Err(_) => cached.as_ref().map(logins).unwrap_or_default(),
    }
}

/// GET returning JSON on a 2xx, else the API's own error message.
pub fn get(url: &str) -> Result<Value> {
    match agent(Duration::from_secs(20)).get(url).call() {
        Ok(r) => Ok(r.into_json()?),
        Err(ureq::Error::Status(c, r)) => Err(anyhow!("{}", api_error(c, r))),
        Err(e) => Err(anyhow!("{}", short(&e))),
    }
}

fn short(e: &ureq::Error) -> String {
    let s = e.to_string();
    match s.rsplit_once(": ") {
        Some((_, c)) if !c.is_empty() => c.to_string(),
        _ => s,
    }
}

/// "HTTP 402: <message>" from an error response's JSON body (OpenAI or cadre-api shape).
fn api_error(code: u16, r: ureq::Response) -> String {
    let mut body = String::new();
    let _ = r.into_reader().take(1 << 20).read_to_string(&mut body);
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let m = v["error"]["message"].as_str().or(v["error"].as_str()).or(v["message"].as_str()).map(String::from);
    match m {
        Some(m) => format!("HTTP {code}: {m}"),
        None if code == 402 => "HTTP 402: payment required (no prepaid balance for this wallet)".into(),
        None => format!("HTTP {code}"),
    }
}

impl Net {
    /// Base of the OpenAI-compatible API and the path prefix the gateway signs.
    /// Through cadre-api: `<api>/openai/chat/completions` maps to gateway `/v1/chat/completions`.
    fn openai(&self) -> Option<String> {
        match (&self.gateway, &self.api) {
            (Some(g), _) => Some(format!("{}/v1", g.trim_end_matches('/'))),
            (None, Some(a)) => Some(format!("{a}/openai")),
            _ => None,
        }
    }

    pub fn models(&self) -> Result<Model> {
        let base = self.openai().ok_or_else(|| anyhow!("no cadre-api or gateway on this network"))?;
        let v = get(&format!("{base}/models"))?;
        let m = v["data"].as_array().and_then(|d| d.iter().find(|m| m["cadre"]["pricing"]["default"] == true).or(d.first())).ok_or_else(|| anyhow!("the gateway serves no model"))?;
        Ok(Model { id: m["id"].as_str().unwrap_or("").to_string(), server: m["cadre"]["server"].as_str().unwrap_or("").to_string() })
    }

    /// The one-line way to set up paying, when nothing is configured.
    pub fn setup_hint(&self) -> Option<&'static str> {
        if self.key.is_some() || self.api_key.is_some() {
            None
        } else {
            Some("Asking is paid from a prepaid balance: run `cadre wallet use --key <wallet.json>` once (a funded Cadre wallet) or set CADRE_API_KEY=cadre_sk_...")
        }
    }

    /// Headers paying for `body` on POST `/v1/chat/completions` at this gateway.
    pub fn auth_headers(&self, server: &str, body: &[u8], now_ms: u64) -> Result<Vec<(String, String)>> {
        if let Some(k) = &self.api_key {
            return Ok(vec![("authorization".into(), format!("Bearer {k}"))]);
        }
        let key = self.key.as_ref().ok_or_else(|| anyhow!("no wallet key or API key"))?;
        if server.is_empty() {
            bail!("the gateway did not name its server key (needed for the signature)");
        }
        let msg = auth_message(server, "POST", "/v1/chat/completions", now_ms, body);
        let sig = key.sign(&msg);
        Ok(vec![
            ("x-cadre-wallet".into(), bs58::encode(key.verifying_key().to_bytes()).into_string()),
            ("x-cadre-timestamp".into(), now_ms.to_string()),
            ("x-cadre-signature".into(), hex::encode(sig.to_bytes())),
        ])
    }

    /// Stream one chat answer: `Chunk`s, then `Done` with the receipt (or the error).
    pub fn ask(&self, n: u64, model: &Model, messages: Vec<Value>, max_tokens: u32, tx: &Sender<Msg>) {
        let r = (|| -> Result<String> {
            let base = self.openai().ok_or_else(|| anyhow!("no cadre-api or gateway on this network"))?;
            let body = serde_json::to_vec(&json!({"model": model.id, "messages": messages, "max_tokens": max_tokens, "stream": true}))?;
            let mut req = agent(Duration::from_secs(1800)).post(&format!("{base}/chat/completions")).set("content-type", "application/json").set("accept", "text/event-stream");
            for (k, v) in self.auth_headers(&model.server, &body, crate::probe::now_ms())? {
                req = req.set(&k, &v);
            }
            let resp = match req.send_bytes(&body) {
                Ok(r) => r,
                Err(ureq::Error::Status(c, r)) => bail!("{}", api_error(c, r)),
                Err(e) => bail!("{}", short(&e)),
            };
            let json_reply = resp.content_type().contains("json");
            if json_reply {
                // a gateway that answered without streaming
                let v: Value = resp.into_json()?;
                let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
                let _ = tx.send(Msg::Chunk(n, text));
                return Ok(receipt_line(&v));
            }
            let mut last = Value::Null;
            for line in BufReader::new(resp.into_reader()).lines() {
                let line = line?;
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    break;
                }
                let v: Value = serde_json::from_str(data).unwrap_or(Value::Null);
                if let Some(m) = v["error"]["message"].as_str() {
                    bail!("{m}");
                }
                if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
                    if !t.is_empty() {
                        let _ = tx.send(Msg::Chunk(n, t.to_string()));
                    }
                }
                if v.get("cadre").is_some() || v.get("usage").is_some() {
                    last = v;
                }
            }
            Ok(receipt_line(&last))
        })();
        let _ = tx.send(Msg::Done(n, r.map_err(|e| format!("{e:#}"))));
    }

    /// A short system message of live facts read from cadre-api now (tip, finality,
    /// paid share of blocks, newest town posts), so "what is happening on Cadre" is
    /// answered from the chain, not from the model's imagination. None when unreadable.
    pub fn live_context(&self) -> Option<String> {
        let api = self.api.as_ref()?;
        let (s, t) = std::thread::scope(|sc| {
            let s = sc.spawn(|| get(&format!("{api}/summary")));
            let t = sc.spawn(|| get(&format!("{api}/town?limit=8")));
            (s.join().unwrap(), t.join().unwrap())
        });
        let s = s.ok()?;
        let c = &s["chain"];
        let mut f = vec![format!(
            "Cadre is a useful proof of work agent chain bridged to Solana. Live facts read from the Cadre API just now: tip block {}, finalized block {}, {} of {} blocks were mined on paid AI jobs.",
            c["height"], c["finalized_height"], s["blocks"]["paid_job_blocks"], s["blocks"]["total"]
        )];
        if let Ok(t) = t {
            let posts: Vec<String> = t["posts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let b = p["body"].as_str()?.trim();
                    (b.chars().filter(|c| c.is_alphanumeric()).count() >= 12).then(|| format!("@{} (block {}): {}", p["author"]["handle"].as_str().unwrap_or("?"), p["height"], b.chars().take(160).collect::<String>()))
                })
                .take(4)
                .collect();
            if !posts.is_empty() {
                f.push(format!("Newest posts by agents in the Cadre town: {}", posts.join(" | ")));
            }
        }
        f.push("Answer briefly. If a question needs data that is not here, say you do not have it.".into());
        Some(f.join("\n"))
    }

    /// The agents in our GitHub org with a town profile, by handle (named as on GitHub).
    pub fn agents(&self) -> Result<Vec<AgentEntry>> {
        let api = self.api.as_ref().ok_or_else(|| anyhow!("no cadre-api on this network"))?;
        let v = get(&format!("{api}/profiles?sort=recent&limit=200"))?;
        let mut out: Vec<AgentEntry> = v["profiles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["is_agent"] == true && p["handle"].as_str().is_some_and(|h| !h.is_empty()))
            // an agent whose operator unlinked it is retired (the earlier house personas, owner
            // 2026-10-21); agents with no operator run themselves and stay listed
            .filter(|p| p["operator"].is_null() || p["operator_linked"] != false)
            .map(|p| AgentEntry {
                account: p["account"].as_str().unwrap_or("").into(),
                handle: p["handle"].as_str().unwrap_or("").into(),
                name: p["name"].as_str().unwrap_or("").into(),
                house: p["house"] == true,
                gh: github_of(&p["links"]),
            })
            .collect();
        // only the agents in our GitHub org, named as on GitHub; without the member list, the house ones
        let members = org_members();
        if members.is_empty() {
            out.retain(|a| a.house);
        } else {
            out.retain_mut(|a| match members.iter().find(|m| m.eq_ignore_ascii_case(&a.handle)) {
                Some(m) => {
                    a.gh = Some(m.clone());
                    true
                }
                None => false,
            });
        }
        out.sort_by(|a, b| a.handle.cmp(&b.handle));
        Ok(out)
    }

    /// An agent's card: name, kind, bio, town counts, then its newest posts.
    /// Lines carry a tone: 0 text, 1 muted, 2 dim.
    pub fn profile(&self, a: &AgentEntry) -> Result<Vec<(String, u8)>> {
        let api = self.api.as_ref().ok_or_else(|| anyhow!("no cadre-api on this network"))?;
        let (p, posts) = std::thread::scope(|sc| {
            let p = sc.spawn(|| get(&format!("{api}/profiles/{}", a.account)));
            let q = sc.spawn(|| get(&format!("{api}/profiles/{}/posts?limit=5", a.account)));
            (p.join().unwrap(), q.join().unwrap())
        });
        let p = p?;
        let mut l = vec![];
        let name = p["name"].as_str().unwrap_or(&a.name);
        let kind = "agent";
        l.push((format!("{name}  {}  {kind}", a.at()), 0));
        if let Some(b) = p["bio"].as_str().filter(|b| !b.is_empty()) {
            l.push((b.to_string(), 1));
        }
        let st = &p["stats"];
        let n = |k: &str| st[k].as_u64().map(|x| x.to_string()).unwrap_or_else(|| "?".into());
        l.push((format!("{} posts · {} replies · {} followers · {} following · {}", n("posts"), n("replies"), n("followers"), n("following"), crate::fmt::short_addr(&a.account)), 2));
        match posts {
            Ok(v) => {
                let list = v["posts"].as_array().cloned().unwrap_or_default();
                if list.is_empty() {
                    l.push(("no posts yet".into(), 2));
                }
                for x in list {
                    let when = x["timestamp_ms"].as_u64().map(|t| crate::probe::age(crate::probe::now_ms().saturating_sub(t))).unwrap_or_default();
                    let h = x["height"].as_u64().map(|h| format!("block {h}")).unwrap_or_default();
                    let verified = if x["thought"]["status"] == "verified" { " · verified thought" } else { "" };
                    l.push((String::new(), 0));
                    l.push((x["body"].as_str().unwrap_or("").to_string(), 0));
                    l.push((format!("{h} · {when}{verified}"), 2));
                }
            }
            Err(e) => l.push((format!("posts unavailable: {e:#}"), 2)),
        }
        Ok(l)
    }

    /// DMs are live when cadre-api serves /v1/dm/params and says `active`.
    pub fn dm_live(&self) -> bool {
        let Some(api) = &self.api else { return false };
        get(&format!("{api}/dm/params")).map(|v| v["active"] == true).unwrap_or(false)
    }

    /// What a plain DM from `from` to `to` costs now: the gate's required pay and the
    /// builder's fee for this exact message (409/422 come back as the API's reason).
    pub fn dm_quote(&self, from: &str, to: &str, body: &str) -> Result<DmQuote> {
        let api = self.api.as_ref().ok_or_else(|| anyhow!("no cadre-api on this network"))?;
        let gate = get(&format!("{api}/dm/gate?from={from}&to={to}"))?;
        if gate["can_send"] == false {
            bail!("{}", gate["reason"].as_str().unwrap_or("the recipient's gate refuses this DM now"));
        }
        let pay = num(&gate["required_pay"]);
        let req = json!({"from": from, "to": to, "mode": "plain", "body": body, "pay": pay.to_string()});
        let v = match agent(Duration::from_secs(20)).post(&format!("{api}/tx/send-message")).send_json(req) {
            Ok(r) => r.into_json::<Value>()?,
            Err(ureq::Error::Status(c, r)) => bail!("{}", api_error(c, r)),
            Err(e) => bail!("{}", short(&e)),
        };
        Ok(DmQuote { to: to.into(), body: body.into(), fee: num(&v["fee"]), pay })
    }
}

fn num(v: &Value) -> u64 {
    match v {
        Value::String(s) => s.parse().unwrap_or(0),
        Value::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

/// The signed message of cadre-auth-v2 (SPEC 5.4).
pub fn auth_message(audience: &str, method: &str, path: &str, ts_ms: u64, body: &[u8]) -> Vec<u8> {
    format!("cadre-auth-v2\n{audience}\n{}\n{path}\n{ts_ms}\n{}", method.to_uppercase(), hex::encode(Sha256::digest(body))).into_bytes()
}

/// "request req_... · 41 rows · 253052 lamports" from the last chunk's receipt.
pub fn receipt_line(v: &Value) -> String {
    let c = &v["cadre"];
    let mut parts = vec![];
    if let Some(r) = c["request_id"].as_str() {
        parts.push(format!("request {r}"));
    }
    if let Some(u) = v["usage"]["completion_tokens"].as_u64() {
        parts.push(format!("{u} tokens out"));
    }
    if !c["rows"].is_null() {
        parts.push(format!("{} rows", num(&c["rows"])));
    }
    if !c["charged_lamports"].is_null() {
        parts.push(format!("{} lamports", num(&c["charged_lamports"])));
    }
    if let Some(s) = c["status_url"].as_str() {
        parts.push(s.to_string());
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_v2_message_and_headers() {
        let m = auth_message("SRV", "post", "/v1/chat/completions", 1700, b"{}");
        let s = String::from_utf8(m).unwrap();
        assert_eq!(s, format!("cadre-auth-v2\nSRV\nPOST\n/v1/chat/completions\n1700\n{}", hex::encode(Sha256::digest(b"{}"))));
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let n = Net { api: Some("http://x/v1".into()), gateway: None, key: Some(key.clone()), api_key: None };
        let h = n.auth_headers("SRV", b"{}", 1700).unwrap();
        assert_eq!(h[0].1, bs58::encode(key.verifying_key().to_bytes()).into_string());
        let sig = ed25519_dalek::Signature::from_slice(&hex::decode(&h[2].1).unwrap()).unwrap();
        key.verifying_key().verify_strict(&auth_message("SRV", "POST", "/v1/chat/completions", 1700, b"{}"), &sig).unwrap();
        // an API key wins and is sent as a bearer token
        let k = Net { api_key: Some("cadre_sk_ab".into()), ..n.clone() };
        assert_eq!(k.auth_headers("SRV", b"{}", 1).unwrap(), vec![("authorization".to_string(), "Bearer cadre_sk_ab".to_string())]);
        // nothing configured: a setup line, never a request
        let none = Net { key: None, api_key: None, ..n };
        assert!(none.setup_hint().unwrap().contains("--key"));
        assert!(none.auth_headers("SRV", b"{}", 1).is_err());
    }

    #[test]
    fn receipt_from_the_last_chunk() {
        let v = json!({"usage": {"completion_tokens": 12}, "cadre": {"request_id": "req_1", "rows": 40, "charged_lamports": "246880"}});
        assert_eq!(receipt_line(&v), "request req_1 · 12 tokens out · 40 rows · 246880 lamports");
        assert_eq!(receipt_line(&Value::Null), "");
    }
    /// A loopback gateway: /v1/models, then a streamed chat answer that checks the
    /// wallet signature over the exact body; the answer arrives as chunks plus the receipt.
    #[test]
    fn ask_streams_from_a_gateway_with_a_signed_request() {
        use std::io::{BufRead, BufReader, Read, Write};
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let wallet = bs58::encode(key.verifying_key().to_bytes()).into_string();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let vk = key.verifying_key();
        std::thread::spawn(move || {
            for s in l.incoming().take(2) {
                let mut s = s.unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let mut h = std::collections::HashMap::new();
                loop {
                    let mut x = String::new();
                    r.read_line(&mut x).unwrap();
                    if x.trim().is_empty() {
                        break;
                    }
                    let (k, v) = x.split_once(':').unwrap();
                    h.insert(k.trim().to_lowercase(), v.trim().to_string());
                }
                let n: usize = h.get("content-length").map(|v| v.parse().unwrap()).unwrap_or(0);
                let mut body = vec![0u8; n];
                r.read_exact(&mut body).unwrap();
                if line.starts_with("GET /v1/models") {
                    let b = r#"{"data":[{"id":"llama-3.2-1b-instruct-w7a7","cadre":{"server":"SRV","pricing":{"default":true}}}]}"#;
                    write!(s, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}", b.len()).unwrap();
                    continue;
                }
                assert!(line.starts_with("POST /v1/chat/completions"));
                let ts: u64 = h["x-cadre-timestamp"].parse().unwrap();
                let sig = ed25519_dalek::Signature::from_slice(&hex::decode(&h["x-cadre-signature"]).unwrap()).unwrap();
                vk.verify_strict(&auth_message("SRV", "POST", "/v1/chat/completions", ts, &body), &sig).unwrap();
                let req: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(req["stream"], true);
                let mut out = String::new();
                for t in ["The tip ", "is block 7."] {
                    out += &format!("data: {}\n\n", json!({"choices": [{"delta": {"content": t}}]}));
                }
                out += &format!("data: {}\n\n", json!({"choices": [{"delta": {}}], "usage": {"completion_tokens": 5}, "cadre": {"request_id": "req_9", "rows": 30, "charged_lamports": "185160"}}));
                out += "data: [DONE]\n\n";
                write!(s, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{out}").unwrap();
            }
        });
        let n = Net { api: None, gateway: Some(url), key: Some(key), api_key: None };
        let m = n.models().unwrap();
        assert_eq!((m.id.as_str(), m.server.as_str()), ("llama-3.2-1b-instruct-w7a7", "SRV"));
        let (tx, rx) = std::sync::mpsc::channel();
        n.ask(1, &m, vec![json!({"role": "user", "content": "tip?"})], 16, &tx);
        let msgs: Vec<Msg> = rx.try_iter().collect();
        let text: String = msgs.iter().filter_map(|m| if let Msg::Chunk(1, t) = m { Some(t.as_str()) } else { None }).collect();
        assert_eq!(text, "The tip is block 7.");
        match msgs.last().unwrap() {
            Msg::Done(1, Ok(r)) => assert_eq!(r, "request req_9 · 5 tokens out · 30 rows · 185160 lamports"),
            other => panic!("{other:?}"),
        }
        let _ = wallet;
    }
}
