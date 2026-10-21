//! `pearl contract ...` (docs/CONTRACTS.md, docs/CLI.md): build a Rust contract to wasm,
//! deploy its code, instantiate it, call and query it, read receipts and events.
//!
//! Arguments use the contract ABI (docs/CONTRACTS.md "ABI"): `TYPE:VALUE` with TYPE one of
//! u8 u16 u32 u64 bool addr hash bytes str cadre (cadre = a CADRE amount, sent as u64 base
//! units). Call and query inputs start with the method name; instantiate and migrate inputs
//! are the fields alone. `--decode u64,addr,str` decodes return data.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};

use agent_types::contracts::{self as ac, contract_address};
use agent_types::tx::{Tx, TxBody};

use crate::ctx::Ctx;
use crate::fmt::short;
use crate::model::Out;
use crate::snapshot::{s, u};
use crate::solana::b58;
use crate::term::{Panel, View};

const UNITS: u64 = 1_000_000_000;

#[derive(Subcommand, Debug, Clone)]
pub enum ContractCmd {
    /// Build the contract crate in DIR (default .) for wasm32-unknown-unknown and check it.
    Build {
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Upload contract code (a .wasm file, or a crate directory to build first).
    Deploy { path: PathBuf },
    /// Create a contract from deployed code: CODE_HASH [TYPE:VALUE ...].
    Instantiate {
        code_hash: String,
        args: Vec<String>,
        #[arg(long, default_value = "")]
        label: String,
        /// Who may migrate it: an address, "me" (default), or "none" (immutable).
        #[arg(long, default_value = "me")]
        admin: String,
        /// CADRE sent to the contract.
        #[arg(long, default_value = "0")]
        value: String,
        #[arg(long)]
        gas: Option<u64>,
    },
    /// Deploy (if needed) and instantiate in one go: FILE_OR_DIR [TYPE:VALUE ...].
    New {
        path: PathBuf,
        args: Vec<String>,
        #[arg(long, default_value = "")]
        label: String,
        #[arg(long, default_value = "me")]
        admin: String,
        #[arg(long, default_value = "0")]
        value: String,
    },
    /// Call: ADDRESS METHOD [TYPE:VALUE ...]. Simulates first to set the gas limit.
    Call {
        address: String,
        method: String,
        args: Vec<String>,
        #[arg(long, default_value = "0")]
        value: String,
        /// Gas limit (default: the simulated gas plus 25%).
        #[arg(long)]
        gas: Option<u64>,
        /// Only simulate (nothing is sent; shows gas, return data, events and logs).
        #[arg(long)]
        dry_run: bool,
        /// Decode the return data: comma-separated types.
        #[arg(long)]
        decode: Option<String>,
    },
    /// Read-only query: ADDRESS METHOD [TYPE:VALUE ...].
    Query {
        address: String,
        method: String,
        args: Vec<String>,
        #[arg(long)]
        decode: Option<String>,
    },
    /// One contract: code, admin, storage, deposit, calls.
    Show { address: String },
    /// Contracts on the chain (newest first).
    #[command(alias = "list")]
    Ls {
        #[arg(long, default_value_t = 20)]
        limit: u64,
    },
    /// A contract's events (newest first).
    Events {
        address: String,
        #[arg(long, default_value_t = 20)]
        limit: u64,
    },
    /// A contract tx's receipt.
    Receipt { tx_hash: String },
    /// Swap a contract's code (its admin only): ADDRESS CODE_HASH [TYPE:VALUE ...].
    Migrate { address: String, code_hash: String, args: Vec<String> },
    /// Change a contract's admin (its admin only): ADDRESS NEW_ADMIN|none.
    Admin { address: String, admin: String },
}

fn addr(s: &str) -> Result<[u8; 32]> {
    crate::fmt::parse_chain_addr(s)
}

fn hash32(s: &str) -> Result<[u8; 32]> {
    let t = s.trim().trim_start_matches("0x");
    if t.len() == 64 {
        return Ok(hex::decode(t)?.try_into().unwrap());
    }
    addr(s)
}

/// A CADRE amount ("1.5") in base units.
pub fn cadre(s: &str) -> Result<u64> {
    let (w, f) = s.split_once('.').unwrap_or((s, ""));
    ensure!(f.len() <= 9 && w.chars().chain(f.chars()).all(|c| c.is_ascii_digit()), "bad CADRE amount {s:?}");
    let w: u64 = if w.is_empty() { 0 } else { w.parse()? };
    let f: u64 = if f.is_empty() { 0 } else { format!("{f:0<9}").parse()? };
    w.checked_mul(UNITS).and_then(|x| x.checked_add(f)).ok_or_else(|| anyhow!("amount overflow"))
}

/// Encode `TYPE:VALUE` fields (docs/CONTRACTS.md "ABI").
pub fn encode_fields(args: &[String]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for a in args {
        let (t, v) = a.split_once(':').ok_or_else(|| anyhow!("argument {a:?} is not TYPE:VALUE"))?;
        match t {
            "u8" => out.push(v.parse::<u8>()?),
            "u16" => out.extend_from_slice(&v.parse::<u16>()?.to_le_bytes()),
            "u32" => out.extend_from_slice(&v.parse::<u32>()?.to_le_bytes()),
            "u64" => out.extend_from_slice(&v.parse::<u64>()?.to_le_bytes()),
            "cadre" => out.extend_from_slice(&cadre(v)?.to_le_bytes()),
            "bool" => out.push(match v {
                "true" | "1" => 1,
                "false" | "0" => 0,
                _ => bail!("bool is true or false"),
            }),
            "addr" => out.extend_from_slice(&addr(v)?),
            "hash" => out.extend_from_slice(&hash32(v)?),
            "bytes" => {
                let b = hex::decode(v.trim_start_matches("0x"))?;
                out.extend_from_slice(&(b.len() as u32).to_le_bytes());
                out.extend_from_slice(&b);
            }
            "str" => {
                out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                out.extend_from_slice(v.as_bytes());
            }
            _ => bail!("unknown argument type {t:?} (u8 u16 u32 u64 bool addr hash bytes str cadre)"),
        }
    }
    Ok(out)
}

/// `method (u8 len) || fields`.
pub fn encode_call(method: &str, args: &[String]) -> Result<Vec<u8>> {
    ensure!(!method.is_empty() && method.len() < 256, "method name 1..=255 bytes");
    let mut v = vec![method.len() as u8];
    v.extend_from_slice(method.as_bytes());
    v.extend(encode_fields(args)?);
    Ok(v)
}

/// Decode return data with a comma-separated type list.
pub fn decode_fields(types: &str, b: &[u8]) -> Result<Value> {
    let mut pos = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        ensure!(b.len() - pos >= n, "return data too short for {types:?}");
        let s = &b[pos..pos + n];
        pos += n;
        Ok(s)
    };
    let mut out = Vec::new();
    for t in types.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let v = match t {
            "u8" => json!(take(1)?[0]),
            "u16" => json!(u16::from_le_bytes(take(2)?.try_into()?)),
            "u32" => json!(u32::from_le_bytes(take(4)?.try_into()?)),
            "u64" => json!(u64::from_le_bytes(take(8)?.try_into()?).to_string()),
            "cadre" => {
                let x = u64::from_le_bytes(take(8)?.try_into()?);
                json!(format!("{}.{:09}", x / UNITS, x % UNITS))
            }
            "bool" => json!(take(1)?[0] == 1),
            "addr" => json!(bs58::encode(take(32)?).into_string()),
            "hash" => json!(hex::encode(take(32)?)),
            "bytes" | "str" => {
                let n = u32::from_le_bytes(take(4)?.try_into()?) as usize;
                let d = take(n)?;
                if t == "str" { json!(String::from_utf8_lossy(d)) } else { json!(hex::encode(d)) }
            }
            _ => bail!("unknown decode type {t:?}"),
        };
        out.push(v);
    }
    Ok(Value::Array(out))
}

fn params(ctx: &Ctx) -> Result<(Value, u32)> {
    let p = ctx.call("getParams", json!({}))?;
    let info = ctx.call("getChainInfo", json!({}))?;
    Ok((p, u(&info, "chain_id") as u32))
}

fn num(v: &Value, k: &str) -> u64 {
    match &v[k] {
        Value::String(x) => x.parse().unwrap_or(0),
        Value::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

/// The contract fee floor (docs/CONTRACTS.md "Gas") for `body` sent by the connected key.
fn fee_for(ctx: &Ctx, p: &Value, chain_id: u32, body: &TxBody, gas_limit: u64) -> Result<u64> {
    let key = ctx.key()?;
    let len = Tx::signed(&key.to_bytes(), chain_id, 0, 0, body.clone()).encode().len() as u64;
    let gas_fee = (gas_limit as u128 * num(p, "gas_price") as u128).div_ceil(num(p, "gas_price_unit").max(1) as u128) as u64;
    Ok((num(p, "contract_base_fee") + num(p, "contract_byte_fee") * len + gas_fee).max(num(p, "min_fee")))
}

fn send(ctx: &Ctx, what: &str, body: TxBody, gas_limit: u64) -> Result<(crate::commands::Sent, Value)> {
    let (p, chain_id) = params(ctx)?;
    let fee = fee_for(ctx, &p, chain_id, &body, gas_limit)?;
    let sent = crate::commands::send_body(ctx, what, body, Some(fee), None, false)?;
    let receipt = if sent.height.is_some() { ctx.call("getContractReceipt", json!({"tx_hash": sent.tx_hash}))? } else { Value::Null };
    Ok((sent, receipt))
}

fn receipt_panel(title: &str, r: &Value) -> Panel {
    let mut p = Panel::new(title)
        .state("Status", s(r, "status"))
        .kv("Gas", format!("{} of {}", s(r, "gas_used"), s(r, "gas_limit")))
        .kv("Fee charged", s(r, "fee_charged"));
    if !s(r, "error").is_empty() {
        p = p.kv("Error", s(r, "error"));
    }
    if !s(r, "return").is_empty() {
        p = p.kv("Return", s(r, "return"));
    }
    if let Some(evs) = r["events"].as_array() {
        if !evs.is_empty() {
            p = p.kv("Events", evs.len().to_string());
        }
    }
    p
}

/// Build the crate in `dir`; returns the .wasm path.
fn build(ctx: &Ctx, dir: &Path) -> Result<PathBuf> {
    let st = ctx.ui.step(format!("Building {} for wasm32-unknown-unknown", dir.display()));
    let r = (|| -> Result<PathBuf> {
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).with_context(|| format!("no Cargo.toml in {}", dir.display()))?;
        let name = manifest
            .lines()
            .skip_while(|l| l.trim() != "[package]")
            .find_map(|l| l.trim().strip_prefix("name").map(|r| r.trim().trim_start_matches('=').trim().trim_matches('"').to_string()))
            .ok_or_else(|| anyhow!("no package name in Cargo.toml"))?;
        let out = Command::new("cargo")
            .args(["build", "--release", "--target", "wasm32-unknown-unknown"])
            .current_dir(dir)
            .output()
            .context("running cargo (install Rust and `rustup target add wasm32-unknown-unknown`)")?;
        if !out.status.success() {
            bail!("cargo build failed:\n{}", String::from_utf8_lossy(&out.stderr).lines().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
        }
        let meta = Command::new("cargo").args(["metadata", "--format-version", "1", "--no-deps"]).current_dir(dir).output()?;
        let m: Value = serde_json::from_slice(&meta.stdout)?;
        let target = m["target_directory"].as_str().map(PathBuf::from).unwrap_or_else(|| dir.join("target"));
        let wasm = target.join("wasm32-unknown-unknown/release").join(format!("{}.wasm", name.replace('-', "_")));
        ensure!(wasm.exists(), "built, but {} is missing (is the crate a cdylib?)", wasm.display());
        Ok(wasm)
    })();
    st.wrap(r, |p| format!("Built {}", p.display()))
}

fn load_code(ctx: &Ctx, path: &Path) -> Result<Vec<u8>> {
    let wasm = if path.is_dir() { build(ctx, path)? } else { path.to_path_buf() };
    let code = std::fs::read(&wasm).with_context(|| format!("reading {}", wasm.display()))?;
    let st = ctx.ui.step("Checking the module against the chain's rules");
    let r = contracts::Vm::new().validate(&code, ac::MAX_CODE_SIZE).map(|(_, i)| i).map_err(|e| anyhow!(e));
    st.wrap(r, |i| format!("Valid: {} bytes, entry points {}", code.len(), i.entries.join(", ")))?;
    Ok(code)
}

fn me(ctx: &Ctx) -> Result<[u8; 32]> {
    Ok(ctx.key()?.verifying_key().to_bytes())
}

fn admin_arg(ctx: &Ctx, a: &str) -> Result<[u8; 32]> {
    match a {
        "me" => me(ctx),
        "none" | "" => Ok([0; 32]),
        x => addr(x),
    }
}

fn deploy(ctx: &Ctx, code: Vec<u8>) -> Result<(String, Value)> {
    let h = hex::encode(ac::code_hash(&code));
    if !ctx.call("getCode", json!({"code_hash": h}))?.is_null() {
        ctx.ui.done(&format!("Code {} is already deployed", short(&h)));
        return Ok((h, Value::Null));
    }
    let gas = contracts::deploy_cost(code.len());
    let (sent, r) = send(ctx, "DeployCode", TxBody::DeployCode(ac::DeployCode { gas_limit: gas, code }), gas)?;
    Ok((h, json!({"tx_hash": sent.tx_hash, "height": sent.height, "receipt": r})))
}

fn instantiate(ctx: &Ctx, code_hash: &str, fields: Vec<u8>, label: &str, admin: [u8; 32], value: u64, gas: Option<u64>) -> Result<Out> {
    let ch = hash32(code_hash)?;
    let gas = gas.unwrap_or(50_000_000);
    let body = TxBody::Instantiate(ac::Instantiate { code_hash: ch, admin, value, gas_limit: gas, label: label.as_bytes().to_vec(), args: fields });
    let (sent, r) = send(ctx, "Instantiate", body, gas)?;
    let address = b58(&contract_address(&me(ctx)?, sent.nonce));
    let p = receipt_panel("Contract", &r).kv("Address", crate::fmt::cx_str(&address)).kv("Code", hex::encode(ch)).kv("Tx", sent.tx_hash.as_str());
    Ok(Out { json: json!({"address": address, "tx_hash": sent.tx_hash, "receipt": r}), text: crate::fmt::cx_str(&address), view: View::panel(p) })
}

pub fn exec(ctx: &Ctx, cmd: &ContractCmd) -> Result<Out> {
    match cmd {
        ContractCmd::Build { dir } => {
            let wasm = build(ctx, dir)?;
            let code = load_code(ctx, &wasm)?;
            let h = hex::encode(ac::code_hash(&code));
            let p = Panel::new("Contract code").kv("File", wasm.display().to_string()).kv("Size", format!("{} bytes", code.len())).kv("Code hash", h.as_str());
            Ok(Out { json: json!({"wasm": wasm, "size": code.len(), "code_hash": h}), text: h, view: View::panel(p) })
        }
        ContractCmd::Deploy { path } => {
            let code = load_code(ctx, path)?;
            let (h, v) = deploy(ctx, code)?;
            let p = Panel::new("Code").kv("Code hash", h.as_str());
            Ok(Out { json: json!({"code_hash": h, "deploy": v}), text: h, view: View::panel(p) })
        }
        ContractCmd::Instantiate { code_hash, args, label, admin, value, gas } => {
            instantiate(ctx, code_hash, encode_fields(args)?, label, admin_arg(ctx, admin)?, cadre(value)?, *gas)
        }
        ContractCmd::New { path, args, label, admin, value } => {
            let code = load_code(ctx, path)?;
            let (h, _) = deploy(ctx, code)?;
            instantiate(ctx, &h, encode_fields(args)?, label, admin_arg(ctx, admin)?, cadre(value)?, None)
        }
        ContractCmd::Call { address, method, args, value, gas, dry_run, decode } => {
            let a = addr(address)?;
            let input = encode_call(method, args)?;
            let value = cadre(value)?;
            let st = ctx.ui.step("Simulating the call");
            let sim = ctx.call("simulateCall", json!({"from": b58(&me(ctx)?), "address": address, "args": hex::encode(&input), "value": value}));
            let sim = st.wrap(sim, |v| format!("Simulated: {}, {} gas", if v["ok"] == json!(true) { "ok" } else { "fails" }, s(v, "gas_used")))?;
            if *dry_run || sim["ok"] != json!(true) {
                ensure!(*dry_run, "the call would fail: {}", s(&sim, "error"));
                let mut p = Panel::new("Simulation").kv("Gas", s(&sim, "gas_used")).kv("Return", s(&sim, "return"));
                if !s(&sim, "log").is_empty() {
                    p = p.kv("Log", s(&sim, "log"));
                }
                let mut j = sim.clone();
                if let Some(t) = decode {
                    j["decoded"] = decode_fields(t, &hex::decode(s(&sim, "return"))?)?;
                }
                return Ok(Out { json: j, text: s(&sim, "return").to_string(), view: View::panel(p) });
            }
            let used = num(&sim, "gas_used");
            let gas = gas.unwrap_or(used + used / 4 + 100_000);
            let (sent, r) = send(ctx, &format!("Call {method}"), TxBody::Call(ac::Call { contract: a, value, gas_limit: gas, args: input }), gas)?;
            let mut j = json!({"tx_hash": sent.tx_hash, "height": sent.height, "receipt": r});
            if let (Some(t), false) = (decode, r.is_null()) {
                j["decoded"] = decode_fields(t, &hex::decode(s(&r, "return"))?)?;
            }
            let text = if r.is_null() { sent.tx_hash.clone() } else { format!("{} {} gas", s(&r, "status"), s(&r, "gas_used")) };
            Ok(Out { json: j, text, view: View::panel(receipt_panel("Call", &r).kv("Tx", sent.tx_hash.as_str())) })
        }
        ContractCmd::Query { address, method, args, decode } => {
            let input = encode_call(method, args)?;
            let v = ctx.call("queryContract", json!({"address": address, "args": hex::encode(&input)}))?;
            ensure!(v["ok"] == json!(true), "query failed: {}", s(&v, "error"));
            let mut j = v.clone();
            let mut text = s(&v, "return").to_string();
            if let Some(t) = decode {
                let d = decode_fields(t, &hex::decode(s(&v, "return"))?)?;
                text = d.as_array().unwrap().iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(" ");
                j["decoded"] = d;
            }
            let p = Panel::new("Query").kv("Return", text.as_str()).kv("Gas", s(&v, "gas_used"));
            Ok(Out { json: j, text, view: View::panel(p) })
        }
        ContractCmd::Show { address } => {
            let c = ctx.call("getContract", json!({"address": address}))?;
            ensure!(!c.is_null(), "no contract at {address}");
            let p = Panel::new("Contract")
                .kv("Address", crate::fmt::cx_str(s(&c, "address")))
                .kv("Label", s(&c, "label"))
                .kv("Code", s(&c, "code_hash"))
                .kv("Admin", if c["admin"].is_null() { "none (immutable)".to_string() } else { crate::fmt::cx_str(s(&c, "admin")) })
                .kv("Creator", crate::fmt::cx_str(s(&c, "creator")))
                .kv("Created at", u(&c, "created_height").to_string())
                .kv("Balance", s(&c, "balance"))
                .kv("Storage", format!("{} items, {} bytes, deposit {}", u(&c, "storage_items"), u(&c, "storage_bytes"), s(&c, "deposit")))
                .kv("Calls / events", format!("{} / {}", u(&c, "calls"), u(&c, "events")));
            Ok(Out { text: s(&c, "address").to_string(), json: c, view: View::panel(p) })
        }
        ContractCmd::Ls { limit } => {
            let l = ctx.call("listContracts", json!({"limit": limit}))?;
            let lines: Vec<String> = l["items"].as_array().into_iter().flatten().map(|c| format!("{}  {}  calls {}  {}", crate::fmt::cx_str(s(c, "address")), s(c, "label"), u(c, "calls"), short(s(c, "code_hash")))).collect();
            let p = Panel::new("Contracts").kv("Total", u(&l, "total").to_string());
            Ok(Out { json: l, text: lines.join("\n"), view: View::panel(p).hint(lines.join("\n")) })
        }
        ContractCmd::Events { address, limit } => {
            let l = ctx.call("getContractEvents", json!({"address": address, "limit": limit}))?;
            let lines: Vec<String> = l["events"].as_array().into_iter().flatten().map(|e| format!("#{} block {} tx {} topics {} data {}", u(e, "n"), u(e, "height"), short(s(e, "tx_hash")), e["topics"].as_array().map(|t| t.len()).unwrap_or(0), s(e, "data"))).collect();
            let p = Panel::new("Events").kv("Total", u(&l, "total").to_string());
            Ok(Out { json: l, text: lines.join("\n"), view: View::panel(p).hint(lines.join("\n")) })
        }
        ContractCmd::Receipt { tx_hash } => {
            let r = ctx.call("getContractReceipt", json!({"tx_hash": tx_hash}))?;
            ensure!(!r.is_null(), "no contract receipt for {tx_hash}");
            Ok(Out { text: s(&r, "status").to_string(), view: View::panel(receipt_panel("Receipt", &r).kv("Block", u(&r, "height").to_string())), json: r })
        }
        ContractCmd::Migrate { address, code_hash, args } => {
            let gas = 50_000_000;
            let body = TxBody::Migrate(ac::Migrate { contract: addr(address)?, code_hash: hash32(code_hash)?, gas_limit: gas, args: encode_fields(args)? });
            let (sent, r) = send(ctx, "Migrate", body, gas)?;
            Ok(Out { json: json!({"tx_hash": sent.tx_hash, "receipt": r}), text: s(&r, "status").to_string(), view: View::panel(receipt_panel("Migrate", &r)) })
        }
        ContractCmd::Admin { address, admin } => {
            let body = TxBody::SetContractAdmin { contract: addr(address)?, admin: admin_arg(ctx, admin)? };
            let (sent, r) = send(ctx, "SetContractAdmin", body, 0)?;
            Ok(Out { json: json!({"tx_hash": sent.tx_hash, "receipt": r}), text: s(&r, "status").to_string(), view: View::panel(receipt_panel("Admin", &r)) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_matches_the_chain_vector() {
        // chain/agent-types/vectors/contracts.json call_args
        let v = encode_call("increment", &["u64:5".into()]).unwrap();
        assert_eq!(hex::encode(v), "09696e6372656d656e740500000000000000");
        assert_eq!(cadre("1.5").unwrap(), 1_500_000_000);
        assert_eq!(cadre("2").unwrap(), 2 * UNITS);
        assert!(cadre("1.0000000001").is_err());
        let f = encode_fields(&["str:hi".into(), "bool:true".into(), "u8:7".into()]).unwrap();
        assert_eq!(f, vec![2, 0, 0, 0, b'h', b'i', 1, 7]);
        let d = decode_fields("str,bool,u8", &f).unwrap();
        assert_eq!(d, json!(["hi", true, 7]));
        assert!(encode_fields(&["x:1".into()]).is_err());
    }
}
