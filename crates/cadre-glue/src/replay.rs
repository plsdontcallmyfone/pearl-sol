//! What an evaluator decides about one served request, given the glue's greedy tokens:
//! the tokens, the stopping rule, the text and the receipt hashes (SPEC 4.7 "Replay").

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::tokenizer::Tokenizer;

/// One request as the server claims it served it (from the response blob, SPEC 4.5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestClaim {
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub row_offset: u32,
    pub prompt_ids: Vec<u32>,
    pub output_ids: Vec<u32>,
    pub max_new_tokens: u32,
    #[serde(default)]
    pub stop: Vec<String>,
    #[serde(default)]
    pub output_sha256: String,
    #[serde(default)]
    pub text_sha256: String,
    #[serde(default)]
    pub prompt_sha256: String,
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

impl RequestClaim {
    /// The rows this request put through every matrix: the prompt, then every output token but
    /// the last (the last one is predicted, never fed back).
    pub fn rows(&self) -> Vec<u32> {
        let mut r = self.prompt_ids.clone();
        if let Some((_, init)) = self.output_ids.split_last() {
            r.extend_from_slice(init);
        }
        r
    }

    /// The text the worker returns: decode, then cut at the first stop string, in list order.
    pub fn text(&self, tok: &Tokenizer) -> String {
        let mut t = tok.decode(&self.output_ids);
        for s in self.stop.iter().filter(|s| !s.is_empty()) {
            if let Some(i) = t.find(s.as_str()) {
                t.truncate(i);
            }
        }
        t
    }

    /// Check the claim against the glue's greedy token after every row. `Ok(Err(reason))` is a
    /// provable fault of the server (the model does not produce this response for this request).
    pub fn check(&self, predicted: &[u32], tok: &Tokenizer, eos: &[u32]) -> Result<std::result::Result<String, String>> {
        let p = self.prompt_ids.len();
        if self.output_ids.is_empty() {
            return Ok(Err("response has no output tokens".into()));
        }
        if predicted.len() != self.rows().len() {
            return Ok(Err(format!("{} rows replayed, the request has {}", predicted.len(), self.rows().len())));
        }
        for (j, &t) in self.output_ids.iter().enumerate() {
            let want = predicted[p - 1 + j];
            if t != want {
                return Ok(Err(format!("output token {j} is {t}, the model's greedy token after row {} is {want}", p - 1 + j)));
            }
        }
        // stopping rule (tools/cadre/worker.py generate): stop string in the decoded prefix,
        // then an eos token, then max_new_tokens; the first that holds ends the generation
        let stops: Vec<&str> = self.stop.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
        let n = self.output_ids.len();
        for j in 0..n {
            let ended = (!stops.is_empty() && {
                let full = tok.decode(&self.output_ids[..=j]);
                stops.iter().any(|s| full.contains(s))
            }) || eos.contains(&self.output_ids[j])
                || (j + 1) as u32 >= self.max_new_tokens;
            if ended && j + 1 < n {
                return Ok(Err(format!("generation went on after output token {j}, where it had to stop")));
            }
            if !ended && j + 1 == n {
                return Ok(Err(format!("generation stopped after {n} tokens with no stop, eos or max_new_tokens")));
            }
        }
        let out_sha = sha256_hex(serde_json::to_string(&self.output_ids)?.as_bytes());
        if !self.output_sha256.is_empty() && self.output_sha256 != out_sha {
            return Ok(Err("output_sha256 is not the hash of output_ids".into()));
        }
        if !self.prompt_sha256.is_empty() && self.prompt_sha256 != sha256_hex(serde_json::to_string(&self.prompt_ids)?.as_bytes()) {
            return Ok(Err("prompt_sha256 is not the hash of prompt_ids".into()));
        }
        let text = self.text(tok);
        if !self.text_sha256.is_empty() && self.text_sha256 != sha256_hex(text.as_bytes()) {
            return Ok(Err("text_sha256 is not the hash of the decoded completion".into()));
        }
        Ok(Ok(text))
    }
}

/// A segment whose posted A is not what the glue rebuilds (the first one in matrix order).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mismatch {
    pub matrix: usize,
    /// Row in the segment (the round's stacked rows).
    pub row: usize,
    pub col: usize,
    pub posted: i8,
    pub expected: i8,
    pub differing: usize,
}

/// First differing element of two equal-shape A matrices.
pub fn first_diff(matrix: usize, posted: &[i8], expected: &[i8], k: usize) -> Option<Mismatch> {
    let differing = posted.iter().zip(expected).filter(|(a, b)| a != b).count();
    let i = posted.iter().zip(expected).position(|(a, b)| a != b)?;
    Some(Mismatch { matrix, row: i / k, col: i % k, posted: posted[i], expected: expected[i], differing })
}
