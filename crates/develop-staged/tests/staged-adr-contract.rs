//! `check-staged-adr` が ADR 番号の契約を index の `.develop-tools.json` から読むこと。
//!
//! 宣言が無ければ失敗する。島の台帳に載る repo でも縮退しないことは、島の配置に依るので
//! 非公開側の `tools/develop-health/tests/staged-adr.rs` が確かめる。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const CHECKER: &str = env!("CARGO_BIN_EXE_check-staged-adr");

const STANDARD_ADR: &str = "---\nstatus: accepted\ndate: 2026-09-11\n---\n\n# ADR-0001: decision\n\n## Context and Problem Statement\n\ncontext\n\n## Decision Drivers\n\n- driver\n\n## Considered Options\n\n- option\n\n## Decision Outcome\n\nChosen option: option\n\n### Consequences\n\n- Good: result\n- Bad: cost\n- Neutral: effect\n\n### Confirmation\n\nmanual review\n";

const DECLARATION: &str = r#"{"adrMigratedThrough":0}"#;

fn git_ok(repo: &Path, args: &[&str]) {
    let output = develop_core::git(repo).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// ADR を 1 本 staged にした repo。宣言はまだ置かない。
fn repo_with_staged_adr() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join("docs/adr")).unwrap();
    git_ok(&repo, &["init", "-q"]);
    fs::write(repo.join("docs/adr/0001-new.md"), STANDARD_ADR).unwrap();
    fs::write(repo.join("docs/adr/README.md"), "- [0001](./0001-new.md)\n").unwrap();
    git_ok(&repo, &["add", "docs/adr"]);
    (dir, repo)
}

fn run_checker(repo: &Path) -> Output {
    std::process::Command::new(CHECKER)
        .current_dir(repo)
        .output()
        .unwrap()
}

#[test]
fn repo_without_declaration_fails() {
    let (_dir, repo) = repo_with_staged_adr();
    let out = run_checker(&repo);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(".develop-tools.json"), "{err}");
}

#[test]
fn index_declaration_is_read_instead_of_the_worktree() {
    let (_dir, repo) = repo_with_staged_adr();
    fs::write(repo.join(".develop-tools.json"), DECLARATION).unwrap();
    git_ok(&repo, &["add", ".develop-tools.json"]);
    // 作業ツリーだけを壊す。index を読めば通る。
    fs::write(repo.join(".develop-tools.json"), "not json").unwrap();
    let out = run_checker(&repo);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn worktree_only_declaration_is_not_read() {
    let (_dir, repo) = repo_with_staged_adr();
    fs::write(repo.join(".develop-tools.json"), DECLARATION).unwrap();
    let out = run_checker(&repo);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn explicit_index_without_declaration_fails_even_when_the_default_index_has_one() {
    let (_dir, repo) = repo_with_staged_adr();
    let alternate = repo.join(".git/alternate-index");
    fs::copy(repo.join(".git/index"), &alternate).unwrap();
    fs::write(repo.join(".develop-tools.json"), DECLARATION).unwrap();
    git_ok(&repo, &["add", ".develop-tools.json"]);

    let explicit = std::process::Command::new(CHECKER)
        .current_dir(&repo)
        .arg("--index-file")
        .arg(&alternate)
        .output()
        .unwrap();
    assert_eq!(explicit.status.code(), Some(2), "{explicit:?}");
    let err = String::from_utf8_lossy(&explicit.stderr);
    assert!(err.contains(".develop-tools.json"), "{err}");
    let default = run_checker(&repo);
    assert_eq!(
        default.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&default.stderr)
    );
}
