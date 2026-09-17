mod language;
mod prediction;
mod search;
mod snapshot;
mod syntax;

use clap::Parser;
use language::Language;
use serde_json::json;
use std::{error::Error, path::PathBuf, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Predict and localize compiler errors within a fixed Jev call budget.
/// All results are advisory; this does not compile or certify your code.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Repository to inspect (never modified)
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// Known-good commit; compared with the working tree, not verified by compiling
    #[arg(long, default_value = "HEAD")]
    base: String,
    /// Source language; inferred for a single-language diff
    #[arg(long, value_enum)]
    language: Option<Language>,
    /// Number of categories from the language's curated list (plus other/context checks)
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u16).range(1..=10))]
    top_errors: u16,
    /// Hard maximum API requests, including failed attempts; 0 performs no calls
    #[arg(long, default_value_t = 8)]
    max_calls: u16,
    /// Additional UTF-8 context file, relative to the repository root (repeatable)
    #[arg(long)]
    context: Vec<PathBuf>,
    /// Print the first planned API request without sending it; later requests are adaptive
    #[arg(long)]
    dry_run: bool,
    /// Jev model name; subsequent calls use the first returned model ID
    #[arg(long, default_value = "jev-latest")]
    model: String,
    /// Maximum serialized bytes per request; evidence is never silently truncated
    #[arg(long, default_value_t = 200_000)]
    max_bytes: usize,
}

fn run(args: Args) -> Result<bool> {
    let started = Instant::now();
    let snapshot = snapshot::collect(&args.repo, &args.base, &args.context, args.language)?;
    let collection_ms = started.elapsed().as_millis();
    let options = search::Options {
        model: args.model,
        top_errors: usize::from(args.top_errors),
        max_calls: usize::from(args.max_calls),
        max_bytes: args.max_bytes,
    };
    if snapshot.changes.is_empty() {
        println!(
            "{}",
            json!({"status": "no_tracked_changes", "calls_used": 0, "findings": []})
        );
        return Ok(true);
    }
    if args.dry_run {
        if snapshot.files.is_empty() {
            return Err(
                "no supported source files to screen; only Rust and TypeScript are supported"
                    .into(),
            );
        }
        let preview = search::preview(&snapshot, &options)?;
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(true);
    }
    let key = if options.max_calls > 0 && !snapshot.files.is_empty() {
        std::env::var("TYPESAFE_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .ok_or("set TYPESAFE_API_KEY, or use --dry-run to inspect the request locally")?
    } else {
        String::new()
    };
    let mut report = search::run(
        &snapshot,
        &options,
        |request| prediction::evaluate(request, &key),
        |finding| {
            eprintln!(
                "{}:{}: {} [model probability {:.2}; advisory]",
                finding.file, finding.region.start_line, finding.message, finding.probability
            );
        },
    );
    if snapshot.files.is_empty() {
        report.stop_reason = "no_supported_sources";
        report.search_complete = false;
    }
    let success = report.error.is_none();
    report.collection_ms = collection_ms;
    report.total_elapsed_ms = started.elapsed().as_millis();
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(success)
}

fn main() {
    match run(Args::parse()) {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("jev-compile: {error}");
            std::process::exit(1);
        }
    }
}
