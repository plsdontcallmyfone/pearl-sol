//! Every endpoint `status`, `ping` and the read commands talk to, per network profile
//! (docs/DEVNET.md "Endpoints", the deploy/systemd units' ports, the CLAUDE.md port plan),
//! overridable one by one with `--endpoint NAME=URL` or `CADRE_<NAME>` (for example
//! `CADRE_API=https://...`). An endpoint that is not set is reported as "skip", never guessed.

use std::collections::BTreeMap;

use crate::ctx::{Network, Opts};

/// Names `--endpoint NAME=URL` accepts, with what they are.
pub const NAMES: &[(&str, &str)] = &[
    ("nodes", "agent-chain node JSON-RPC URLs, comma separated (the first is --rpc)"),
    ("solana", "Solana JSON-RPC (--sol-rpc)"),
    ("api", "cadre-api base, ending in /v1"),
    ("explorer", "explorer (JSON under /api)"),
    ("storefront", "storefront-x402 (/health, /quote)"),
    ("relayer", "relayer (/health)"),
    ("site", "the Cadre website"),
    ("docs", "the Cadre docs site"),
    ("gateway", "cadre-gateway (/health, /v1/stats); box-local"),
    ("worker", "inference worker (/health); box-local"),
    ("attester", "pearl-attester (/health, /proofs/<ticket>); box-local"),
    ("swarm", "house swarm status (/health, /status); box-local"),
    ("desktops", "house desktops controller (/health); box-local"),
    ("agents", "user-agent runtime cadre-agents (/health); box-local"),
    ("pool", "mining pool (/stats)"),
];

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub network: Network,
    pub nodes: Vec<String>,
    pub solana: String,
    /// What to print for `solana` (a dedicated RPC URL may carry an API key: never printed).
    pub solana_shown: String,
    pub urls: BTreeMap<String, String>,
    /// header-relay fee payers (public keys) whose SOL balance is checked.
    pub relay_payers: Vec<String>,
    /// The CADRE mint on Solana when the profile knows it (else read from the bridge Config).
    pub mint: Option<String>,
}

pub const DEDICATED_RPC_FILE: &str = "/etc/lode/solana-rpc-url";

const DEVNET_NODES: [&str; 3] = ["https://rpc.64-95-12-116.sslip.io", "https://rpc.64-95-13-213.sslip.io", "https://rpc.64-95-12-22.sslip.io"];

/// Box-local services (127.0.0.1, deploy/systemd units and the port plan in CLAUDE.md).
const LOCAL: &[(&str, &str)] = &[
    ("gateway", "http://127.0.0.1:10664"),
    ("worker", "http://127.0.0.1:10665"),
    ("attester", "http://127.0.0.1:10661"),
    ("swarm", "http://127.0.0.1:10668"),
    ("desktops", "http://127.0.0.1:10720"),
    ("agents", "http://127.0.0.1:10730"),
];

impl Endpoints {
    pub fn resolve(o: &Opts, rpc: &str, sol_rpc: &str, pool: Option<&str>) -> Self {
        let mut urls = BTreeMap::new();
        let mut put = |k: &str, v: &str| {
            urls.insert(k.to_string(), v.trim_end_matches('/').to_string());
        };
        let mut nodes: Vec<String> = vec![rpc.to_string()];
        let (mut relay_payers, mut mint) = (vec![], None);
        match o.network {
            Network::Devnet => {
                for n in DEVNET_NODES {
                    if !nodes.iter().any(|x| x.trim_end_matches('/') == n) {
                        nodes.push(n.into());
                    }
                }
                put("api", "https://api.64-95-12-116.sslip.io/v1");
                put("explorer", "https://explorer.64-95-12-116.sslip.io");
                put("storefront", "https://storefront.64-95-12-116.sslip.io");
                put("relayer", "https://relayer.64-95-12-116.sslip.io");
                put("site", "https://cadre.64-95-12-116.sslip.io");
                relay_payers = vec!["XQUUCs5tZaJrf4VMuKJRmUbA2gEV2M7uKWTQiSTe6Cy".into(), "E66ekRjfD9tjquu5F4qyRHt3y7AZhM9hsXY63KV4Ug7L".into()];
                mint = Some("BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e".into());
            }
            Network::Testbox => {
                put("api", "https://cadre-api.64-95-13-106.sslip.io/v1");
                put("explorer", "http://127.0.0.1:10670");
                put("storefront", "http://127.0.0.1:10662");
                put("relayer", "http://127.0.0.1:10660");
            }
            Network::Localnet => {
                put("api", "http://127.0.0.1:10685/v1");
                put("explorer", "http://127.0.0.1:10670");
                put("storefront", "http://127.0.0.1:10662");
                put("relayer", "http://127.0.0.1:10660");
                put("site", "http://127.0.0.1:10710");
                put("docs", "http://127.0.0.1:10675");
            }
        }
        // on the box itself (cadre-health.timer on box 0, or the test box), the 127.0.0.1 services
        if o.local || o.network != Network::Devnet {
            for (k, v) in LOCAL {
                put(k, v);
            }
        }
        if let Some(p) = pool {
            put("pool", p);
        }
        // env, then flags
        for (k, _) in NAMES {
            if let Ok(v) = std::env::var(format!("CADRE_{}", k.to_ascii_uppercase())) {
                if !v.is_empty() {
                    apply(k, &v, &mut nodes, &mut urls);
                }
            }
        }
        if let Ok(v) = std::env::var("PEARL_EXPLORER") {
            if !v.is_empty() {
                apply("explorer", &v, &mut nodes, &mut urls);
            }
        }
        for e in &o.endpoint {
            if let Some((k, v)) = e.split_once('=') {
                apply(k.trim(), v.trim(), &mut nodes, &mut urls);
            }
        }
        let mut solana = urls.remove("solana").unwrap_or_else(|| sol_rpc.to_string());
        let mut solana_shown = solana.clone();
        // on a devnet box: the dedicated Solana RPC the services use (scripts/devnet-health.sh),
        // unless one was given; its URL is a secret and is shown by its file name only
        if o.local && o.sol_rpc.is_none() && !o.endpoint.iter().any(|e| e.starts_with("solana=")) {
            if let Ok(f) = std::fs::read_to_string(DEDICATED_RPC_FILE) {
                let f = f.trim();
                if f.starts_with("http") {
                    solana = f.to_string();
                    solana_shown = format!("dedicated RPC ({DEDICATED_RPC_FILE})");
                }
            }
        }
        if let Ok(v) = std::env::var("CADRE_RELAY_PAYERS") {
            relay_payers = v.split([',', ' ']).filter(|x| !x.is_empty()).map(String::from).collect();
        }
        Endpoints { network: o.network, nodes, solana, solana_shown, urls, relay_payers, mint }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.urls.get(name).map(|s| s.as_str())
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"network": format!("{:?}", self.network).to_lowercase(), "nodes": self.nodes, "solana": self.solana_shown, "urls": self.urls, "relay_payers": self.relay_payers, "mint": self.mint})
    }
}

fn apply(k: &str, v: &str, nodes: &mut Vec<String>, urls: &mut BTreeMap<String, String>) {
    if k == "nodes" {
        *nodes = v.split(',').map(|x| x.trim().trim_end_matches('/').to_string()).filter(|x| !x.is_empty()).collect();
    } else if v == "none" || v.is_empty() {
        urls.remove(k);
    } else {
        urls.insert(k.to_string(), v.trim_end_matches('/').to_string());
    }
}

/// Validate `--endpoint` values early (clap value parser).
pub fn parse_endpoint(s: &str) -> Result<String, String> {
    let (k, v) = s.split_once('=').ok_or_else(|| format!("expected NAME=URL, got {s:?}"))?;
    if !NAMES.iter().any(|(n, _)| *n == k.trim()) {
        return Err(format!("unknown endpoint {k:?}; known: {}", NAMES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")));
    }
    if v.trim().is_empty() {
        return Err("empty URL (use none to unset)".into());
    }
    Ok(s.to_string())
}

/// Short display name of a node URL: `rpc.64-95-12-116.sslip.io` -> `64-95-12-116`.
pub fn node_name(url: &str) -> String {
    let h = url.trim_start_matches("https://").trim_start_matches("http://");
    let h = h.split('/').next().unwrap_or(h);
    let h = h.strip_prefix("rpc.").unwrap_or(h);
    h.strip_suffix(".sslip.io").unwrap_or(h).to_string()
}
