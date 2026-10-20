use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const CHECKER: &str = env!("CARGO_BIN_EXE_check-staged-docs");

fn git(root: &Path, args: &[&str]) -> Output {
    develop_core::git(root)
        .args(args)
        .output()
        .expect("git starts")
}

fn git_ok(root: &Path, args: &[&str]) {
    let output = git(root, args);
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_with_index(root: &Path, index_file: &Path, args: &[&str]) -> Output {
    develop_core::git(root)
        .env("GIT_INDEX_FILE", index_file)
        .args(args)
        .output()
        .expect("git starts")
}

fn repository() -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    git_ok(temp.path(), &["init", "-q"]);
    temp
}

fn run_checker(root: &Path, extra_env: &[(&str, &str)]) -> Output {
    run_checker_args(root, &[], extra_env)
}

fn run_checker_args(root: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(CHECKER);
    command.current_dir(root);
    command.args(args);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.output().expect("checker starts")
}

fn valid_todo() -> &'static str {
    "- 現在地: 仕様を確認した → 次: CLI をつなぐ → 完了条件: 検査が通り [SoT](https://example.invalid/sot) に反映\n"
}

fn valid_changelog() -> &'static str {
    "# Changelog\n> 古い分は git 履歴に任せる\n\n## 2026-09\n- **成果を追加した（2026-09-26、v1.2 を反映）**: 要約\n"
}

#[test]
fn checker_reads_staged_content_instead_of_working_tree_content() {
    let repo = repository();
    fs::write(repo.path().join("TODO.md"), valid_todo()).unwrap();
    git_ok(repo.path(), &["add", "TODO.md"]);

    fs::write(repo.path().join("TODO.md"), "- [x] unstaged\n").unwrap();
    let unstaged_bad = run_checker(repo.path(), &[]);
    assert_eq!(unstaged_bad.status.code(), Some(0));

    git_ok(repo.path(), &["add", "TODO.md"]);
    fs::write(repo.path().join("TODO.md"), valid_todo()).unwrap();
    let staged_bad = run_checker(repo.path(), &[]);
    assert_eq!(staged_bad.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&staged_bad.stderr);
    assert!(stderr.contains("TODO.md"), "{stderr}");
    assert!(stderr.contains("[x]"), "{stderr}");
}

#[test]
fn checker_warns_for_a_long_todo_item_without_rejecting_it() {
    let repo = repository();
    let long_summary = "概要".repeat(320);
    let todo = format!(
        "- 現在地: 完了条件を確認した → 次: 実装する → 完了条件: {long_summary} [SoT](https://example.invalid/sot)\n"
    );
    fs::write(repo.path().join("TODO.md"), todo).unwrap();
    git_ok(repo.path(), &["add", "TODO.md"]);

    let output = run_checker(repo.path(), &[]);
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning"), "{stderr}");
    assert!(stderr.contains("500"), "{stderr}");
}

#[test]
fn checker_rejects_a_staged_document_over_its_byte_limit() {
    let repo = repository();
    let oversized = format!("{}\n", "x".repeat(36_000));
    fs::write(repo.path().join("TODO.md"), oversized).unwrap();
    git_ok(repo.path(), &["add", "TODO.md"]);

    let output = run_checker(repo.path(), &[]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&develop_staged::docs_standard::TODO_MAX_BYTES.to_string()),
        "{stderr}"
    );
}

#[test]
fn checker_recommends_archiving_an_oversized_staged_changelog() {
    let repo = repository();
    let changelog = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n- **成果（2026-09-27）**: {}\n",
        "x".repeat(120_001)
    );
    fs::write(repo.path().join("CHANGELOG.md"), changelog).unwrap();
    git_ok(repo.path(), &["add", "CHANGELOG.md"]);

    let output = run_checker(repo.path(), &[]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&develop_staged::docs_standard::CHANGELOG_MAX_BYTES.to_string()),
        "{stderr}"
    );
    assert!(stderr.contains("archive-changelog"), "{stderr}");
}

#[test]
fn checker_ignores_nested_todo_files() {
    let repo = repository();
    fs::create_dir(repo.path().join("child")).unwrap();
    fs::write(repo.path().join("child/TODO.md"), "- [x] nested\n").unwrap();
    git_ok(repo.path(), &["add", "child/TODO.md"]);

    let output = run_checker(repo.path(), &[]);
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn checker_rejects_invalid_changelog_and_accepts_a_date_suffix() {
    let repo = repository();
    fs::write(repo.path().join("CHANGELOG.md"), valid_changelog()).unwrap();
    git_ok(repo.path(), &["add", "CHANGELOG.md"]);

    let valid = run_checker(repo.path(), &[]);
    assert_eq!(
        valid.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );

    fs::write(
        repo.path().join("CHANGELOG.md"),
        "# Wrong\n## 2026-09\n- **成果（2026-08-26）**: 要約\n",
    )
    .unwrap();
    git_ok(repo.path(), &["add", "CHANGELOG.md"]);
    let invalid = run_checker(repo.path(), &[]);
    assert_eq!(invalid.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(stderr.contains("CHANGELOG.md"), "{stderr}");
}

#[test]
fn checker_drops_inherited_git_environment_before_reading_index() {
    let repo = repository();
    fs::write(repo.path().join("TODO.md"), "- [x] staged\n").unwrap();
    git_ok(repo.path(), &["add", "TODO.md"]);

    let output = run_checker(
        repo.path(),
        &[
            ("GIT_DIR", "/missing/decoy/.git"),
            ("GIT_INDEX_FILE", "/missing/decoy/index"),
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("TODO.md"), "{stderr}");
    assert!(stderr.contains("[x]"), "{stderr}");
}

#[test]
fn checker_reads_the_explicit_index_relative_to_its_starting_directory() {
    let repo = repository();
    fs::write(repo.path().join("TODO.md"), valid_todo()).unwrap();
    git_ok(repo.path(), &["add", "TODO.md"]);

    let alternate_index = repo.path().join(".git").join("alternate-index");
    fs::copy(repo.path().join(".git/index"), &alternate_index).unwrap();
    fs::write(
        repo.path().join("TODO.md"),
        "- [x] alternate index violation\n",
    )
    .unwrap();
    let staged_alternate = git_with_index(repo.path(), &alternate_index, &["add", "TODO.md"]);
    assert!(
        staged_alternate.status.success(),
        "{}",
        String::from_utf8_lossy(&staged_alternate.stderr)
    );

    let default_index = run_checker(repo.path(), &[]);
    assert_eq!(default_index.status.code(), Some(0));

    let relative_index = alternate_index
        .strip_prefix(repo.path())
        .expect("alternate index is under the repository");
    let relative_index = relative_index.to_str().unwrap();
    let explicit_index = run_checker_args(repo.path(), &["--index-file", relative_index], &[]);

    assert_eq!(explicit_index.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&explicit_index.stderr);
    assert!(stderr.contains("TODO.md"), "{stderr}");
    assert!(stderr.contains("[x]"), "{stderr}");
}

#[test]
fn checker_rejects_an_explicit_index_outside_the_current_git_directory() {
    let repo = repository();
    fs::write(
        repo.path().join("TODO.md"),
        "- [x] default index violation\n",
    )
    .unwrap();
    git_ok(repo.path(), &["add", "TODO.md"]);

    let outside = tempfile::tempdir().unwrap();
    let external_index = outside.path().join("index");
    fs::write(&external_index, b"not a git index").unwrap();
    let external_index = external_index.to_str().unwrap();

    let rejected = run_checker_args(repo.path(), &["--index-file", external_index], &[]);
    assert_eq!(rejected.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.contains("現在の worktree の Git directory 配下"),
        "{stderr}"
    );

    let missing_value = run_checker_args(repo.path(), &["--index-file"], &[]);
    assert_eq!(missing_value.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing_value.stderr).contains("path が必要"));

    let unreadable_index = repo.path().join(".git").join("missing-index");
    let unreadable_index = unreadable_index.to_str().unwrap();
    let rejected = run_checker_args(repo.path(), &["--index-file", unreadable_index], &[]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("明示 index を読み取れません"));

    let corrupt_index = repo.path().join(".git").join("corrupt-index");
    fs::write(&corrupt_index, b"not a git index").unwrap();
    let corrupt_index = corrupt_index.to_str().unwrap();
    let rejected = run_checker_args(repo.path(), &["--index-file", corrupt_index], &[]);
    assert_eq!(rejected.status.code(), Some(2));
    assert!(
        !String::from_utf8_lossy(&rejected.stderr).contains("TODO.md"),
        "an invalid explicit index must not fall back to the bad default index"
    );

    let duplicate = run_checker_args(
        repo.path(),
        &[
            "--index-file",
            unreadable_index,
            "--index-file",
            unreadable_index,
        ],
        &[],
    );
    assert_eq!(duplicate.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("1 回だけ"));

    let option_as_value = run_checker_args(repo.path(), &["--index-file", "--help"], &[]);
    assert_eq!(option_as_value.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&option_as_value.stderr).contains("path が必要"));
}

#[test]
fn checker_uses_the_explicit_index_inside_a_linked_worktrees_git_directory() {
    let workspace = tempfile::tempdir().unwrap();
    let main_repo = workspace.path().join("main");
    fs::create_dir(&main_repo).unwrap();
    git_ok(&main_repo, &["init", "-q"]);
    git_ok(&main_repo, &["config", "user.name", "Fixture"]);
    git_ok(
        &main_repo,
        &["config", "user.email", "fixture@example.invalid"],
    );
    fs::write(main_repo.join("TODO.md"), valid_todo()).unwrap();
    git_ok(&main_repo, &["add", "TODO.md"]);
    git_ok(&main_repo, &["commit", "-q", "-m", "fixture"]);

    let linked = workspace.path().join("linked");
    let linked_arg = linked.to_str().unwrap();
    git_ok(
        &main_repo,
        &["worktree", "add", "-q", "-b", "linked", linked_arg, "HEAD"],
    );

    let git_dir_output = git(&linked, &["rev-parse", "--absolute-git-dir"]);
    assert!(git_dir_output.status.success());
    let git_dir = Path::new(std::str::from_utf8(&git_dir_output.stdout).unwrap().trim());
    let alternate_index = git_dir.join("alternate-index");
    fs::copy(git_dir.join("index"), &alternate_index).unwrap();

    fs::write(
        linked.join("TODO.md"),
        "- [x] linked alternate index violation\n",
    )
    .unwrap();
    let staged_alternate = git_with_index(&linked, &alternate_index, &["add", "TODO.md"]);
    assert!(
        staged_alternate.status.success(),
        "{}",
        String::from_utf8_lossy(&staged_alternate.stderr)
    );

    let default_index = run_checker(&linked, &[]);
    assert_eq!(default_index.status.code(), Some(0));
    let alternate_index = alternate_index.to_str().unwrap();
    let explicit_index = run_checker_args(&linked, &["--index-file", alternate_index], &[]);
    assert_eq!(explicit_index.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&explicit_index.stderr);
    assert!(stderr.contains("TODO.md"), "{stderr}");
    assert!(stderr.contains("[x]"), "{stderr}");
}
