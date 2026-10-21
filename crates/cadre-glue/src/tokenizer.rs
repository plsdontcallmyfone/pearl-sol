//! Byte-level BPE decoding from a Hugging Face `tokenizer.json` (Llama 3), exactly as the
//! worker's `tok.decode(ids, skip_special_tokens=True)`: special added tokens are skipped, the
//! rest map back to bytes through GPT-2's byte-to-unicode table, the bytes are read as UTF-8
//! with U+FFFD for invalid sequences, then transformers' `clean_up_tokenization` runs when the
//! tokenizer config asks for it (Llama 3.2: `clean_up_tokenization_spaces: true`).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

pub struct Tokenizer {
    tokens: HashMap<u32, Vec<u8>>,
    special: std::collections::HashSet<u32>,
    clean_up: bool,
}

fn byte_decoder() -> HashMap<char, u8> {
    let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    bs.into_iter().zip(cs).map(|(b, c)| (char::from_u32(c).unwrap(), b as u8)).collect()
}

impl Tokenizer {
    pub fn load(dir: &Path) -> Result<Self> {
        let t: Value = serde_json::from_slice(&std::fs::read(dir.join("tokenizer.json")).context("read tokenizer.json")?)?;
        anyhow::ensure!(t["decoder"]["type"] == "ByteLevel", "only ByteLevel decoders are supported");
        let dec = byte_decoder();
        let mut tokens = HashMap::new();
        for (s, id) in t["model"]["vocab"].as_object().ok_or_else(|| anyhow!("tokenizer.json has no model.vocab"))? {
            let id = id.as_u64().ok_or_else(|| anyhow!("vocab id"))? as u32;
            let bytes: Vec<u8> = s.chars().map(|c| dec.get(&c).copied().ok_or_else(|| anyhow!("token {s:?}: char {c:?} not byte-level"))).collect::<Result<_>>()?;
            tokens.insert(id, bytes);
        }
        let mut special = std::collections::HashSet::new();
        for a in t["added_tokens"].as_array().into_iter().flatten() {
            let id = a["id"].as_u64().unwrap_or(0) as u32;
            if a["special"].as_bool().unwrap_or(false) {
                special.insert(id);
            } else {
                tokens.insert(id, a["content"].as_str().unwrap_or("").as_bytes().to_vec());
            }
        }
        let clean_up = std::fs::read(dir.join("tokenizer_config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|c| c["clean_up_tokenization_spaces"].as_bool())
            .unwrap_or(false);
        Ok(Self { tokens, special, clean_up })
    }

    pub fn decode(&self, ids: &[u32]) -> String {
        let mut b = Vec::new();
        for id in ids {
            if self.special.contains(id) {
                continue;
            }
            if let Some(t) = self.tokens.get(id) {
                b.extend_from_slice(t);
            }
        }
        let s = String::from_utf8_lossy(&b).into_owned();
        if !self.clean_up {
            return s;
        }
        s.replace(" .", ".")
            .replace(" ?", "?")
            .replace(" !", "!")
            .replace(" ,", ",")
            .replace(" ' ", "'")
            .replace(" n't", "n't")
            .replace(" 'm", "'m")
            .replace(" 's", "'s")
            .replace(" 've", "'ve")
            .replace(" 're", "'re")
    }
}
