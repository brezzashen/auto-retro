//! A returned source file used through its module path rather than its file name. Search-retro
//! 8486853407474113856 (35e991ab…jsonl:484): `search "fn ms("` returned
//! `dev/deliver_wait_facts.rs:44`, and the agent's next edit wrote
//! `use crate::dev::deliver_wait_facts::{load_board, ms};`. The hit was used, but no later call named
//! the file, so it counted as unadopted.
//!
//! A Rust file is named as a module by `<stem>::` (or `<dir>::<stem>` closing a `use` path); a
//! TypeScript or JavaScript import names it as `/<stem>'`, `/<stem>"` or `/<stem>.js`.
//! Generic stems (`mod`, `lib`, `main`, `index`, `types`, `utils`) and stems shorter than
//! MIN_STEM say too little about which file was used and never count.

/// Shortest file stem that identifies a module on its own.
const MIN_STEM: usize = 5;
/// Stems shared by many directories; a module path through them names no particular file.
const GENERIC: &[&str] = &["mod", "lib", "main", "index", "types", "utils", "tests", "error", "config"];

/// True when `text` names the module of `file` (a returned path tail such as `dev/deliver_wait_facts.rs`).
pub fn names_module(text: &str, file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    if stem.len() < MIN_STEM || GENERIC.contains(&stem) {
        return false;
    }
    match ext {
        "rs" => occurs(text, stem, |after| after.starts_with("::") || (after.starts_with(';') || after.starts_with('}') || after.starts_with(',')), true),
        "ts" | "tsx" | "js" | "jsx" | "mjs" => occurs(text, stem, |after| after.starts_with('\'') || after.starts_with('"') || after.starts_with(".js'") || after.starts_with(".js\""), false),
        _ => false,
    }
}

/// An occurrence of `stem` as a whole identifier whose following text satisfies `tail`. For Rust,
/// a stem followed by `;`, `}` or `,` counts only after `::` (closing a `use` path), never as a bare word.
fn occurs(text: &str, stem: &str, tail: impl Fn(&str) -> bool, rust: bool) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(stem).any(|(i, _)| {
        let before = &text[..i];
        let after = &text[i + stem.len()..];
        if before.chars().next_back().is_some_and(ident) || after.chars().next().is_some_and(ident) {
            return false;
        }
        if !tail(after) {
            return false;
        }
        if rust {
            after.starts_with("::") || before.ends_with("::")
        } else {
            before.ends_with('/')
        }
    })
}

#[cfg(test)]
mod tests {
    use super::names_module;

    #[test]
    fn a_rust_use_path_names_the_returned_file() {
        // Search-retro 8486853407474113856, 35e991ab…jsonl:484 → the heredoc edit that followed.
        assert!(names_module("s{use crate::dev::deliver_wait_facts::load_board;}{use crate::dev::deliver_wait_facts::{load_board, ms};};", "dev/deliver_wait_facts.rs"));
        assert!(names_module("use crate::dev::deliver_wait_facts;", "dev/deliver_wait_facts.rs"));
        assert!(names_module("deliver_wait_facts::ms(&iso)", "dev/deliver_wait_facts.rs"));
    }

    #[test]
    fn a_ts_import_names_the_returned_file() {
        assert!(names_module("import { fetchPeople } from '../lib/api/people'", "api/people.ts"));
        assert!(names_module("import { x } from \"./workspaceSearch.js\"", "components/workspaceSearch.tsx"));
    }

    #[test]
    fn bare_words_generic_stems_and_longer_names_do_not() {
        assert!(!names_module("the deliver_wait_facts helper", "dev/deliver_wait_facts.rs"));
        assert!(!names_module("use crate::dev::deliver_wait_facts_v2::ms;", "dev/deliver_wait_facts.rs"));
        assert!(!names_module("use crate::release::types::Plan;", "release/types.rs"));
        assert!(!names_module("use super::mod::x;", "dev/mod.rs"));
        assert!(!names_module("people is a word", "api/people.ts"));
        assert!(!names_module("use crate::dev::deliver_wait_facts::ms;", "dev/deliver_wait_facts.swift"));
    }
}
