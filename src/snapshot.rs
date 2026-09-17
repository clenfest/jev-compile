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

impl Snapshot {
    pub fn state(&self, targets: &BTreeSet<usize>) -> Value {
        let names: BTreeSet<_> = targets
            .iter()
            .flat_map(|index| self.files[*index].syntax.identifiers.iter())
            .collect();
        let mut declarations = Vec::new();
        let mut included = BTreeSet::new();
        for (index, file) in self.files.iter().enumerate() {
            if targets.contains(&index) {
                continue;
            }
            for (name, spans) in &file.syntax.declarations {
                if !names.contains(name) {
                    continue;
                }
                for span in spans {
                    if !included.insert((index, *span)) {
                        continue;
                    }
                    declarations.push(json!({
                        "file": file.path, "symbol": name, "revision": file.revision,
                        "start_line": span.start_line,
                        "source": file.source.lines().enumerate()
                            .skip(span.start_line - 1).take(span.width())
                            .map(|(line, text)| format!("{}: {text}\n", line + 1)).collect::<String>()
                    }));
                }
            }
        }
        json!({
            "base_commit": self.base_commit,
            "snapshot": self.fingerprint,
            "language": self.language,
            "changes": self.changes,
            "files": targets.iter().map(|index| {
                let file = &self.files[*index];
                json!({"path": file.path, "revision": file.revision, "source": syntax::numbered(&file.source)})
            }).collect::<Vec<_>>(),
            "context": self.context,
            "related_declarations": declarations,
            "context_strategy": "Lexical matching of local declarations and references, not compiler name resolution. External dependencies, aliases, macros, generated files, and configuration may require more context.",
            "warnings": self.warnings,
        })
    }
}

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
                "--unified=12",
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
            warnings.push(format!("Changed file is supplied as diff evidence but not localized for the selected language: {path}"));
            continue;
        }
        let previous = if baseline_paths.contains(path) {
            String::from_utf8(git(root, &["show", &format!("{commit}:{path}")])?)?
        } else {
            String::new()
        };
        let before = Syntax::parse(language, path, &previous)?;
        changed_names.extend(before.declarations.keys().cloned());
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
        changed_names.extend(syntax.declarations.keys().cloned());
        references.extend(syntax.identifiers.iter().cloned());
        files.push(SourceFile {
            path: path.clone(),
            source,
            revision,
            syntax,
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
        if syntax
            .identifiers
            .iter()
            .any(|name| changed_names.contains(name))
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
        "files": snapshot.files.iter().map(|file| (&file.path, &file.source, file.revision)).collect::<Vec<_>>(),
        "context": snapshot.context, "warnings": snapshot.warnings,
    });
    snapshot.fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&fingerprint)?));
    Ok(snapshot)
}

pub fn root_span(file: &SourceFile) -> Span {
    Span::whole(&file.source)
}
