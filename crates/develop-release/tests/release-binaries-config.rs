use develop_release::{create_release_tarball, load_release_binaries};
use std::path::Path;

fn config(root: &Path, text: &str) {
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(root.join("config/release-binaries.json"), text).unwrap();
}

fn public_config(root: &Path, text: &str) {
    std::fs::create_dir_all(root.join("packages/cli/config")).unwrap();
    std::fs::write(root.join("packages/cli/config/release-binaries.json"), text).unwrap();
}

#[test]
fn changed_config_changes_the_archive_exactly() {
    let dir = tempfile::tempdir().unwrap();
    public_config(dir.path(), r#"{"binaries":["one"]}"#);
    config(dir.path(), r#"{"binaries":["two"]}"#);
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for name in ["one", "two", "extra"] {
        std::fs::write(bin.join(name), name).unwrap();
    }
    let archive = dir.path().join("archive.tar.gz");
    create_release_tarball(dir.path(), &bin, &archive).unwrap();
    let listing = std::process::Command::new("tar")
        .arg("-tzf")
        .arg(&archive)
        .output()
        .unwrap();
    assert!(listing.status.success());
    assert_eq!(String::from_utf8(listing.stdout).unwrap(), "one\ntwo\n");
}

#[test]
fn missing_binary_fails_without_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    public_config(dir.path(), r#"{"binaries":["one"]}"#);
    config(dir.path(), r#"{"binaries":["two"]}"#);
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(bin.join("one"), "one").unwrap();
    let archive = dir.path().join("archive.tar.gz");
    assert!(create_release_tarball(dir.path(), &bin, &archive).is_err());
    assert!(!archive.exists());
}

#[test]
fn malformed_or_weakened_lists_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    public_config(dir.path(), r#"{"binaries":["public"]}"#);
    for text in [
        r#"{"binaries":[]}"#,
        r#"{"binaries":["one","one"]}"#,
        r#"{"binaries":["one/two"]}"#,
        r#"{"binaries":["one\\two"]}"#,
        r#"{"binaries":["../one"]}"#,
        r#"{"binaries":[""]}"#,
        r#"{"binaries":["one"],"extra":true}"#,
        r#"{"other":["one"]}"#,
    ] {
        config(dir.path(), text);
        assert!(load_release_binaries(dir.path()).is_err(), "{text}");
    }
}

#[test]
fn names_shared_by_public_and_private_lists_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    public_config(dir.path(), r#"{"binaries":["shared"]}"#);
    config(dir.path(), r#"{"binaries":["shared"]}"#);
    assert!(load_release_binaries(dir.path()).is_err());
}

/// Release 済みの main（v0.5.10）は公開側の file を持たない。その snapshot でも一覧が読める。
#[test]
fn a_v0_5_10_snapshot_without_the_public_manifest_lists_its_eight_binaries() {
    let dir = tempfile::tempdir().unwrap();
    config(
        dir.path(),
        r#"{"binaries":["develop-docs-check","check-staged-docs","check-staged-adr","develop-secrets","sync-claude-rules","sync-claude-skills","sync-claude-settings","sync-managed-lefthook"]}"#,
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_release-tarball"))
        .arg("--list")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listed: Vec<_> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        listed,
        [
            "develop-docs-check",
            "check-staged-docs",
            "check-staged-adr",
            "develop-secrets",
            "sync-claude-rules",
            "sync-claude-skills",
            "sync-claude-settings",
            "sync-managed-lefthook",
        ]
    );
}

#[test]
fn list_command_uses_the_same_combined_inventory() {
    let dir = tempfile::tempdir().unwrap();
    public_config(dir.path(), r#"{"binaries":["public"]}"#);
    config(dir.path(), r#"{"binaries":["private"]}"#);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_release-tarball"))
        .arg("--list")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "public\nprivate\n"
    );
}

#[test]
fn explicit_false_creates_a_private_only_archive_without_reading_public_manifest() {
    let dir = tempfile::tempdir().unwrap();
    public_config(dir.path(), "invalid public manifest");
    config(
        dir.path(),
        r#"{"includePublic":false,"binaries":["private"]}"#,
    );
    assert_eq!(load_release_binaries(dir.path()).unwrap(), ["private"]);
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(bin.join("private"), "private").unwrap();
    let archive = dir.path().join("archive.tar.gz");
    create_release_tarball(dir.path(), &bin, &archive).unwrap();
    let listing = std::process::Command::new("tar")
        .arg("-tzf")
        .arg(archive)
        .output()
        .unwrap();
    assert!(listing.status.success());
    assert_eq!(String::from_utf8(listing.stdout).unwrap(), "private\n");
}

#[test]
fn explicit_true_preserves_the_combined_inventory() {
    let dir = tempfile::tempdir().unwrap();
    public_config(dir.path(), r#"{"binaries":["public"]}"#);
    config(
        dir.path(),
        r#"{"includePublic":true,"binaries":["private"]}"#,
    );
    assert_eq!(
        load_release_binaries(dir.path()).unwrap(),
        ["public", "private"]
    );
}

#[test]
fn include_public_rejects_every_non_boolean_json_type() {
    let dir = tempfile::tempdir().unwrap();
    for value in ["null", "0", r#""false""#, "[]", "{}"] {
        config(
            dir.path(),
            &format!(r#"{{"includePublic":{value},"binaries":["private"]}}"#),
        );
        let error = load_release_binaries(dir.path()).unwrap_err();
        assert!(error.contains("config/release-binaries.json"), "{error}");
        assert!(error.contains("boolean"), "{error}");
    }
}

#[test]
fn private_only_still_rejects_a_missing_private_binary() {
    let dir = tempfile::tempdir().unwrap();
    config(
        dir.path(),
        r#"{"includePublic":false,"binaries":["private"]}"#,
    );
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let archive = dir.path().join("archive.tar.gz");
    assert!(
        create_release_tarball(dir.path(), &bin, &archive)
            .unwrap_err()
            .contains("private")
    );
    assert!(!archive.exists());
}

/// 期待値は snapshot の既知の一覧から固定し、loader から tar の中身を導出しない。
#[test]
fn snapshot_declarations_preserve_twelve_eight_and_four_member_archives() {
    let public = [
        "develop-docs-check",
        "check-staged-docs",
        "check-staged-adr",
        "develop-secrets",
        "release",
        "check-release-version",
        "archive-changelog",
        "release-notes",
    ];
    let private = [
        "sync-claude-rules",
        "sync-claude-skills",
        "sync-claude-settings",
        "sync-managed-lefthook",
    ];
    for mode in ["omitted", "true", "legacy", "false"] {
        let dir = tempfile::tempdir().unwrap();
        let expected: Vec<_> = match mode {
            "legacy" => public[..4].iter().chain(private.iter()).copied().collect(),
            "false" => private.to_vec(),
            _ => public.iter().chain(private.iter()).copied().collect(),
        };
        let mut manifest = serde_json::json!({"binaries": if mode == "legacy" { expected.clone() } else { private.to_vec() }});
        if mode == "true" || mode == "false" {
            manifest["includePublic"] = serde_json::json!(mode == "true");
        }
        config(dir.path(), &manifest.to_string());
        if mode != "legacy" {
            public_config(
                dir.path(),
                &serde_json::json!({"binaries":public}).to_string(),
            );
        }
        assert_eq!(
            load_release_binaries(dir.path()).unwrap(),
            expected,
            "{mode}"
        );
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_release-tarball"))
            .arg("--list")
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected.join("\n") + "\n",
            "{mode}"
        );
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for name in &expected {
            std::fs::write(bin.join(name), name).unwrap();
        }
        let archive = dir.path().join("archive.tar.gz");
        create_release_tarball(dir.path(), &bin, &archive).unwrap();
        let listing = std::process::Command::new("tar")
            .arg("-tzf")
            .arg(archive)
            .output()
            .unwrap();
        assert!(listing.status.success());
        assert_eq!(
            String::from_utf8(listing.stdout).unwrap(),
            expected.join("\n") + "\n",
            "{mode}"
        );
    }
}
