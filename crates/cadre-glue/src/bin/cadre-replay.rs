//! cadre-replay: the cadre-glue-v1 replay on local files (SPEC 4.7).
//!
//!   cadre-replay tables --model DIR [--positions N]      digest of the integer tables
//!   cadre-replay decode --model DIR --ids '[...]'         the evaluator's decode of token ids
//!   cadre-replay local  --model DIR --run DIR [--proof-out F]
//!       DIR/request.json (RequestClaim) + DIR/a/<i>.i8 (the served A, rows x k, maybe padded)
//!       [+ DIR/c/<i>.i32]: rebuild every A, compare, check C, tokens, stop rule and text
//!   cadre-replay verify --model DIR --proof F --a-file F --m M
//!       check a cadre-fraud-v1 activation proof against a segment's posted A
//! Prints one JSON object; exit 0 = honest / proof valid, 1 = fraud found / proof invalid.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use cadre_glue::fraud::{self, FraudProof};
use cadre_glue::replay::{first_diff, Mismatch, RequestClaim};
use cadre_glue::tokenizer::Tokenizer;
use cadre_glue::{forward_round, gemm_bt, GemmSink, Model};
use serde_json::json;

fn arg(args: &[String], k: &str) -> Option<String> {
    args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned()
}

struct FileSink<'a> {
    model: &'a Model,
    run: PathBuf,
    rows: usize,
    mismatch: Option<Mismatch>,
    c_bad: Option<usize>,
    gemm_s: f64,
}

impl GemmSink for FileSink<'_> {
    fn gemm(&mut self, index: usize, a: &[i8], m: usize, k: usize, n: usize) -> Result<Vec<i32>> {
        let posted: Vec<i8> = std::fs::read(self.run.join(format!("a/{index}.i8")))?.into_iter().map(|x| x as i8).collect();
        anyhow::ensure!(posted.len() >= self.rows * k, "a/{index}.i8 is shorter than {} rows", self.rows);
        if self.mismatch.is_none() {
            if let Some(d) = first_diff(index, &posted[..self.rows * k], &a[..self.rows * k], k) {
                self.mismatch = Some(d);
                return Err(anyhow!("mismatch"));
            }
        }
        let t = Instant::now();
        let c = gemm_bt(a, m, k, self.model.bt(index).unwrap(), n);
        self.gemm_s += t.elapsed().as_secs_f64();
        if let Ok(cb) = std::fs::read(self.run.join(format!("c/{index}.i32"))) {
            let cp: Vec<i32> = cb.chunks_exact(4).take(self.rows * n).map(|x| i32::from_le_bytes(x.try_into().unwrap())).collect();
            if self.c_bad.is_none() && cp != c[..self.rows * n] {
                self.c_bad = Some(index);
            }
        }
        Ok(c)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok((ok, v)) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
            std::process::exit(if ok { 0 } else { 1 });
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    }
}

fn run(args: &[String]) -> Result<(bool, serde_json::Value)> {
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let model_dir = PathBuf::from(arg(args, "--model").ok_or_else(|| anyhow!("--model DIR"))?);
    match cmd {
        "tables" => {
            let m = Model::load(&model_dir)?;
            let p: usize = arg(args, "--positions").map(|x| x.parse()).transpose()?.unwrap_or(4096);
            Ok((true, json!({"glue": cadre_glue::GLUE_ID, "positions": p, "digest": m.table_digest(p)})))
        }
        "decode" => {
            let tok = Tokenizer::load(&model_dir)?;
            let ids: Vec<u32> = serde_json::from_str(&arg(args, "--ids").ok_or_else(|| anyhow!("--ids"))?)?;
            Ok((true, json!({"text": tok.decode(&ids)})))
        }
        "local" => {
            let t0 = Instant::now();
            let run = PathBuf::from(arg(args, "--run").ok_or_else(|| anyhow!("--run DIR"))?);
            let req: RequestClaim = serde_json::from_slice(&std::fs::read(run.join("request.json")).context("request.json")?)?;
            let model = Model::load(&model_dir)?;
            let tok = Tokenizer::load(&model_dir)?;
            let load_s = t0.elapsed().as_secs_f64();
            let rows = req.rows();
            let t1 = Instant::now();
            let mut sink = FileSink { model: &model, run: run.clone(), rows: rows.len(), mismatch: None, c_bad: None, gemm_s: 0.0 };
            let res = forward_round(&model, &[rows.clone()], rows.len(), &mut sink);
            let replay_s = t1.elapsed().as_secs_f64();
            let timing = json!({"load_s": load_s, "replay_s": replay_s, "gemm_s": sink.gemm_s, "glue_s": replay_s - sink.gemm_s, "rows": rows.len()});
            if let Some(mx) = sink.mismatch.clone() {
                let man: serde_json::Value = serde_json::from_slice(&std::fs::read(model_dir.join("manifest.json"))?)?;
                let name = man["matrices"][mx.matrix]["name"].as_str().unwrap_or("").to_string();
                let proof = FraudProof {
                    format: fraud::FORMAT.into(),
                    glue: cadre_glue::GLUE_ID.into(),
                    kind: "activation".into(),
                    model_id: model.model_id.clone(),
                    ticket_id: String::new(),
                    round: 0,
                    matrix: Some(mx.matrix as u16),
                    segment_id: None,
                    response_uri: String::new(),
                    a_uri: None,
                    request: req.clone(),
                    mismatch: Some(mx.clone()),
                    reason: format!("posted A of matrix {} ({name}) differs from the glue's A at row {}, col {}: {} vs {} ({} elements differ)", mx.matrix, mx.row, mx.col, mx.posted, mx.expected, mx.differing),
                };
                if let Some(p) = arg(args, "--proof-out") {
                    std::fs::write(&p, serde_json::to_vec_pretty(&proof)?)?;
                }
                return Ok((false, json!({"verdict": "fraud", "kind": "activation", "matrix": mx.matrix, "matrix_name": name, "mismatch": mx, "reason": proof.reason, "timing": timing})));
            }
            let predicted = res?.pop().unwrap();
            if let Some(i) = sink.c_bad {
                return Ok((false, json!({"verdict": "fraud", "kind": "product", "matrix": i, "reason": format!("served C of matrix {i} is not A B"), "timing": timing})));
            }
            match req.check(&predicted, &tok, &model.eos)? {
                Ok(text) => Ok((true, json!({"verdict": "honest", "matrices": model.matrices, "text": text, "timing": timing}))),
                Err(r) => Ok((false, json!({"verdict": "fraud", "kind": "tokens", "reason": r, "timing": timing}))),
            }
        }
        "verify" => {
            let model = Model::load(&model_dir)?;
            let p: FraudProof = serde_json::from_slice(&std::fs::read(arg(args, "--proof").ok_or_else(|| anyhow!("--proof"))?)?)?;
            let a: Vec<i8> = std::fs::read(Path::new(&arg(args, "--a-file").ok_or_else(|| anyhow!("--a-file"))?))?.into_iter().map(|x| x as i8).collect();
            let m: usize = arg(args, "--m").ok_or_else(|| anyhow!("--m"))?.parse()?;
            let t = Instant::now();
            let r = fraud::verify_activation(&model, &p, &a, m)?;
            let s = t.elapsed().as_secs_f64();
            Ok(match r {
                Ok(()) => (true, json!({"proof": "valid", "matrix": p.matrix, "verify_s": s})),
                Err(e) => (false, json!({"proof": "invalid", "reason": e, "verify_s": s})),
            })
        }
        _ => Err(anyhow!("usage: cadre-replay tables|decode|local|verify --model DIR ...")),
    }
}
