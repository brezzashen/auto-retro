//! prompt-review: the weekly review of the Mars CRM main agent's prompt surfaces (system prompt, tool
//! descriptions and schemas, bundled skill prompts and their catalog). cpu-watchdog runs `run` every
//! Monday (route `prompt-review`); it prints one JSON receipt line and opens no session itself.
mod dup;
mod embed;
mod embed_cache;
mod literals;
mod locate;
mod metrics;
mod page;
mod publish;
mod rerank;
mod run;
mod semantic;
mod tree;
mod units;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Weekly review of the Mars CRM agent's prompt size and duplicates")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture origin/mastra, compare with the previous run, find semantic duplicates, publish the page.
    Run {
        /// Build the page but do not publish it.
        #[arg(long)]
        no_publish: bool,
    },
    /// Semantic duplicate pass over one captured body, sources read from `--tree`.
    Semantic {
        body: PathBuf,
        #[arg(long)]
        tree: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Prints the stored run history (one JSON object per line).
    History,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Run { no_publish } => run::main(!no_publish),
        Command::Semantic { body, tree, out } => {
            let body = serde_json::from_slice(&std::fs::read(body)?)?;
            let pass = semantic::run(semantic::collect(&body, &tree)?, &tree, None)?;
            let json = pass.to_json();
            match out {
                Some(path) => std::fs::write(path, serde_json::to_vec_pretty(&json)?)?,
                None => println!("{}", serde_json::to_string_pretty(&json)?),
            }
            Ok(())
        }
        Command::History => {
            print!("{}", std::fs::read_to_string(tree::state_dir().join("history.ndjson")).unwrap_or_default());
            Ok(())
        }
    }
}
