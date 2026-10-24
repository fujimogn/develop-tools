use std::path::Path;
use std::process::{Command, Output};

fn git(root: &Path, args: &[&str]) {
    let out = develop_tools_core::git(root).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `manifest` を宣言に持つ 1 commit の repo を作る。
fn repo(manifest: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("package.json"), "{\"version\":\"1.2.3\"}\n").unwrap();
    std::fs::write(root.join(".develop-tools.json"), manifest).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "initial"]);
    dir
}

fn check_version(root: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_check-release-version"));
    develop_tools_core::strip_git_env(&mut command);
    command.current_dir(root).output().unwrap()
}

#[test]
fn soft_skips_repo_without_release_declaration() {
    for manifest in [
        "{\"integrationBranch\": \"develop\"}\n",
        "{\"release\": {}}\n",
        "{\"release\": {\"changelogStamp\": true}}\n",
    ] {
        let dir = repo(manifest);
        let output = check_version(dir.path());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{manifest}: {stderr}");
        assert_eq!(stderr.lines().count(), 1, "{manifest}: {stderr}");
        assert!(
            stderr.contains("release.versionFiles"),
            "{manifest}: {stderr}"
        );
    }
}

#[test]
fn broken_release_declaration_still_fails() {
    for manifest in [
        "{\"release\": \n",
        "{\"release\": \"package.json\"}\n",
        "{\"release\": {\"changelogStamp\": \"yes\"}}\n",
        "{\"release\": {\"versionFiles\": []}}\n",
        "{\"release\": {\"versionFiles\": \"package.json\"}}\n",
        "{\"release\": {\"versionFiles\": [{\"path\": \"missing.json\", \"kind\": \"package-json\"}]}}\n",
    ] {
        let dir = repo(manifest);
        let output = check_version(dir.path());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{manifest}: {stderr}");
        assert!(
            stderr.contains("check-release-version:"),
            "{manifest}: {stderr}"
        );
    }
}

#[test]
fn declared_release_is_still_checked() {
    let dir = repo(
        "{\"release\": {\"versionFiles\": [{\"path\": \"package.json\", \"kind\": \"package-json\"}]}}\n",
    );
    let output = check_version(dir.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
}
