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
fn english_changelog_reads_language_and_stamp_gate_from_index() {
    let repo = repository();
    fs::create_dir(repo.path().join(".config")).unwrap();
    let manifest = repo.path().join(".config/develop-tools.json");
    let declared = r#"{"release":{"changelogStamp":true,"notes":{"mode":"changelog-section","language":"en"}}}"#;
    fs::write(&manifest, declared).unwrap();
    fs::write(repo.path().join("CHANGELOG.md"), "# Changelog\n> Older entries remain in git history.\n\n## 2026-11\n\n### 1.2.3 (2026-11-09)\n\nSummary.\n\n- **Stop (2026-11-09/v1.2.3, fast pane)**: Shares a deadline.\n").unwrap();
    git_ok(repo.path(), &["add", "."]);
    fs::write(&manifest, "{}").unwrap();
    let out = run_checker(repo.path(), &[]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    git_ok(repo.path(), &["add", ".config/develop-tools.json"]);
    fs::write(&manifest, declared).unwrap();
    let out = run_checker(repo.path(), &[]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
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
        "x".repeat(360_001)
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

fn write_manifest(root: &Path, source: &str) {
    fs::create_dir_all(root.join(".config")).unwrap();
    fs::write(root.join(".config/develop-tools.json"), source).unwrap();
}

fn stamped_changelog() -> &'static str {
    "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2026-11\n- **成果（2026-11-05/⏳）**: 要約\n"
}

#[test]
fn checker_selects_changelog_shape_from_index_manifest() {
    for (manifest, changelog, expected) in [
        (
            Some(r#"{"release":{"changelogStamp":true}}"#),
            stamped_changelog(),
            0,
        ),
        (
            Some(r#"{"release":{"changelogStamp":true}}"#),
            valid_changelog(),
            1,
        ),
        (None, stamped_changelog(), 1),
        (Some("{}"), valid_changelog(), 0),
        (Some("not JSON"), stamped_changelog(), 2),
        (
            Some(r#"{"release":{"changelogStamp":"yes"}}"#),
            stamped_changelog(),
            2,
        ),
    ] {
        let repo = repository();
        fs::write(repo.path().join("CHANGELOG.md"), changelog).unwrap();
        if let Some(manifest) = manifest {
            write_manifest(repo.path(), manifest);
        }
        git_ok(repo.path(), &["add", "."]);
        write_manifest(repo.path(), r#"{"release":{"changelogStamp":false}}"#);
        let output = run_checker(repo.path(), &[]);
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{manifest:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn checker_uses_manifest_from_explicit_index() {
    let repo = repository();
    write_manifest(repo.path(), "{}");
    fs::write(repo.path().join("CHANGELOG.md"), valid_changelog()).unwrap();
    git_ok(repo.path(), &["add", "."]);
    let alternate = repo.path().join(".git/alternate-index");
    fs::copy(repo.path().join(".git/index"), &alternate).unwrap();
    write_manifest(repo.path(), r#"{"release":{"changelogStamp":true}}"#);
    fs::write(repo.path().join("CHANGELOG.md"), stamped_changelog()).unwrap();
    assert!(
        git_with_index(repo.path(), &alternate, &["add", "."])
            .status
            .success()
    );
    write_manifest(repo.path(), "broken working tree");
    let output = run_checker_args(repo.path(), &["--index-file", ".git/alternate-index"], &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(run_checker(repo.path(), &[]).status.code(), Some(0));
}

fn commit(root: &Path, message: &str) -> String {
    git_ok(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            message,
        ],
    );
    String::from_utf8(git(root, &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_owned()
}

fn link_history() -> (tempfile::TempDir, String, String) {
    let repo = repository();
    git_ok(repo.path(), &["checkout", "-q", "-b", "work"]);
    git_ok(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/current.git",
        ],
    );
    fs::create_dir(repo.path().join("config")).unwrap();
    fs::write(repo.path().join("config/island-docs-scopes.json"), r#"{"version":1,"byteLimits":[{"scope":"all","path":"CHANGELOG.md","maxBytes":120000}],"linkSourceExclusions":[],"hostPathLinkAllowScopes":[],"indexRecordDirs":[{"scope":"all","dirs":["reviews"]}],"indexRequirements":[],"docsLayouts":[]}"#).unwrap();
    git_ok(repo.path(), &["add", "."]);
    commit(repo.path(), "base");
    git_ok(repo.path(), &["checkout", "-q", "-b", "side"]);
    let side = commit(repo.path(), "side implementation");
    git_ok(repo.path(), &["checkout", "-q", "work"]);
    let head = commit(repo.path(), "work implementation");
    (repo, head, side)
}

fn linked_changelog(sha: &str) -> String {
    format!(
        "{} [{}](https://github.com/example/current/commit/{sha})\n",
        valid_changelog().trim_end(),
        sha
    )
}

fn stage_changelog(root: &Path, text: &str) {
    fs::write(root.join("CHANGELOG.md"), text).unwrap();
    git_ok(root, &["add", "CHANGELOG.md"]);
}

/// Compare the actual develop-docs-check engine's changelog errors, excluding unrelated rules.
fn assert_changelog_parity(root: &Path, index: Option<&Path>, expected: i32) {
    // The engine initializes its process-wide scope table once, just like its CLI.
    let mut probe = Command::new(std::env::current_exe().unwrap());
    probe
        .args([
            "--ignored",
            "--exact",
            "docs_checker_parity_probe",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("STAGED_DOCS_PARITY_ROOT", root);
    if let Some(index) = index {
        probe.env("STAGED_DOCS_PARITY_INDEX", index);
    } else {
        probe.env_remove("STAGED_DOCS_PARITY_INDEX");
    }
    let probe = probe.output().unwrap();
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let common: Vec<(Option<usize>, String)> = String::from_utf8_lossy(&probe.stderr)
        .lines()
        .filter_map(|line| line.strip_prefix("parity:"))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let args: Vec<_> = index
        .map(|p| vec!["--index-file", p.to_str().unwrap()])
        .unwrap_or_default();
    let output = run_checker_args(root, &args, &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(expected), "{stderr}");
    assert_eq!(common.is_empty(), expected == 0, "{common:?}");
    for (line, message) in &common {
        assert!(
            stderr.contains(&format!("CHANGELOG.md:{}: {message}", line.unwrap())),
            "{stderr}"
        );
    }
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.starts_with("error:"))
            .count(),
        common.len(),
        "{stderr}"
    );
}

#[test]
#[ignore = "subprocess probe: each checker invocation owns one scope table"]
fn docs_checker_parity_probe() {
    use island_docs::{check, git::Repo, rules::Rule};
    let root = std::env::var_os("STAGED_DOCS_PARITY_ROOT").expect("probe root");
    let index = std::env::var_os("STAGED_DOCS_PARITY_INDEX");
    let repo = Repo::discover(Path::new(&root), index.as_deref().map(Path::new)).unwrap();
    let outcome = check::run(&repo, check::Mode::Staged).unwrap();
    for finding in outcome.findings {
        if let check::Finding::Rule(f) = finding
            && f.rule == Rule::Changelog
        {
            eprintln!(
                "parity:{}",
                serde_json::to_string(&(f.line, f.message)).unwrap()
            );
        }
    }
}

#[test]
fn commit_links_use_index_and_head_with_common_checker_parity() {
    let (repo, head, side) = link_history();
    // The implementation is HEAD; its parent does not contain it.
    let good = linked_changelog(&head[..8]);
    let bad = linked_changelog(&side[..8]);
    stage_changelog(repo.path(), &good);
    fs::write(repo.path().join("CHANGELOG.md"), &bad).unwrap();
    assert_changelog_parity(repo.path(), None, 0);

    stage_changelog(repo.path(), &bad);
    fs::write(repo.path().join("CHANGELOG.md"), &good).unwrap();
    assert_changelog_parity(repo.path(), None, 1);
    let output = run_checker(repo.path(), &[]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("祖先ではありません"));

    stage_changelog(repo.path(), &good);
    let alternate = repo.path().join(".git/link-index");
    fs::copy(repo.path().join(".git/index"), &alternate).unwrap();
    fs::write(repo.path().join("CHANGELOG.md"), &bad).unwrap();
    assert!(
        git_with_index(repo.path(), &alternate, &["add", "CHANGELOG.md"])
            .status
            .success()
    );
    fs::write(repo.path().join("CHANGELOG.md"), "broken working tree").unwrap();
    assert_changelog_parity(repo.path(), None, 0);
    assert_changelog_parity(repo.path(), Some(&alternate), 1);
    // An inherited index must not replace either explicitly selected snapshot.
    let output = run_checker(
        repo.path(),
        &[("GIT_INDEX_FILE", alternate.to_str().unwrap())],
    );
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn commit_link_errors_and_optional_links_match_common_checker() {
    let (repo, head, _) = link_history();
    for (text, expected) in [
        (linked_changelog(&head[..7]), 0),
        (
            linked_changelog(&head).replace("example/current", "other/repo"),
            1,
        ),
        (linked_changelog("0000000"), 1),
        (
            linked_changelog(&head[..8]).replace(&format!("[{}]", &head[..8]), "[abcdef0]"),
            1,
        ),
        (
            format!(
                "{} [資料](https://example.invalid/docs)\n",
                valid_changelog().trim_end()
            ),
            0,
        ),
        (valid_changelog().to_owned(), 0),
    ] {
        stage_changelog(repo.path(), &text);
        assert_changelog_parity(repo.path(), None, expected);
    }
}

#[test]
fn unchanged_changelog_does_not_block_staged_todo_with_bad_commit_link() {
    let (repo, _, side) = link_history();
    stage_changelog(repo.path(), &linked_changelog(&side[..8]));
    commit(repo.path(), "pre-existing invalid link");
    fs::write(repo.path().join("TODO.md"), valid_todo()).unwrap();
    git_ok(repo.path(), &["add", "TODO.md"]);
    assert_eq!(run_checker(repo.path(), &[]).status.code(), Some(0));
}

#[test]
fn commit_link_bytes_still_count_towards_warning_and_limit() {
    let (repo, head, _) = link_history();
    for (length, expected) in [(500, 0), (360_000, 1)] {
        let text = linked_changelog(&head[..8]).replace("要約", &"x".repeat(length));
        stage_changelog(repo.path(), &text);
        let output = run_checker(repo.path(), &[]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(expected), "{stderr}");
        assert!(
            stderr.contains("warning:") && stderr.contains("500"),
            "{stderr}"
        );
        if expected == 1 {
            assert!(stderr.contains("360000"), "{stderr}");
        }
    }
}

#[test]
fn archive_changelog_preserves_retained_commit_link_suffix_bytes() {
    let (repo, head, _) = link_history();
    let retained = format!(
        "- **残す（2099-12-31）**: 要約 [{}](https://github.com/example/current/commit/{})\n",
        &head[..8],
        &head[..8]
    );
    let text = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2099-12\n{retained}\n## 2000-01\n- **古い成果（2000-01-01）**: {}\n",
        "x".repeat(361_000)
    );
    fs::write(repo.path().join("CHANGELOG.md"), text).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_archive-changelog"))
        .current_dir(repo.path())
        .arg("--yes")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let archived = fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap();
    assert!(archived.ends_with(&format!("{retained}\n")), "{archived}");
    assert!(!archived.contains("## 2000-01"));
    // Exercise the protected-month date pruning path as well.
    let text = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## 2099-12\n{retained}- **古い成果（2099-12-01）**: {}\n",
        "x".repeat(361_000)
    );
    let plan =
        develop_staged::docs_standard::plan_changelog_archive(&text, "2099-12", &head).unwrap();
    assert_eq!(plan.archived_dates, ["2099-12-01"]);
    assert!(plan.content.ends_with(&retained));
}

#[test]
fn staged_version_items_use_release_date_instead_of_month() {
    let repo = repository();
    fs::create_dir(repo.path().join(".config")).unwrap();
    fs::write(repo.path().join(".config/develop-tools.json"), r#"{"release":{"changelogStamp":true,"notes":{"mode":"changelog-section","language":"en"}}}"#).unwrap();
    for (date, exit) in [("2026-10-31", 0), ("2026-11-09", 0), ("2026-11-10", 1)] {
        fs::write(repo.path().join("CHANGELOG.md"), format!("# Changelog\n> Older entries remain in git history.\n\n## 2026-11\n\n### 1.2.3 (2026-11-09)\n\nSummary.\n\n- **October ({date}/v1.2.3)**: Carried over.\n")).unwrap();
        git_ok(repo.path(), &["add", "."]);
        fs::write(repo.path().join("CHANGELOG.md"), "unstaged broken document").unwrap();
        let out = run_checker(repo.path(), &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(exit), "{date}: {stderr}");
        if exit == 1 {
            assert!(stderr.contains("版小節の日付より後"), "{stderr}");
        }
    }
}
