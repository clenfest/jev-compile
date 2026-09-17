mod prediction;
mod snapshot;

use clap::Parser;
use serde_json::json;
use std::{error::Error, path::PathBuf, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Predict likely compiler errors in tracked Git changes using Jev.
/// Predictions are advisory; this does not compile or certify your code.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Repository to inspect (never modified)
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// Known-good commit to compare with the working tree; not verified by compiling
    #[arg(long, default_value = "HEAD")]
    base: String,
    /// Additional UTF-8 context file, relative to the repository root (repeatable)
    #[arg(long)]
    context: Vec<PathBuf>,
    /// Print the exact API request without sending it; no key required
    #[arg(long)]
    dry_run: bool,
    /// Jev model name
    #[arg(long, default_value = "jev-latest")]
    model: String,
    /// Maximum serialized request bytes; oversized requests fail without truncation
    #[arg(long, default_value_t = 200_000)]
    max_bytes: usize,
}

fn run(args: Args) -> Result<()> {
    let snapshot = snapshot::collect(&args.repo, &args.base, &args.context)?;
    if snapshot.files.is_empty() {
        println!(
            "{}",
            json!({"status": "no_tracked_changes", "predictions": []})
        );
        return Ok(());
    }
    let request = prediction::request(&snapshot, &args.model);
    let body = serde_json::to_vec(&request)?;
    if body.len() > args.max_bytes {
        return Err(format!(
            "request is {} bytes, exceeding --max-bytes {}; narrow the diff/context or raise the limit",
            body.len(), args.max_bytes
        ).into());
    }
    if args.dry_run {
        println!("{}", serde_json::to_string_pretty(&request)?);
        return Ok(());
    }
    let key = std::env::var("TYPESAFE_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .ok_or("set TYPESAFE_API_KEY, or use --dry-run to inspect the request locally")?;
    let started = Instant::now();
    let response = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .redirects(0)
        .build()
        .post("https://api.typesafe.ai/v1/systemone")
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .send_bytes(&body)?
        .into_json::<prediction::Response>()?;
    let predictions = prediction::validate(&snapshot, &response)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "advisory_predictions",
            "base_commit": snapshot.base_commit,
            "model": response.model,
            "usage": response.usage,
            "request_bytes": body.len(),
            "inference_ms": started.elapsed().as_millis(),
            "predictions": predictions
        }))?
    );
    Ok(())
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("jev-compile: {error}");
        std::process::exit(1);
    }
}
