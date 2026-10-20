use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> String {
    let output = develop_tools_core::git(root)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgSign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit(root: &Path, changelog: &str, message: &str) {
    std::fs::write(root.join("CHANGELOG.md"), changelog).unwrap();
    git(root, &["add", "CHANGELOG.md"]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            message,
        ],
    );
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:example/project.git",
        ],
    );
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n",
        "initial",
    );
    dir
}

fn notes(root: &Path, tag: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_release-notes"))
        .arg(tag)
        .current_dir(root)
        .output()
        .unwrap()
}

#[test]
fn previous_tag_is_required_even_for_the_first_release() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    let output = notes(dir.path(), "v0.1.0");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn only_added_monthly_items_between_adjacent_tags_are_rendered() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **先の版の項目**\n",
        "first range",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **先の版の項目**\n- **今回の項目**\n\n```md\n- **fence 内**\n```\n\n## その他\n- **月外**\n\n## 2026-10\n- **今月の項目**\n",
        "second range",
    );
    git(dir.path(), &["tag", "v0.3.0"]);
    let output = notes(dir.path(), "v0.3.0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "変更範囲: [v0.2.0...v0.3.0](https://github.com/example/project/compare/v0.2.0...v0.3.0)\n\n- **今回の項目**\n- **今月の項目**\n"
    );
}

#[test]
fn empty_additions_keep_only_the_compare_link() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## その他\n- **月外**\n",
        "no monthly additions",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    let output = notes(dir.path(), "v0.2.0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "変更範囲: [v0.1.0...v0.2.0](https://github.com/example/project/compare/v0.1.0...v0.2.0)\n"
    );
}

#[test]
fn non_semver_previous_tag_fails_closed() {
    let dir = fixture();
    git(dir.path(), &["tag", "draft"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-10\n- **項目**\n",
        "change",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    let output = notes(dir.path(), "v0.2.0");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn invocation_from_a_subdirectory_still_reads_the_root_changelog() {
    let dir = fixture();
    git(dir.path(), &["tag", "v0.1.0"]);
    commit(
        dir.path(),
        "# Changelog\n\n## 2026-09\n- **以前の項目**\n- **追加項目**\n",
        "change",
    );
    git(dir.path(), &["tag", "v0.2.0"]);
    std::fs::create_dir(dir.path().join("tools")).unwrap();
    let output = notes(&dir.path().join("tools"), "v0.2.0");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("- **追加項目**")
    );
}
