//! Qwen3's tokenizer: byte-level BPE, read from the checkpoint's own `tokenizer.json`.

use std::collections::HashMap;

use unicode_normalization::UnicodeNormalization;

/// What a tokenizer refusal means: a file it cannot read, or text it cannot split.
#[derive(Debug)]
pub enum TokenizerRefusal {
    /// The checkpoint's `tokenizer.json` is absent or not the shape this reads.
    Weights(String),
}

impl std::fmt::Display for TokenizerRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenizerRefusal::Weights(reason) => {
                write!(formatter, "the tokenizer is not readable: {reason}")
            }
        }
    }
}

/// The bytes-to-symbol table byte-level BPE uses, and its inverse.
///
/// # Invariant
/// Every one of the 256 bytes has a symbol and every symbol a byte, because a byte the table dropped would
/// make a round trip lose a character rather than fail loudly.
fn byte_symbols() -> ([char; 256], HashMap<char, u8>) {
    let mut forward = ['\0'; 256];
    let mut inverse = HashMap::new();
    // A printable byte is its own character; the rest are numbered in order from 256, which is all
    // this vocabulary holds.
    let mut next = 256;
    for byte in 0u16..256 {
        let symbol = match byte {
            33..=126 | 161..=172 | 174..=255 => {
                char::from_u32(u32::from(byte)).unwrap_or('\u{fffd}')
            }
            _ => {
                let symbol = char::from_u32(next).unwrap_or('\u{fffd}');
                next += 1;
                symbol
            }
        };
        forward[usize::from(byte)] = symbol;
        inverse.insert(symbol, byte as u8);
    }
    (forward, inverse)
}

/// The split the checkpoint's pre-tokenizer states, which is the one its merges were learned against.
const SPLIT: &str = r"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+";

/// A tokenizer read from a checkpoint's `tokenizer.json`.
pub struct Tokenizer {
    vocabulary: HashMap<String, usize>,
    ranks: HashMap<(String, String), usize>,
    added: Vec<(String, usize)>,
    split: fancy_regex::Regex,
    forward: [char; 256],
    inverse: HashMap<char, u8>,
}

impl Tokenizer {
    /// Read a checkpoint's `tokenizer.json`.
    ///
    /// # Invariant
    /// The added tokens are sorted longest first, so a scan that takes the first match takes the longest
    /// one: `<|im_start|>` and `<|im_end|>` share a prefix, and the shorter one would win on the wrong order.
    pub fn open(path: &std::path::Path) -> Result<Self, TokenizerRefusal> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| TokenizerRefusal::Weights(format!("{}: {error}", path.display())))?;
        let document: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| TokenizerRefusal::Weights(format!("{}: {error}", path.display())))?;
        let model = document
            .get("model")
            .ok_or_else(|| TokenizerRefusal::Weights("no `model`".to_string()))?;
        let mut vocabulary = HashMap::new();
        for (token, id) in model
            .get("vocab")
            .and_then(|vocab| vocab.as_object())
            .ok_or_else(|| TokenizerRefusal::Weights("no `model.vocab`".to_string()))?
        {
            let id = id
                .as_u64()
                .ok_or_else(|| TokenizerRefusal::Weights(format!("token `{token}` has no id")))?;
            vocabulary.insert(token.clone(), id as usize);
        }
        let mut ranks = HashMap::new();
        for (rank, merge) in model
            .get("merges")
            .and_then(|merges| merges.as_array())
            .ok_or_else(|| TokenizerRefusal::Weights("no `model.merges`".to_string()))?
            .iter()
            .enumerate()
        {
            let pair = merge
                .as_array()
                .filter(|pair| pair.len() == 2)
                .ok_or_else(|| TokenizerRefusal::Weights("a merge is not a pair".to_string()))?;
            let left = pair[0].as_str().unwrap_or_default().to_string();
            let right = pair[1].as_str().unwrap_or_default().to_string();
            ranks.insert((left, right), rank);
        }
        let mut added: Vec<(String, usize)> = document
            .get("added_tokens")
            .and_then(|added| added.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        Some((
                            entry.get("content")?.as_str()?.to_string(),
                            entry.get("id")?.as_u64()? as usize,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        added.sort_by(|left, right| right.0.len().cmp(&left.0.len()));
        // A backtracking budget: a run of letters makes the last alternatives try every split.
        let split = fancy_regex::RegexBuilder::new(SPLIT)
            .backtrack_limit(8 * 1024 * 1024)
            .build()
            .map_err(|error| TokenizerRefusal::Weights(format!("the split: {error}")))?;
        let (forward, inverse) = byte_symbols();
        Ok(Self {
            vocabulary,
            ranks,
            added,
            split,
            forward,
            inverse,
        })
    }

    /// The ids `text` tokenizes to, special tokens included.
    ///
    /// # Invariant
    /// The text is normalised and then split exactly as the checkpoint's pre-tokenizer states, because the
    /// merges were learned against those pieces: a split that disagreed would merge across a boundary the
    /// vocabulary never saw.
    pub fn encode(&self, text: &str) -> Result<Vec<usize>, TokenizerRefusal> {
        let normalised: String = text.nfc().collect();
        let mut ids = Vec::new();
        let mut ordinary = String::new();
        let mut rest = normalised.as_str();
        // A marker is matched wherever it starts, not only at the text's beginning.
        while !rest.is_empty() {
            match self
                .added
                .iter()
                .find(|(content, _)| rest.starts_with(content.as_str()))
            {
                Some((content, id)) => {
                    if !ordinary.is_empty() {
                        self.encode_ordinary(&ordinary, &mut ids)?;
                        ordinary.clear();
                    }
                    ids.push(*id);
                    rest = &rest[content.len()..];
                }
                None => {
                    let first = rest.chars().next().expect("a character");
                    ordinary.push(first);
                    rest = &rest[first.len_utf8()..];
                }
            }
        }
        if !ordinary.is_empty() {
            self.encode_ordinary(&ordinary, &mut ids)?;
        }
        Ok(ids)
    }

    fn encode_ordinary(
        &self,
        ordinary: &str,
        ids: &mut Vec<usize>,
    ) -> Result<(), TokenizerRefusal> {
        // A piece the split refuses is refused here; dropping it is how a script once vanished silently.
        let pieces = self.split.find_iter(ordinary);
        let mut spans = Vec::new();
        for found in pieces {
            let piece = found.map_err(|error| {
                TokenizerRefusal::Weights(format!("the split refused a piece: {error}"))
            })?;
            spans.push(piece.as_str().to_string());
        }
        for piece in spans.iter().map(String::as_str) {
            let bytes = piece.as_bytes();
            let mut symbols: Vec<String> = bytes
                .iter()
                .map(|byte| self.forward[usize::from(*byte)].to_string())
                .collect();
            while symbols.len() > 1 {
                let mut best: Option<(usize, usize)> = None;
                for index in 0..symbols.len() - 1 {
                    let pair = (symbols[index].clone(), symbols[index + 1].clone());
                    if let Some(rank) = self.ranks.get(&pair)
                        && best.is_none_or(|(held, _)| *rank < held)
                    {
                        best = Some((*rank, index));
                    }
                }
                let Some((_, at)) = best else {
                    break;
                };
                let merged = format!("{}{}", symbols[at], symbols[at + 1]);
                symbols.splice(at..at + 2, [merged]);
            }
            for symbol in symbols {
                let Some(id) = self.vocabulary.get(&symbol) else {
                    return Err(TokenizerRefusal::Weights(format!(
                        "the vocabulary holds no token for {symbol:?}, which the merges produced"
                    )));
                };
                ids.push(*id);
            }
        }
        Ok(())
    }

    /// The text `ids` spells, with an id the vocabulary does not hold left out.
    ///
    /// # Invariant
    /// A left-out id is left out rather than written as a placeholder, because a placeholder would be text
    /// the model never produced.
    pub fn decode(&self, ids: &[usize]) -> String {
        let mut bytes = Vec::new();
        for id in ids {
            let Some(token) = self
                .vocabulary
                .iter()
                .find(|(_, held)| **held == *id)
                .map(|(token, _)| token)
            else {
                continue;
            };
            for symbol in token.chars() {
                if let Some(byte) = self.inverse.get(&symbol) {
                    bytes.push(*byte);
                }
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}
