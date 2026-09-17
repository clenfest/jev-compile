use crate::Result;
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Serialize)]
pub struct Snapshot {
    pub base_commit: String,
    pub files: Vec<ChangedFile>,
    pub context: Vec<Context>,
}

#[derive(Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub diff: String,
}

#[derive(Serialize)]
pub struct Context {
    pub path: String,
    pub content: String,
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

pub fn collect(repo: &Path, base: &str, context_paths: &[PathBuf]) -> Result<Snapshot> {
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
    )?)?;
    let commit = commit.trim().to_string();
    let names = git(
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
    )?;
    let mut files = Vec::new();
    for name in names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let path = std::str::from_utf8(name)?.to_string();
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
                &path,
            ],
        )?)?;
        files.push(ChangedFile { path, diff });
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
    Ok(Snapshot {
        base_commit: commit,
        files,
        context,
    })
}
