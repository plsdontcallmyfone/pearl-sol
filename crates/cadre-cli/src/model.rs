//! `cadre model ...` and `cadre ticket`: the model registry and inference tickets
//! (SPEC 4.5). Registration hashes every registered matrix (hash_b under its fixed job
//! key), chunks every file into blobs, uploads the manifest, sends RegisterModel and waits for
//! it, then uploads the chunks (putBlob push false, several in parallel). Once the model is
//! registered every node that holds its manifest pins the chunks the manifest lists, so an
//! upload may take as long as it needs (SPEC 4.5 "Upload order"). Reads are getModel /
//! listModels / getTicket.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Result};
use base64::Engine as _;
use clap::Subcommand;
use serde_json::{json, Value};

use agent_types::tx::TxBody;
use agent_types::{MatrixSpec, RegisterModel};
use cadre_job::standing::{self, Manifest};

use crate::ctx::Ctx;
use crate::fmt::short;
use crate::snapshot::{s, u};
use crate::solana::b58;
use crate::term::{Block, Panel, Table, Tone, View};

#[derive(Subcommand, Debug, Clone)]
pub enum ModelCmd {
    /// Registered models.
    #[command(alias = "list")]
    Ls,
    /// One model: matrices, totals.
    Show { id: String },
    /// Register a quantized model directory (tools/cadre/quantize.py output) and upload its blobs.
    Register {
        dir: PathBuf,
        /// Registry name (default: the manifest's).
        #[arg(long)]
        name: Option<String>,
        /// Only compute hashes and the manifest; send nothing.
        #[arg(long)]
        dry_run: bool,
        /// Link the directory into this model store as <store>/<model_id hex> (miners, attester, gateway).
        #[arg(long)]
        store: Option<PathBuf>,
        /// Chunk uploads in flight at once (the node's putBlob rate limit still applies).
        #[arg(long, default_value_t = 4)]
        parallel: usize,
    },
    /// Rebuild a registered model's directory from the node's blobs (checks every file hash and hash_b).
    Fetch { id: String, dir: PathBuf },
}

pub struct Out {
    pub json: Value,
    pub text: String,
    pub view: View,
}

fn model_id_arg(ctx: &Ctx, id: &str) -> Result<String> {
    let id = id.trim().trim_start_matches("0x");
    if id.len() == 64 {
        return Ok(id.to_lowercase());
    }
    // a prefix: match against the registry
    let l = ctx.call("listModels", json!({}))?;
    let hits: Vec<String> = l["models"].as_array().into_iter().flatten().filter_map(|m| m["model_id"].as_str()).filter(|m| m.starts_with(id)).map(String::from).collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => bail!("no registered model matches {id:?}"),
        _ => bail!("{id:?} matches {} models", hits.len()),
    }
}

pub fn exec(ctx: &Ctx, cmd: &ModelCmd) -> Result<Out> {
    match cmd {
        ModelCmd::Ls => {
            let l = ctx.call("listModels", json!({}))?;
            let ms = l["models"].as_array().cloned().unwrap_or_default();
            let mut rows = Vec::new();
            let mut lines = Vec::new();
            for m in &ms {
                let row = [short(s(m, "model_id")), s(m, "name").to_string(), u(m, "matrix_count").to_string(), s(m, "macs_per_row").to_string(),
                    u(m, "tickets").to_string(), u(m, "segments").to_string(), u(m, "blocks_mined").to_string(), tk(num(m, "subsidy_paid"))];
                lines.push(row.join("  "));
                rows.push(row.to_vec());
            }
            let t = table("Models", &["Model", "Name", "Matrices", "MACs / row", "Tickets", "Segments", "Blocks", "Subsidy"], rows, "no registered models");
            let text = if lines.is_empty() { "no registered models".into() } else { lines.join("\n") };
            Ok(Out { json: l, text, view: View::panel(Panel::new("Models").kv("Registered", ms.len().to_string())).with(Block::Table(t)) })
        }
        ModelCmd::Show { id } => {
            let id = model_id_arg(ctx, id)?;
            let m = ctx.call("getModel", json!({"model_id": id}))?;
            ensure!(!m.is_null(), "model {id} is not registered");
            let p = model_panel(&m);
            let rows = m["matrices"].as_array().into_iter().flatten().take(200)
                .map(|x| vec![u(x, "index").to_string(), u(x, "k").to_string(), u(x, "n").to_string(), s(x, "macs").to_string(), short(s(x, "hash_b"))])
                .collect();
            let t = table("Matrices", &["#", "k", "n", "MACs", "hash_b"], rows, "no matrices");
            let text = format!("{} {} matrices {} macs/row {} tickets {} blocks {}", s(&m, "name"), short(&id), u(&m, "matrix_count"), s(&m, "macs_per_row"), u(&m, "tickets"), u(&m, "blocks_mined"));
            Ok(Out { json: m, text, view: View::panel(p).with(Block::Table(t)) })
        }
        ModelCmd::Register { dir, name, dry_run, store, parallel } => register(ctx, dir, name.as_deref(), *dry_run, store.as_deref(), *parallel),
        ModelCmd::Fetch { id, dir } => fetch(ctx, &model_id_arg(ctx, id)?, dir),
    }
}

fn table(chip: &str, head: &[&str], rows: Vec<Vec<String>>, empty: &str) -> Table {
    Table {
        chip: Some(chip.into()),
        head: head.iter().map(|h| h.to_uppercase()).collect(),
        right: head.iter().map(|_| false).collect(),
        rows: rows.into_iter().map(|r| r.into_iter().map(|c| (c, Tone::Plain)).collect()).collect(),
        empty: empty.into(),
    }
}

fn tk(units: u64) -> String {
    format!("{} {}", crate::fmt::tokens(units as u128), crate::brand::TICKER)
}

fn num(v: &Value, k: &str) -> u64 {
    match &v[k] {
        Value::String(x) => x.parse().unwrap_or(0),
        Value::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

fn model_panel(m: &Value) -> Panel {
    Panel::new("Model")
        .kv("Id", s(m, "model_id"))
        .kv("Name", s(m, "name"))
        .kv("Registrant", s(m, "registrant"))
        .kv("Recipe", s(m, "recipe_name"))
        .kv("Matrices", u(m, "matrix_count").to_string())
        .kv("MACs per row", s(m, "macs_per_row"))
        .kv("Manifest", s(m, "manifest_uri"))
        .kv("Registered at", u(m, "registered_height").to_string())
        .kv("Tickets", u(m, "tickets").to_string())
        .kv("Segments", u(m, "segments").to_string())
        .kv("Segment blocks", u(m, "blocks_mined").to_string())
        .kv("Subsidy", tk(num(m, "subsidy_paid")))
}

/// hash_b of every matrix and the files map; writes the completed manifest.json.
pub fn prepare(dir: &Path, registrant: &[u8; 32], name: &str) -> Result<(Manifest, Vec<u8>)> {
    let mut man = Manifest::load(dir)?;
    ensure!(man.format == standing::MANIFEST_FORMAT, "manifest format {:?} is not {}", man.format, standing::MANIFEST_FORMAT);
    ensure!(!man.matrices.is_empty(), "no matrices in the manifest");
    man.name = name.to_string();
    let id = standing::model_id(registrant, name.as_bytes());
    let base = man.base_config()?;
    for mx in man.matrices.iter_mut() {
        let p = dir.join(&mx.weights);
        let raw = std::fs::read(&p).with_context(|| format!("read {}", p.display()))?;
        ensure!(raw.len() == mx.k as usize * mx.n as usize, "{}: {} bytes, expected n x k", p.display(), raw.len());
        let bt: Vec<i8> = raw.into_iter().map(|x| x as i8).collect();
        ensure!(bt.iter().all(|&x| (-64..=64).contains(&x)), "{}: values outside [-64, 64]", p.display());
        let key = standing::matrix_job_key(&id, mx.index, mx.k, mx.n, &base);
        mx.hash_b = hex::encode(standing::commit_bt(&key, &bt));
    }
    man.model_id = hex::encode(id);
    man.registrant = b58(registrant);
    man.files = standing::list_files(dir)?;
    let bytes = serde_json::to_vec_pretty(&man)?;
    // new inode (tmp + rename): a hardlinked copy of the model directory keeps its own manifest
    let tmp = dir.join("manifest.json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, dir.join("manifest.json"))?;
    Ok((man, bytes))
}

pub fn register_body(man: &Manifest, manifest_bytes: &[u8]) -> Result<RegisterModel> {
    let base = man.base_config()?;
    let matrices = man
        .matrices
        .iter()
        .map(|m| Ok(MatrixSpec { k: m.k, n: m.n, hash_b: hex::decode(&m.hash_b)?.try_into().map_err(|_| anyhow!("hash_b"))? }))
        .collect::<Result<Vec<_>>>()?;
    Ok(RegisterModel {
        name: man.name.as_bytes().to_vec(),
        manifest_uri: cadre_job::data::blob_uri(&cadre_job::data::blob_id(manifest_bytes)).into_bytes(),
        recipe: standing::RECIPE_W7A7_SQ_ASYM,
        rank: base.rank,
        rows_pattern: base.rows_pattern.to_bytes(),
        cols_pattern: base.cols_pattern.to_bytes(),
        matrices,
    })
}

fn put(node: &cadre_node::HttpNode, data: &[u8]) -> Result<bool> {
    let id = hex::encode(cadre_job::data::blob_id(data));
    if node.call("hasBlob", json!({"blob_id": id}))?["present"] == json!(true) {
        return Ok(false);
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    for attempt in 0..120 {
        match node.call_raw("putBlob", json!({"data": b64, "push": false}))? {
            Ok(_) => return Ok(true),
            Err(e) if e.contains("rate limited") => std::thread::sleep(Duration::from_millis(250 * (attempt + 1).min(8))),
            Err(e) => bail!("putBlob: {e}"),
        }
    }
    bail!("putBlob still rate limited after 120 tries")
}

/// Every chunk of every manifest file, `parallel` uploads at a time, each thread reading its
/// chunk's byte range (a 1 GB embedding is never held whole). Returns the blobs newly stored.
fn upload_chunks(rpc: &str, dir: &Path, man: &Manifest, parallel: usize) -> Result<u64> {
    let mut work: Vec<(String, usize)> = Vec::new();
    for (fname, f) in &man.files {
        for i in 0..f.blobs.len() {
            work.push((fname.clone(), i));
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let new = std::sync::atomic::AtomicU64::new(0);
    std::thread::scope(|sc| -> Result<()> {
        let hs: Vec<_> = (0..parallel.clamp(1, 32))
            .map(|_| {
                sc.spawn(|| -> Result<()> {
                    use std::io::{Read, Seek, SeekFrom};
                    let node = cadre_node::HttpNode::new(rpc);
                    loop {
                        let j = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((fname, i)) = work.get(j) else { return Ok(()) };
                        let f = &man.files[fname];
                        let off = (*i * standing::MODEL_CHUNK_BYTES) as u64;
                        let len = (f.size - off).min(standing::MODEL_CHUNK_BYTES as u64) as usize;
                        let mut buf = vec![0u8; len];
                        let mut fh = std::fs::File::open(dir.join(fname)).with_context(|| format!("open {fname}"))?;
                        fh.seek(SeekFrom::Start(off))?;
                        fh.read_exact(&mut buf)?;
                        let uri = cadre_job::data::blob_uri(&cadre_job::data::blob_id(&buf));
                        ensure!(uri == f.blobs[*i], "{fname} chunk {i} changed since the manifest was written");
                        if put(&node, &buf)? {
                            new.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().map_err(|_| anyhow!("upload thread panicked"))??;
        }
        Ok(())
    })?;
    Ok(new.into_inner())
}

fn register(ctx: &Ctx, dir: &Path, name: Option<&str>, dry_run: bool, store: Option<&Path>, parallel: usize) -> Result<Out> {
    let key = ctx.key()?;
    let me = key.verifying_key().to_bytes();
    let name = match name {
        Some(n) => n.to_string(),
        None => Manifest::load(dir)?.name,
    };
    ensure!(!name.is_empty() && name.len() <= 64, "model name must be 1..=64 bytes");
    let st = ctx.ui.step("Hashing matrices and chunking files");
    let t0 = std::time::Instant::now();
    let (man, bytes) = prepare(dir, &me, &name)?;
    let body = register_body(&man, &bytes)?;
    let chunks: u64 = man.files.values().map(|f| f.blobs.len() as u64).sum();
    let total: u64 = man.files.values().map(|f| f.size).sum();
    st.ok(format!("{} matrices hashed, {} files in {chunks} chunks ({:.1} s)", man.matrices.len(), man.files.len(), t0.elapsed().as_secs_f64()));
    let manifest_uri = String::from_utf8_lossy(&body.manifest_uri).to_string();
    let mut j = json!({"model_id": man.model_id, "name": name, "matrices": man.matrices.len(), "macs_per_row": man.macs_per_row().to_string(),
        "manifest_uri": manifest_uri, "files": man.files.len(), "chunks": chunks, "bytes": total});
    if !dry_run {
        // Upload order (SPEC 4.5, 2026-10-21): manifest, RegisterModel, then the chunks. A node
        // keeps an unreferenced blob only `grace_blocks` (100 by default), far shorter than an
        // 8B upload (7.5 GB, about 2 h at the default 1 MiB/s put rate), but it pins the
        // manifest and every chunk the manifest lists as soon as the model is registered. So
        // only the manifest (one blob) has to outlive the inclusion wait, and the chunks are
        // pinned whenever they arrive. A rerun skips the tx when the model exists and uploads
        // whatever the node lacks. (Before 2026-10-20 the tx went first with no manifest on the
        // node, and an inclusion timeout left devnet's first model registered with no blobs.)
        let existing = ctx.call("getModel", json!({"model_id": man.model_id}))?;
        if !existing.is_null() {
            ensure!(existing["manifest_uri"].as_str() == Some(manifest_uri.as_str()),
                "model {} is already registered with manifest {} (this directory hashes to {manifest_uri}); register under a new name", man.model_id, existing["manifest_uri"]);
        }
        let st = ctx.ui.step("Uploading the manifest");
        let r = put(&ctx.node, &bytes);
        st.wrap(r, |n| if *n { "manifest stored".to_string() } else { "manifest already on the node".to_string() })?;
        if existing.is_null() {
            let sent = crate::commands::send_body(ctx, "RegisterModel", TxBody::RegisterModel(body.clone()), None, None, true)
                .context("RegisterModel did not land in time: rerun `cadre model register` once it does (the manifest stays on the node for its grace period, and a rerun uploads only what is missing)")?;
            j["tx_hash"] = json!(sent.tx_hash);
            j["included_height"] = json!(sent.height);
        } else {
            j["registered"] = json!("already");
        }
        let st = ctx.ui.step(format!("Uploading {chunks} chunks ({:.0} MB, {} in parallel)", total as f64 / 1e6, parallel.clamp(1, 32)));
        let t1 = std::time::Instant::now();
        let r = (|| -> Result<u64> {
            let new = upload_chunks(&ctx.rpc, dir, &man, parallel)?;
            let held = ctx.call("hasBlob", json!({"blob_id": hex::encode(cadre_job::data::blob_id(&bytes))}))?;
            ensure!(held["present"] == json!(true), "the node does not hold the manifest after the upload");
            Ok(new)
        })();
        let new = st.wrap(r, |n| format!("{n} blobs stored ({:.1} s), pinned by the registered manifest", t1.elapsed().as_secs_f64()))?;
        let up = t1.elapsed().as_secs_f64();
        j["uploaded"] = json!(new);
        j["upload_s"] = json!(up);
        if up > 0.0 {
            j["upload_mb_per_s"] = json!(((total as f64 / 1e6) / up * 100.0).round() / 100.0);
        }
    }
    if let Some(store) = store {
        std::fs::create_dir_all(store)?;
        let link = store.join(&man.model_id);
        if !link.exists() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::canonicalize(dir)?, &link)?;
        }
        j["store"] = json!(link.display().to_string());
    }
    let p = Panel::new("RegisterModel")
        .kv("Model", s(&j, "model_id"))
        .kv("Name", name.as_str())
        .kv("Matrices", man.matrices.len().to_string())
        .kv("MACs per row", man.macs_per_row().to_string())
        .kv("Manifest", s(&j, "manifest_uri"))
        .kv("Blobs", format!("{chunks} chunks, {:.0} MB", total as f64 / 1e6))
        .kv("Block", j["included_height"].as_u64().map(|h| h.to_string()).unwrap_or(if dry_run { "dry run".into() } else { "-".into() }));
    let text = format!("model {} registered: {} matrices, {chunks} blobs", short(s(&j, "model_id")), man.matrices.len());
    Ok(Out { json: j, text, view: View::panel(p) })
}

fn fetch(ctx: &Ctx, id: &str, dir: &Path) -> Result<Out> {
    let m = ctx.call("getModel", json!({"model_id": id}))?;
    ensure!(!m.is_null(), "model {id} is not registered");
    let node = cadre_node::HttpNode::new(&ctx.rpc);
    let manifest_bytes = cadre_node::fetch_blob(&node, s(&m, "manifest_uri"))?;
    let man: Manifest = serde_json::from_slice(&manifest_bytes)?;
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("manifest.json"), &manifest_bytes)?;
    let st = ctx.ui.step(format!("Fetching {} files", man.files.len()));
    for name in man.files.keys() {
        cadre_node::fetch_model_file(&node, &man, name, dir)?;
    }
    st.ok("every file matches its blake3");
    let base = man.base_config()?;
    let mid: [u8; 32] = hex::decode(id)?.try_into().map_err(|_| anyhow!("model id"))?;
    for (i, x) in m["matrices"].as_array().into_iter().flatten().enumerate() {
        let mx = &man.matrices[i];
        let bt: Vec<i8> = std::fs::read(dir.join(&mx.weights))?.into_iter().map(|b| b as i8).collect();
        let key = standing::matrix_job_key(&mid, i as u16, mx.k, mx.n, &base);
        ensure!(hex::encode(standing::commit_bt(&key, &bt)) == s(x, "hash_b"), "matrix {i}: weights do not match the registered hash_b");
    }
    let j = json!({"model_id": id, "dir": dir.display().to_string(), "files": man.files.len(), "matrices": man.matrices.len()});
    Ok(Out { text: format!("model {} fetched into {}", short(id), dir.display()), view: View::panel(Panel::new("Model fetch").kv("Model", id).kv("Dir", dir.display().to_string()).kv("Files", man.files.len().to_string()).kv("hash_b", "all match")), json: j })
}

/// `cadre ticket <id>`: an inference ticket with its segments (getTicket).
pub fn ticket(ctx: &Ctx, id: &str) -> Result<Out> {
    let t = ctx.call("getTicket", json!({"ticket_id": id}))?;
    ensure!(!t.is_null(), "no ticket {id}");
    let st = &t["standing"];
    let segs = t["segment_list"].as_array().cloned().unwrap_or_default();
    let mined: u64 = segs.iter().map(|g| u(g, "blocks_mined")).sum();
    let p = Panel::new("Ticket")
        .kv("Ticket", s(&t, "job_id"))
        .kv("Status", s(&t, "status"))
        .kv("Model", s(st, "model_id"))
        .kv("Server", s(st, "server"))
        .kv("Buyer", s(&t, "poster"))
        .kv("Prompt / max new", format!("{} / {}", u(st, "prompt_tokens"), u(st, "max_new_tokens")))
        .kv("W", tk(num(&t, "worker_pay")))
        .kv("P", tk(num(&t, "protocol_fee")))
        .kv("Subsidy cap (alpha x P)", tk(num(st, "cap_total")))
        .kv("Subsidy paid", tk(num(st, "subsidy_paid")))
        .kv("Segments", format!("{} ({} blocks mined)", segs.len(), mined))
        .kv("Response", s(&t, "result_uri"));
    let rows = segs
        .iter()
        .map(|g| vec![u(g, "matrix").to_string(), format!("{}x{}x{}", u(g, "m"), u(g, "k"), u(g, "n")), format!("{}/{}", u(g, "blocks_mined"), u(g, "max_blocks")),
            tk(num(g, "subsidy_paid")), s(g, "status").to_string()])
        .collect();
    let tb = table("Segments", &["#", "m x k x n", "Blocks", "Subsidy", "Status"], rows, "no segments posted yet");
    let text = format!("ticket {} {}: {} segments, {mined} blocks, subsidy {}", short(id), s(&t, "status"), segs.len(), s(st, "subsidy_paid"));
    Ok(Out { json: t, text, view: View::panel(p).with(Block::Table(tb)) })
}
