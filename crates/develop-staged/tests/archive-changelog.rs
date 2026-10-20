use std::fs;
use std::process::{Command, Output};

use tempfile::TempDir;

const ARCHIVER: &str = env!("CARGO_BIN_EXE_archive-changelog");

#[test]
fn default_mode_prints_plan_without_changing_changelog() {
    let repo = repo_with_changelog(&oversized_changelog());
    let before = fs::read(repo.path().join("CHANGELOG.md")).unwrap();

    let output = run_archiver(repo.path(), &[]);

    assert!(output.status.success(), "{}", output_text(&output));
    assert!(output_text(&output).contains("dry-run"));
    assert_eq!(fs::read(repo.path().join("CHANGELOG.md")).unwrap(), before);
}

#[test]
fn yes_archives_oldest_months_and_records_pre_archive_commit() {
    let repo = repo_with_changelog(&oversized_changelog());
    let before_sha = git_output(repo.path(), &["rev-parse", "HEAD"]);

    let output = run_archiver(repo.path(), &["--yes"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let result = fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap();
    let archived_through = previous_month(&current_month(), 1);
    assert!(result.contains(&format!(
        "> {archived_through} 以前は git 履歴（退避直前の commit: {before_sha}）"
    )));
    assert!(!result.contains(&format!("## {}", previous_month(&current_month(), 1))));
    assert!(!result.contains(&format!("## {}", previous_month(&current_month(), 2))));
    assert!(result.contains(&format!("## {}", current_month())));
    assert!(output_text(&output).contains("退避する月:"));
    assert!(result.len() <= 96_000);
    assert_eq!(
        fs::read_dir(repo.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| name != ".git" && name != "CHANGELOG.md")
            .count(),
        0
    );
}

#[test]
fn archive_without_current_month_keeps_the_newest_existing_month() {
    let newest = previous_month(&current_month(), 1);
    let middle = previous_month(&current_month(), 2);
    let oldest = previous_month(&current_month(), 3);
    let block = |month: &str| {
        format!(
            "## {month}\n- **成果（{month}-01）**: {}\n\n",
            "x".repeat(55_000)
        )
    };
    let content = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n{}{}{}",
        block(&newest),
        block(&middle),
        block(&oldest)
    );
    let repo = repo_with_changelog(&content);
    let before_sha = git_output(repo.path(), &["rev-parse", "HEAD"]);

    let output = run_archiver(repo.path(), &["--yes"]);

    assert!(output.status.success(), "{}", output_text(&output));
    let result = fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap();
    assert!(result.contains(&format!("## {newest}")));
    assert!(!result.contains(&format!("## {middle}")));
    assert!(!result.contains(&format!("## {oldest}")));
    assert!(result.contains(&format!(
        "> {middle} 以前は git 履歴（退避直前の commit: {before_sha}）"
    )));
    assert!(result.len() <= 96_000);
}

#[test]
fn newest_existing_month_overage_fails_without_changing_the_file() {
    let newest = previous_month(&current_month(), 1);
    let older = previous_month(&current_month(), 2);
    let content = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## {newest}\n- **成果（{newest}-01）**: {}\n\n## {older}\n- **成果（{older}-01）**: old\n",
        "x".repeat(120_001)
    );
    let repo = repo_with_changelog(&content);

    let output = run_archiver(repo.path(), &["--yes"]);

    assert!(!output.status.success());
    assert!(output_text(&output).contains("最新の月の項目を畳む"));
    assert_eq!(
        fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap(),
        content
    );
}

#[test]
fn input_at_the_success_cap_stays_unchanged() {
    let content = changelog_of_len(120_000);
    assert_eq!(content.len(), 120_000);
    let repo = repo_with_changelog(&content);

    let output = run_archiver(repo.path(), &["--yes"]);

    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(
        fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap(),
        content
    );
}

#[test]
fn exhausted_archive_succeeds_between_target_and_cap() {
    for result_bytes in [96_001, 120_000] {
        let (content, protected, older) = changelog_whose_archive_result_is(result_bytes);
        assert!(content.len() > 120_000, "{}", content.len());
        let repo = repo_with_changelog(&content);
        let before_sha = git_output(repo.path(), &["rev-parse", "HEAD"]);

        let output = run_archiver(repo.path(), &["--yes"]);

        assert!(
            output.status.success(),
            "{result_bytes}: {}",
            output_text(&output)
        );
        let result = fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap();
        assert_eq!(result.len(), result_bytes, "{result_bytes}");
        assert!(result.contains(&format!("## {protected}")));
        assert!(!result.contains(&format!("## {older}")));
        assert!(result.contains(&format!("退避直前の commit: {before_sha}")));
    }
}

#[test]
fn exhausted_archive_over_the_cap_fails_without_changing_the_file() {
    let (content, _, _) = changelog_whose_archive_result_is(120_001);
    assert!(content.len() > 120_000);
    let repo = repo_with_changelog(&content);

    let output = run_archiver(repo.path(), &["--yes"]);

    assert!(!output.status.success());
    assert!(output_text(&output).contains("当月の項目を畳む"));
    assert_eq!(
        fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap(),
        content
    );
}

#[test]
fn current_month_overage_fails_without_changing_the_file() {
    let month = current_month();
    let content = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## {month}\n- **成果（{month}-01）**: {}\n",
        "x".repeat(120_001)
    );
    let repo = repo_with_changelog(&content);

    let output = run_archiver(repo.path(), &["--yes"]);

    assert!(!output.status.success());
    assert!(output_text(&output).contains("当月の項目を畳む"));
    assert_eq!(
        fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap(),
        content
    );
}

fn repo_with_changelog(content: &str) -> TempDir {
    let repo = tempfile::tempdir().unwrap();
    git_ok(repo.path(), &["init", "-q"]);
    git_ok(
        repo.path(),
        &["config", "user.email", "tests@example.invalid"],
    );
    git_ok(repo.path(), &["config", "user.name", "test"]);
    fs::write(repo.path().join("CHANGELOG.md"), content).unwrap();
    git_ok(repo.path(), &["add", "CHANGELOG.md"]);
    git_ok(repo.path(), &["commit", "-m", "initial changelog"]);
    repo
}

fn run_archiver(repo: &std::path::Path, args: &[&str]) -> Output {
    Command::new(ARCHIVER)
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap()
}

fn current_month() -> String {
    let output = Command::new("date").arg("+%Y-%m").output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn previous_month(month: &str, count: usize) -> String {
    let (year, month_number) = month.split_once('-').unwrap();
    let mut year = year.parse::<i32>().unwrap();
    let mut month_number = month_number.parse::<i32>().unwrap();
    for _ in 0..count {
        month_number -= 1;
        if month_number == 0 {
            year -= 1;
            month_number = 12;
        }
    }
    format!("{year:04}-{month_number:02}")
}

fn changelog_of_len(bytes: usize) -> String {
    let month = current_month();
    let head = format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n## {month}\n- **成果（{month}-01）**: "
    );
    let summary = "x".repeat(bytes - head.len() - 1);
    format!("{head}{summary}\n")
}

fn changelog_whose_archive_result_is(result_bytes: usize) -> (String, String, String) {
    let protected = current_month();
    let older = previous_month(&protected, 1);
    let note = format!(
        "> {older} 以前は git 履歴（退避直前の commit: {}）",
        "a".repeat(40)
    );
    let preamble = format!("# Changelog\n{note}\n\n");
    let prefix = format!("## {protected}\n- **成果（{protected}-01）**: ");
    let summary = "x".repeat(result_bytes - preamble.len() - prefix.len() - 1);
    let protected_section = format!("{prefix}{summary}\n");
    let older_section = format!(
        "## {older}\n- **成果（{older}-01）**: {}\n",
        "y".repeat(40_000)
    );
    let content =
        format!("# Changelog\n> 古い分は git 履歴に任せる。\n\n{protected_section}{older_section}");
    (content, protected, older)
}

fn oversized_changelog() -> String {
    let month = current_month();
    let block = |month: &str| {
        format!(
            "## {month}\n- **成果（{month}-01）**: {}\n\n",
            "x".repeat(55_000)
        )
    };
    format!(
        "# Changelog\n> 古い分は git 履歴に任せる。\n\n{}{}{}",
        block(&month),
        block(&previous_month(&month, 1)),
        block(&previous_month(&month, 2))
    )
}

fn git_ok(repo: &std::path::Path, args: &[&str]) {
    let output = develop_core::git(repo).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(repo: &std::path::Path, args: &[&str]) -> String {
    let output = develop_core::git(repo).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
