use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    fs::write(dir.path().join("types.rs"), "pub type Name = String;\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "baseline",
        ],
    );
    dir
}

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jev-compile"))
        .arg("--repo")
        .arg(root)
        .args(args)
        .env_remove("TYPESAFE_API_KEY")
        .output()
        .unwrap()
}

#[test]
fn collects_staged_and_unstaged_changes_with_explicit_context_without_mutation() {
    let repo = fixture();
    fs::write(repo.path().join("main.rs"), "fn main() { missing(); }\n").unwrap();
    fs::write(repo.path().join("new.rs"), "fn f() -> u32 { true }\n").unwrap();
    fs::write(repo.path().join("untracked.rs"), "not included\n").unwrap();
    git(repo.path(), &["add", "new.rs"]);
    let output = cli(
        repo.path(),
        &["--dry-run", "--context", "types.rs", "--top-errors", "2"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    let files = request["state"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["path"], "main.rs");
    assert!(request["state"]["changes"][0]["diff"]
        .as_str()
        .unwrap()
        .contains("+fn main() { missing(); }"));
    assert_eq!(files[1]["path"], "new.rs");
    assert_eq!(
        request["state"]["context"][0]["content"],
        "pub type Name = String;\n"
    );
    let categories: std::collections::BTreeSet<_> = request["questions"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|question| question["instructions"]["category"]["id"].as_str())
        .collect();
    assert_eq!(
        categories,
        std::collections::BTreeSet::from(["type_mismatch", "name_resolution", "other"])
    );
    let index = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["diff", "--cached", "--name-only"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(index.stdout).unwrap(), "new.rs\n");
    assert_eq!(
        fs::read_to_string(repo.path().join("main.rs")).unwrap(),
        "fn main() { missing(); }\n"
    );
}

#[test]
fn no_changes_skips_api_and_oversized_requests_and_invalid_bases_fail_locally() {
    let repo = fixture();
    let output = cli(repo.path(), &[]);
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["status"],
        "no_tracked_changes"
    );
    fs::write(repo.path().join("main.rs"), "broken\n").unwrap();
    let output = cli(repo.path(), &["--max-bytes", "1", "--dry-run"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeding --max-bytes"));
    let output = cli(repo.path(), &["--base=--help", "--dry-run"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("git failed"));
}

#[test]
fn deleted_files_and_git_pathspec_characters_are_handled_literally() {
    let repo = fixture();
    fs::remove_file(repo.path().join("types.rs")).unwrap();
    let literal = ":(glob)*.rs";
    fs::write(repo.path().join(literal), "fn test() {}\n").unwrap();
    git(repo.path(), &["--literal-pathspecs", "add", "--", literal]);
    let output = cli(repo.path(), &["--dry-run"]);
    assert!(output.status.success());
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    let files = request["state"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    let weird = files.iter().find(|file| file["path"] == literal).unwrap();
    assert!(weird["source"]
        .as_str()
        .unwrap()
        .contains("1: fn test() {}"));
    assert!(!weird["source"].as_str().unwrap().contains("pub type Name"));
    let deleted = files
        .iter()
        .find(|file| file["path"] == "types.rs")
        .unwrap();
    assert_eq!(deleted["revision"], "baseline");
    assert!(deleted["source"]
        .as_str()
        .unwrap()
        .contains("1: pub type Name = String;"));
}

#[test]
fn signature_changes_include_unchanged_callers_and_local_declarations() {
    let repo = fixture();
    fs::write(
        repo.path().join("types.rs"),
        "pub type Name = String;\npub fn greet(n: Name) {}\n",
    )
    .unwrap();
    fs::write(
        repo.path().join("main.rs"),
        "fn main() { greet(String::new()); }\n",
    )
    .unwrap();
    git(repo.path(), &["add", "."]);
    git(
        repo.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "signature baseline",
        ],
    );
    fs::write(
        repo.path().join("types.rs"),
        "pub type Name = String;\npub fn greet(n: u32) {}\n",
    )
    .unwrap();
    let output = cli(repo.path(), &["--dry-run"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    let files = request["state"]["files"].as_array().unwrap();
    assert!(files.iter().any(|file| file["path"] == "main.rs"));
    assert!(files.iter().any(|file| file["path"] == "types.rs"));
    assert_eq!(request["state"]["changes"].as_array().unwrap().len(), 1);
    assert_eq!(request["state"]["snapshot"].as_str().unwrap().len(), 64);

    let zero = cli(repo.path(), &["--max-calls", "0"]);
    assert!(zero.status.success());
    let report: Value = serde_json::from_slice(&zero.stdout).unwrap();
    assert_eq!(report["calls_used"], 0);
    assert_eq!(report["search_complete"], false);
    assert!(report["unscreened_questions"].as_u64().unwrap() > 0);
}

#[test]
fn typescript_categories_are_language_specific_and_mixed_diffs_require_selection() {
    let repo = fixture();
    fs::write(
        repo.path().join("app.tsx"),
        "export function A() { return <p>Hello</p>; }\n",
    )
    .unwrap();
    git(repo.path(), &["add", "app.tsx"]);
    let output = cli(repo.path(), &["--dry-run"]);
    assert!(output.status.success());
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(request["state"]["language"], "typescript");
    assert!(request["questions"]
        .as_object()
        .unwrap()
        .values()
        .any(|q| q["instructions"]["category"]["id"] == "nullability"));
    fs::write(repo.path().join("main.rs"), "fn main() { missing(); }\n").unwrap();
    let output = cli(repo.path(), &["--dry-run"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("multiple source languages"));
    let output = cli(repo.path(), &["--dry-run", "--language", "rust"]);
    assert!(output.status.success());
    for args in [["--top-errors", "0"], ["--top-errors", "11"]] {
        assert_eq!(cli(repo.path(), &args).status.code(), Some(2));
    }
}
