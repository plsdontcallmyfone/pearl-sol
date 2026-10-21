//! `cadre-fraud-v1` (SPEC 4.7 "Fraud proofs"): what a challenger hands anyone (an evaluator, a
//! buyer, later the chain's adjudicators) to show a ticket's posted activations are not what the
//! registered model computes for the request. Verifying one costs one request's forward up to the
//! named segment, never the whole ticket, and needs no trust in the challenger: every input is
//! bound to the chain (the ticket's response blob id = its result_hash, the segment's A blob is
//! its data_uri and must match its hash_a commitment).

use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};

use crate::replay::{first_diff, Mismatch, RequestClaim};
use crate::{forward_round, gemm_bt, GemmSink, Model};

pub const FORMAT: &str = "cadre-fraud-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FraudProof {
    pub format: String,
    pub glue: String,
    /// "activation": the segment's posted A differs from the glue's A for this request's rows
    /// (`mismatch` names the element). "tokens": every A matches but the response's tokens, stop
    /// or text are not what the model produces (`reason`).
    pub kind: String,
    pub model_id: String,
    pub ticket_id: String,
    pub round: u16,
    pub matrix: Option<u16>,
    pub segment_id: Option<String>,
    /// blob: URI of the ticket's response (its id is the ticket's on-chain result_hash).
    pub response_uri: String,
    /// blob: URI of the segment's activation blob (the segment's data_uri).
    pub a_uri: Option<String>,
    pub request: RequestClaim,
    pub mismatch: Option<Mismatch>,
    pub reason: String,
}

#[derive(Debug)]
struct Stop;
impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "stop")
    }
}
impl std::error::Error for Stop {}

/// Rebuild one request's A for `matrix` (its rows only, m = rows).
pub fn request_a(model: &Model, req: &RequestClaim, matrix: usize) -> Result<Vec<i8>> {
    struct S<'a> {
        model: &'a Model,
        at: usize,
        got: Option<Vec<i8>>,
    }
    impl GemmSink for S<'_> {
        fn gemm(&mut self, index: usize, a: &[i8], m: usize, k: usize, n: usize) -> Result<Vec<i32>> {
            if index == self.at {
                self.got = Some(a.to_vec());
                return Err(Stop.into());
            }
            Ok(gemm_bt(a, m, k, self.model.bt(index).ok_or_else(|| anyhow!("no matrix {index}"))?, n))
        }
    }
    let rows = req.rows();
    let mut s = S { model, at: matrix, got: None };
    match forward_round(model, &[rows], req.rows().len(), &mut s) {
        Err(e) if e.downcast_ref::<Stop>().is_some() => {}
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    s.got.ok_or_else(|| anyhow!("matrix {matrix} is not in the forward"))
}

/// Verify an "activation" proof against the segment's posted A (`m` x k, already checked by the
/// caller to be the segment's blob). `Ok(Ok(()))`: the proof holds (the server is at fault).
pub fn verify_activation(model: &Model, p: &FraudProof, posted: &[i8], m: usize) -> Result<std::result::Result<(), String>> {
    ensure!(p.format == FORMAT && p.glue == crate::GLUE_ID, "not a cadre-fraud-v1 / cadre-glue-v1 proof");
    let mx = p.mismatch.as_ref().ok_or_else(|| anyhow!("activation proof without a mismatch"))?;
    let matrix = p.matrix.ok_or_else(|| anyhow!("activation proof without a matrix"))? as usize;
    let (k, _) = model.shape(matrix).ok_or_else(|| anyhow!("model has no matrix {matrix}"))?;
    ensure!(posted.len() == m * k, "posted A is not m x k");
    let rows = p.request.rows().len();
    let off = p.request.row_offset as usize;
    ensure!(off + rows <= m, "request rows {off}..{} outside the segment's {m} rows", off + rows);
    let own = request_a(model, &p.request, matrix)?;
    let seg = &posted[off * k..(off + rows) * k];
    let Some(d) = first_diff(matrix, seg, &own, k) else {
        return Ok(Err("the posted A equals the glue's A for this request: no fraud".into()));
    };
    if d.row + off != mx.row || d.col != mx.col || d.posted != mx.posted || d.expected != mx.expected {
        // a different first element is still a mismatch; the claim must name a real one
        let i = (mx.row - off) * k + mx.col;
        if mx.row < off || i >= own.len() || seg[i] != mx.posted || own[i] != mx.expected || seg[i] == own[i] {
            return Ok(Err(format!("the named element does not differ as claimed (first real difference: row {}, col {})", d.row + off, d.col)));
        }
    }
    Ok(Ok(()))
}
