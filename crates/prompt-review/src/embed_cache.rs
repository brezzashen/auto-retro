//! On-disk vector cache keyed by the SHA-256 of the text, so the weekly run (two captures sharing most
//! text) and a rerun embed only new text. Records: 32-byte key, u32 dimension, f32 values (LE).
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

pub type Key = [u8; 32];

pub fn key(text: &str) -> Key {
    Sha256::digest(text.as_bytes()).into()
}

pub fn load(path: &Path) -> HashMap<Key, Vec<f32>> {
    let mut out = HashMap::new();
    let Ok(bytes) = std::fs::read(path) else { return out };
    let mut at = 0;
    while at + 36 <= bytes.len() {
        let k: Key = bytes[at..at + 32].try_into().expect("32 bytes");
        let dim = u32::from_le_bytes(bytes[at + 32..at + 36].try_into().expect("4 bytes")) as usize;
        let end = at + 36 + dim * 4;
        if end > bytes.len() { break; }
        out.insert(k, bytes[at + 36..end].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().expect("4 bytes"))).collect());
        at = end;
    }
    out
}

pub fn append(path: &Path, rows: &[(Key, Vec<f32>)]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let mut buf = Vec::new();
    for (k, v) in rows {
        buf.extend_from_slice(k);
        buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
        v.iter().for_each(|x| buf.extend_from_slice(&x.to_le_bytes()));
    }
    file.write_all(&buf)
}
