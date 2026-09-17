use crate::{
    language::Language,
    syntax::{self, Span, Syntax},
    Result,
};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

const INDEX_BYTES: usize = 16 * 1024 * 1024;

pub struct Snapshot {
    pub base_commit: String,
    pub fingerprint: String,
    pub language: Language,
    pub files: Vec<SourceFile>,
    pub changes: Vec<Change>,
    pub context: Vec<Context>,
    pub warnings: Vec<String>,
}

pub struct SourceFile {
    pub path: String,
    pub source: String,
    pub revision: &'static str,
    pub syntax: Syntax,
    pub screen: bool,
}

#[derive(Serialize)]
pub struct Change {
    pub path: String,
    pub diff: String,
}

#[derive(Serialize)]
pub struct Context {
    pub path: String,
    pub content: String,
}

#[path = "evidence.rs"]
mod evidence;

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["--literal-pathspecs", "-c", "core.quotePath=false"])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(output.stdout)
}

fn paths(bytes: &[u8]) -> Result<Vec<String>> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| Ok(std::str::from_utf8(part)?.to_owned()))
        .collect()
}

fn read_source(root: &Path, path: &str) -> Result<Option<String>> {
    let full = root.join(path);
    if !full.exists() {
        return Ok(None);
    }
    // Never follow a tracked symlink into another file or outside the repository.
    if !std::fs::symlink_metadata(&full)?.file_type().is_file() {
        return Ok(None);
    }
    if !full.canonicalize()?.starts_with(root.canonicalize()?) {
        return Err(format!("source path resolves outside the repository: {path}").into());
    }
    if std::fs::metadata(&full)?.len() > INDEX_BYTES as u64 {
        return Err(format!("source file exceeds the 16 MiB local indexing limit: {path}").into());
    }
    Ok(Some(std::fs::read_to_string(full)?))
}

pub fn collect(
    repo: &Path,
    base: &str,
    context_paths: &[PathBuf],
    requested_language: Option<Language>,
) -> Result<Snapshot> {
    let root = String::from_utf8(git(repo, &["rev-parse", "--show-toplevel"])?)?;
    let root = Path::new(root.trim_end_matches('\n'));
    let commit = String::from_utf8(git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{base}^{{commit}}"),
        ],
    )?)?
    .trim()
    .to_owned();
    let changed_paths = paths(&git(
        root,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--ignore-submodules=none",
            &commit,
            "--",
        ],
    )?)?;
    let tracked = paths(&git(root, &["ls-files", "-z"])?)?;
    let mut languages: Vec<_> = changed_paths
        .iter()
        .filter_map(|path| Language::for_path(path))
        .collect();
    if languages.is_empty() {
        languages = tracked
            .iter()
            .filter_map(|path| Language::for_path(path))
            .collect();
    }
    let language = match requested_language {
        Some(language) => language,
        None if changed_paths.is_empty() => Language::Rust,
        None if languages.iter().all(|language| *language == Language::Rust) => Language::Rust,
        None if languages
            .iter()
            .all(|language| *language == Language::Typescript) =>
        {
            Language::Typescript
        }
        None => return Err(
            "multiple source languages detected; select --language rust or --language typescript"
                .into(),
        ),
    };
    let baseline_paths: BTreeSet<_> = paths(&git(
        root,
        &["ls-tree", "-r", "--name-only", "-z", &commit],
    )?)?
    .into_iter()
    .collect();
    let mut changes = Vec::new();
    let mut files = Vec::new();
    let mut changed_names = BTreeSet::new();
    let mut references = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut indexed_bytes = 0;
    for path in &changed_paths {
        let diff = String::from_utf8(git(
            root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--no-renames",
                "--ignore-submodules=none",
                "--unified=3",
                &commit,
                "--",
                path,
            ],
        )?)?;
        changes.push(Change {
            path: path.clone(),
            diff,
        });
        if Language::for_path(path) != Some(language) {
            warnings.push(format!("Changed file is recorded in the snapshot but not analyzed for the selected language: {path}; supply relevant configuration with --context"));
            continue;
        }
        let previous = if baseline_paths.contains(path) {
            String::from_utf8(git(root, &["show", &format!("{commit}:{path}")])?)?
        } else {
            String::new()
        };
        let before = Syntax::parse(language, path, &previous)?;
        if std::fs::symlink_metadata(root.join(path))
            .is_ok_and(|metadata| !metadata.file_type().is_file())
        {
            warnings.push(format!(
                "Changed non-regular source file was not localized: {path}"
            ));
            continue;
        }
        let (source, revision) = match read_source(root, path)? {
            Some(source) => (source, "working_tree"),
            None => (previous, "baseline"),
        };
        indexed_bytes += source.len();
        if indexed_bytes > INDEX_BYTES {
            return Err("changed source exceeds the 16 MiB local indexing limit".into());
        }
        let syntax = Syntax::parse(language, path, &source)?;
        for name in before.declarations.keys().chain(syntax.declarations.keys()) {
            let contracts = |syntax: &Syntax| {
                syntax.declarations.get(name).map(|entries| {
                    entries
                        .iter()
                        .map(|entry| entry.evidence.clone())
                        .collect::<Vec<_>>()
                })
            };
            if revision == "baseline" || contracts(&before) != contracts(&syntax) {
                changed_names.insert(name.clone());
            }
        }
        references.extend(syntax.identifier_spans.keys().cloned());
        files.push(SourceFile {
            path: path.clone(),
            source,
            revision,
            syntax,
            screen: true,
        });
    }
    // Inspect one lexical hop. Freeze source once; calls never reread the checkout.
    for path in tracked
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|_| !changed_names.is_empty() || !references.is_empty())
    {
        if changed_paths.contains(&path) || Language::for_path(&path) != Some(language) {
            continue;
        }
        let Some(source) = read_source(root, &path)? else {
            continue;
        };
        indexed_bytes += source.len();
        if indexed_bytes > INDEX_BYTES {
            warnings.push("Local symbol lookup stopped at 16 MiB; some declarations or callers were not inspected".into());
            break;
        }
        let syntax = Syntax::parse(language, &path, &source)?;
        let screen = syntax
            .identifier_spans
            .keys()
            .any(|name| changed_names.contains(name));
        if screen
            || syntax
                .declarations
                .keys()
                .any(|name| references.contains(name))
        {
            files.push(SourceFile {
                path,
                source,
                revision: "working_tree",
                syntax,
                screen,
            });
        }
    }
    let context = context_paths
        .iter()
        .map(|path| {
            Ok(Context {
                path: path.to_string_lossy().into_owned(),
                content: std::fs::read_to_string(root.join(path))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut snapshot = Snapshot {
        base_commit: commit,
        fingerprint: String::new(),
        language,
        files,
        changes,
        context,
        warnings,
    };
    let fingerprint = json!({
        "base": snapshot.base_commit, "language": snapshot.language, "changes": snapshot.changes,
        "files": snapshot.files.iter().map(|file| (&file.path, &file.source, file.revision, file.screen)).collect::<Vec<_>>(),
        "context": snapshot.context, "warnings": snapshot.warnings,
    });
    snapshot.fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&fingerprint)?));
    Ok(snapshot)
}

/// Stable, exhaustive evidence windows. Descendants keep their original window
/// while localizing, so narrowing a question does not discard its context.
pub fn screen_spans(file: &SourceFile) -> Vec<Span> {
    let mut pending = vec![Span::whole(&file.source)];
    let mut regions = Vec::new();
    while let Some(region) = pending.pop() {
        if region.width() > 1 && syntax::numbered_region(&file.source, region).len() > 12_000 {
            pending.extend(file.syntax.partition(region));
        } else {
            regions.push(region);
        }
    }
    regions.sort_unstable();
    regions
}
