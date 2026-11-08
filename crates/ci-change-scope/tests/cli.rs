use std::{fs, path::Path, process::Command};

use ci_change_scope::{Scope, changed_paths, classify_paths};

fn git(root: &Path, args: &[&str]) -> String {
    let mut command = develop_tools_core::git(root);
    command.args(args);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(root: &Path, path: &str, contents: &str) -> String {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, contents).unwrap();
    git(root, &["add", "--", path]);
    git(root, &["commit", "-q", "-m", path]);
    git(root, &["rev-parse", "HEAD"])
}

#[test]
fn every_commit_is_inspected_even_when_a_disallowed_change_is_reverted() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "Test"]);
    let before = commit(root, "TODO.md", "before\n");
    commit(root, "config/x.json", "temporary\n");
    git(root, &["rm", "-q", "config/x.json"]);
    git(root, &["commit", "-q", "-m", "remove temporary config"]);
    let head = commit(root, "docs/reviews/a.md", "# Record\n");

    let paths = changed_paths(root, &before, &head).unwrap();
    assert!(paths.contains(&"config/x.json".to_owned()));
    assert_eq!(classify_paths(Some(&paths)), Scope::Heavy);
    assert_cli(root, "push", "false", &before, &head, false);
    assert!(changed_paths(root, &"f".repeat(40), &head).is_none());
}

#[test]
fn all_commits_with_allowed_documents_are_light() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "Test"]);
    let before = commit(root, "README.md", "base\n");
    commit(root, "TODO.md", "task\n");
    let head = commit(root, "docs/reviews/nested/record.md", "# Record\n");

    let paths = changed_paths(root, &before, &head).unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(classify_paths(Some(&paths)), Scope::Light);
}

#[test]
fn cli_prints_only_a_safe_output_when_scope_is_unknown() {
    let output = Command::new(env!("CARGO_BIN_EXE_ci-change-scope"))
        .env("GITHUB_EVENT_NAME", "push")
        .env("FORCED_PUSH", "false")
        .env("BEFORE_SHA", "0".repeat(40))
        .env("HEAD_SHA", "a".repeat(40))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"light=false\n");
}

#[test]
fn cli_selects_light_for_a_document_only_push() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "Test"]);
    let before = commit(root, "README.md", "base\n");
    let head = commit(root, "docs/reviews/record.md", "# Record\n");

    let output = Command::new(env!("CARGO_BIN_EXE_ci-change-scope"))
        .current_dir(root)
        .env("GITHUB_EVENT_NAME", "push")
        .env("FORCED_PUSH", "false")
        .env("BEFORE_SHA", before)
        .env("HEAD_SHA", head)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"light=true\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn cli_selects_light_for_a_scheduled_or_manual_range() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "Test"]);
    let before = commit(root, "README.md", "base\n");
    let head = commit(root, "docs/reviews/record.md", "# Record\n");

    for event in ["schedule", "workflow_dispatch"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ci-change-scope"))
            .current_dir(root)
            .env("GITHUB_EVENT_NAME", event)
            .env("FORCED_PUSH", "false")
            .env("BEFORE_SHA", &before)
            .env("HEAD_SHA", &head)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"light=true\n", "event: {event}");
    }
}

// 有効な履歴を使い、入力条件だけが heavy を選ぶことを確かめる。
fn assert_cli(root: &Path, event: &str, forced: &str, before: &str, head: &str, light: bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ci-change-scope"));
    command.current_dir(root);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .env("GITHUB_EVENT_NAME", event)
        .env("FORCED_PUSH", forced)
        .env("BEFORE_SHA", before)
        .env("HEAD_SHA", head)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, format!("light={light}\n").as_bytes());
}

fn scope_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q"]);
    git(
        repo.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(repo.path(), &["config", "user.name", "Test"]);
    git(repo.path(), &["config", "commit.gpgsign", "false"]);
    repo
}

#[test]
fn cli_fails_closed_for_event_force_and_range_boundaries() {
    let repo = scope_repo();
    let root = repo.path();
    let before = commit(root, "README.md", "base\n");
    let head = commit(root, "TODO.md", "task\n");
    assert_cli(root, "push", "false", &before, &head, true);
    for (event, forced) in [
        ("pull_request", "false"),
        ("push", "true"),
        ("push", "null"),
    ] {
        assert_cli(root, event, forced, &before, &head, false);
    }
    for invalid in [
        "0".repeat(40),
        "invalid".into(),
        "g".repeat(40),
        "f".repeat(40),
    ] {
        assert!(changed_paths(root, &invalid, &head).is_none());
        assert_cli(root, "push", "false", &invalid, &head, false);
    }
    assert_eq!(changed_paths(root, &head, &head), Some(vec![]));
    assert_cli(root, "push", "false", &head, &head, false);
    // 両方の commit は存在するが、before は head の祖先ではない。
    assert!(changed_paths(root, &head, &before).is_none());
    assert_cli(root, "push", "false", &head, &before, false);
}

#[test]
fn cli_counts_both_sides_of_a_rename_from_outside_the_allowed_set() {
    let repo = scope_repo();
    let root = repo.path();
    let before = commit(root, "docs/reference/a.md", "record\n");
    fs::create_dir_all(root.join("docs/reviews")).unwrap();
    git(root, &["mv", "docs/reference/a.md", "docs/reviews/a.md"]);
    git(root, &["commit", "-q", "-m", "move record"]);
    let head = git(root, &["rev-parse", "HEAD"]);
    let paths = changed_paths(root, &before, &head).unwrap();
    assert!(paths.contains(&"docs/reference/a.md".to_owned()));
    assert!(paths.contains(&"docs/reviews/a.md".to_owned()));
    assert_cli(root, "push", "false", &before, &head, false);
}

#[cfg(target_os = "linux")]
#[test]
fn cli_fails_closed_for_a_non_utf8_path() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let repo = scope_repo();
    let root = repo.path();
    let before = commit(root, "TODO.md", "base\n");
    fs::create_dir_all(root.join("docs/reviews")).unwrap();
    fs::write(
        root.join(OsStr::from_bytes(b"docs/reviews/invalid-\xff.md")),
        "record\n",
    )
    .unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "invalid path"]);
    let head = git(root, &["rev-parse", "HEAD"]);
    assert!(changed_paths(root, &before, &head).is_none());
    assert_cli(root, "push", "false", &before, &head, false);
}
