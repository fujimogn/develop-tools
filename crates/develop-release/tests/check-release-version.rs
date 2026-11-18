use std::path::Path;
use std::process::{Command, Output};

fn git(root: &Path, args: &[&str]) -> String {
    let out = develop_tools_core::git(root).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// `manifest` を宣言に持つ 1 commit の repo を作る。
fn repo(manifest: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("package.json"), "{\"version\":\"1.2.3\"}\n").unwrap();
    std::fs::create_dir_all(root.join(".config")).unwrap();
    std::fs::write(root.join(".config/develop-tools.json"), manifest).unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgSign", "false"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "initial"]);
    dir
}

fn check_version(root: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_check-release-version"));
    develop_tools_core::strip_git_env(&mut command);
    command.current_dir(root).output().unwrap()
}

fn commit(root: &Path, message: &str) -> String {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", message]);
    git(root, &["rev-parse", "HEAD"])
}

fn workspace(root: &Path, prefix: Option<&str>, manifest: &str, version: &str) {
    let mut config = serde_json::json!({"release":{"versionFiles":[
        {"path":"package.json","kind":"package-json"},
        {"path":manifest,"kind":"cargo-workspace"}
    ]}});
    if let Some(prefix) = prefix {
        config["release"]["tagPrefix"] = serde_json::json!(prefix);
    }
    std::fs::write(root.join(".config/develop-tools.json"), config.to_string()).unwrap();
    std::fs::write(
        root.join("package.json"),
        serde_json::json!({"version":version}).to_string(),
    )
    .unwrap();
    let path = root.join(manifest);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!("[workspace]\nmembers=[]\n[workspace.package]\nversion=\"{version}\"\n"),
    )
    .unwrap();
}

fn check_pushed(root: &Path, input: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut command = Command::new(env!("CARGO_BIN_EXE_check-release-version"));
    develop_tools_core::strip_git_env(&mut command);
    let mut child = command
        .arg("--pre-push")
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn pushed(tag: &str, object: &str) -> String {
    format!("HEAD {object} refs/tags/{tag} {}\n", "0".repeat(40))
}

#[test]
fn pushed_remote_tag_uses_its_commit_rule_instead_of_head() {
    let dir =
        repo(r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#);
    let root = dir.path();
    workspace(root, Some("release-"), "old/Cargo.toml", "1.2.3");
    let old = commit(root, "old release rule");
    workspace(root, Some("head-"), "new/Cargo.toml", "2.0.0");
    commit(root, "different head");
    let wrong = check_pushed(root, &pushed("release-9.9.9", &old));
    assert!(
        !wrong.status.success(),
        "remote alias must be checked at its object"
    );
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("release-9.9.9"));
    let correct = check_pushed(root, &pushed("release-1.2.3", &old));
    assert!(
        correct.status.success(),
        "{}",
        String::from_utf8_lossy(&correct.stderr)
    );
    assert!(String::from_utf8_lossy(&correct.stdout).contains("push release-1.2.3"));
}

#[test]
fn non_release_tag_does_not_validate_unrelated_historical_fields() {
    let dir =
        repo(r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#);
    let root = dir.path();
    std::fs::write(root.join(".config/develop-tools.json"), r#"{"adrMigratedThrough":"old","ci":"old","release":{"tagPrefix":"release-","versionFiles":[{"path":"missing.toml","kind":"cargo-workspace"}]}}"#).unwrap();
    let old = commit(root, "old unrelated schema");
    workspace(root, Some("head-"), "new/Cargo.toml", "2.0.0");
    commit(root, "valid head");
    let ignored = check_pushed(root, &pushed("archive", &old));
    assert!(
        ignored.status.success(),
        "{}",
        String::from_utf8_lossy(&ignored.stderr)
    );
    let checked = check_pushed(root, &pushed("release-1.2.3", &old));
    assert!(
        !checked.status.success(),
        "release tag must use the strict reader"
    );
    assert!(String::from_utf8_lossy(&checked.stderr).contains("adrMigratedThrough"));
}

#[test]
fn pre_declaration_root_cargo_tag_keeps_its_legacy_rule() {
    let dir =
        repo(r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#);
    let root = dir.path();
    std::fs::remove_file(root.join(".config/develop-tools.json")).unwrap();
    std::fs::remove_file(root.join("package.json")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace.package]\nversion=\"1.2.3\"\n",
    )
    .unwrap();
    let old = commit(root, "before declaration");
    workspace(root, Some("head-"), "new/Cargo.toml", "2.0.0");
    commit(root, "valid current declaration");
    let output = check_pushed(root, &pushed("v1.2.3", &old));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("push v1.2.3"));
    assert!(!check_pushed(root, &pushed("v9.9.9", &old)).status.success());
}

#[test]
fn declared_historical_tags_keep_missing_prefix_and_legacy_location_compatibility() {
    for legacy_location in [false, true] {
        let dir =
            repo(r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#);
        let root = dir.path();
        workspace(root, None, "old/Cargo.toml", "1.2.3");
        if legacy_location {
            std::fs::rename(
                root.join(".config/develop-tools.json"),
                root.join(".develop-tools.json"),
            )
            .unwrap();
        }
        let old = commit(root, "historical declaration");
        if legacy_location {
            std::fs::remove_file(root.join(".develop-tools.json")).unwrap();
        }
        workspace(root, Some("head-"), "new/Cargo.toml", "2.0.0");
        commit(root, "current declaration");
        let output = check_pushed(root, &pushed("v1.2.3", &old));
        assert!(
            output.status.success(),
            "legacy_location={legacy_location}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("push v1.2.3"));
    }
}

#[test]
fn non_commit_tags_preserve_non_release_names_and_reject_version_names() {
    for prefix in ["v", "release-"] {
        let dir =
            repo(r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#);
        let root = dir.path();
        workspace(root, Some(prefix), "Cargo.toml", "1.2.3");
        commit(root, "current rule");
        let blob = git(root, &["hash-object", "-w", "package.json"]);
        let output = check_pushed(root, &pushed("assets", &blob));
        assert!(
            output.status.success(),
            "{prefix}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !check_pushed(root, &pushed(&format!("{prefix}9.9.9"), &blob))
                .status
                .success()
        );
        assert!(
            !check_pushed(root, &pushed("v1.2.3", &blob))
                .status
                .success()
        );
        assert!(
            !check_pushed(root, &pushed("assets", &"f".repeat(40)))
                .status
                .success()
        );
    }
}

#[test]
fn multiple_sent_objects_keep_their_own_rules_including_annotated_tags() {
    let dir =
        repo(r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#);
    let root = dir.path();
    git(root, &["config", "tag.gpgSign", "false"]);
    workspace(root, Some("release-"), "one/Cargo.toml", "1.2.3");
    let first = commit(root, "first rule");
    workspace(root, Some("other-"), "two/Cargo.toml", "2.0.0");
    let second = commit(root, "second rule");
    git(
        root,
        &["tag", "-a", "object-only", "-m", "annotation", &second],
    );
    let annotated = git(root, &["rev-parse", "refs/tags/object-only"]);
    workspace(root, Some("head-"), "three/Cargo.toml", "3.0.0");
    commit(root, "third rule");
    let input = pushed("release-1.2.3", &first) + &pushed("other-2.0.0", &annotated);
    let output = check_pushed(root, &input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("push release-1.2.3") && stdout.contains("push other-2.0.0"),
        "{stdout}"
    );
    assert!(
        !check_pushed(
            root,
            &(pushed("release-1.2.3", &first) + &pushed("other-9.9.9", &annotated))
        )
        .status
        .success()
    );
}

#[test]
fn malformed_historical_declarations_are_never_legacy_fallbacks() {
    for declaration in [
        r#"{"release":{"tagPrefix":null}}"#,
        r#"{"release":{"tagPrefix":"release1"}}"#,
        r#"{"release":{"tagPrefix":""}}"#,
        r#"{"release":null}"#,
        "{",
    ] {
        let dir =
            repo(r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#);
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace.package]\nversion=\"1.2.3\"\n",
        )
        .unwrap();
        std::fs::write(root.join(".config/develop-tools.json"), declaration).unwrap();
        let old = commit(root, "invalid declaration");
        workspace(root, Some("head-"), "new/Cargo.toml", "2.0.0");
        commit(root, "current declaration");
        for tag in ["v1.2.3", "archive"] {
            let output = check_pushed(root, &pushed(tag, &old));
            assert!(!output.status.success(), "{declaration}: {tag}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("develop-tools.json"));
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(&old),
                "送信 commit と設定 path を診断に残す: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn ordinary_guard_does_not_read_stdin_and_checks_the_declared_head_tag() {
    let dir = repo(
        r#"{"release":{"tagPrefix":"release-","versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#,
    );
    let root = dir.path();
    git(root, &["tag", "release-1.2.3"]);
    std::fs::write(root.join("stdin-bytes"), [0xff]).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_check-release-version"));
    develop_tools_core::strip_git_env(&mut command);
    let output = command
        .current_dir(root)
        .stdin(std::fs::File::open(root.join("stdin-bytes")).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("OK (release-1.2.3)"));
    git(root, &["tag", "release-9.9.9"]);
    assert!(!check_version(root).status.success());
}

#[test]
fn declared_missing_version_files_are_not_pre_declaration_commits() {
    for declaration in [
        r#"{"release":{"versionFiles":[{"path":"missing.toml","kind":"cargo-workspace"}]}}"#,
        r#"{"release":{}}"#,
    ] {
        let dir = repo(declaration);
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace.package]\nversion=\"1.2.3\"\n",
        )
        .unwrap();
        let old = commit(root, "declared but incomplete");
        workspace(root, Some("head-"), "new/Cargo.toml", "2.0.0");
        commit(root, "valid head");
        assert!(!check_pushed(root, &pushed("v1.2.3", &old)).status.success());
        assert!(
            check_pushed(root, &pushed("archive", &old))
                .status
                .success()
        );
    }
}

#[test]
fn deleted_and_non_tag_rows_are_ignored_before_object_resolution() {
    let dir =
        repo(r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#);
    let zero = "0".repeat(40);
    let input = format!(
        "(delete) {zero} refs/tags/v9.9.9 {zero}\nHEAD nonexistent refs/heads/topic {zero}\nmalformed\ntwo fields\n"
    );
    assert!(check_pushed(dir.path(), &input).status.success());
}

#[test]
fn duplicate_historical_locations_are_rejected_before_classification() {
    let dir =
        repo(r#"{"release":{"versionFiles":[{"path":"package.json","kind":"package-json"}]}}"#);
    let root = dir.path();
    std::fs::copy(
        root.join(".config/develop-tools.json"),
        root.join(".develop-tools.json"),
    )
    .unwrap();
    let old = commit(root, "duplicate declarations");
    std::fs::remove_file(root.join(".develop-tools.json")).unwrap();
    workspace(root, Some("head-"), "new/Cargo.toml", "2.0.0");
    commit(root, "valid head");
    for tag in ["v1.2.3", "archive"] {
        let output = check_pushed(root, &pushed(tag, &old));
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("manifest_duplicated"));
    }
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
