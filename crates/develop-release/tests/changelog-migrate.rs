use std::{fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let out = develop_tools_core::git(root).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "user.email", "test@example.invalid"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        git(
            root,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/fixture/demo.git",
            ],
        );
        fs::create_dir_all(root.join(".config")).unwrap();
        fs::write(
            root.join(".config/develop-tools.json"),
            br#"{"release":{"versionFiles":["VERSION"]}}"#,
        )
        .unwrap();
        fs::write(root.join("VERSION"), "0.0.0\n").unwrap();
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        fs::write(
            &gh,
            "#!/bin/sh\nprintf '%s' \"$MIGRATION_TEST_RELEASES_JSON\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { temp }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn changelog(&self, text: &str) {
        fs::write(self.root().join("CHANGELOG.md"), text).unwrap();
    }

    fn commit(&self, message: &str) -> String {
        git(
            self.root(),
            &[
                "add",
                "--",
                "CHANGELOG.md",
                "VERSION",
                ".config/develop-tools.json",
            ],
        );
        git(self.root(), &["commit", "-qm", message]);
        git(self.root(), &["rev-parse", "HEAD"])
    }

    fn run(&self, releases: &str, extra: &[&str]) -> (std::process::Output, serde_json::Value) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_changelog-migrate"));
        develop_tools_core::strip_git_env(&mut command);
        let mut paths = vec![self.root().join("bin")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let output = command
            .arg(format!("--repo={}", self.root().display()))
            .arg("--json")
            .args(extra)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("MIGRATION_TEST_RELEASES_JSON", releases)
            .output()
            .unwrap();
        let json = serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);
        (output, json)
    }

    fn check(&self, releases: &str, extra: &[&str]) -> (std::process::Output, serde_json::Value) {
        let mut args = vec!["--check"];
        args.extend_from_slice(extra);
        self.run(releases, &args)
    }
}

#[test]
fn replays_introduction_and_chooses_first_containing_tag() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 最初\n");
    let introduction = f.commit("add A");
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文だけ編集 [abc](https://github.com/fixture/demo/commit/abc)\n");
    f.commit("edit A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文だけ編集 [abc](https://github.com/fixture/demo/commit/abc)\n- **B（2026-11-02）**: 未リリース\n");
    f.commit("add B");
    let before = fs::read(f.root().join("CHANGELOG.md")).unwrap();
    let (out, json) = f.check(r#"[{"tagName":"v1.0.0","isDraft":false}]"#, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json["items"][0]["introduction"], introduction);
    assert_eq!(json["items"][0]["stamp"], "v1.0.0");
    assert_eq!(
        json["items"][0]["releaseUrl"],
        "https://github.com/fixture/demo/releases/tag/v1.0.0"
    );
    assert_eq!(json["items"][1]["stamp"], "⏳");
    assert_eq!(json["holds"], 0);
    assert_eq!(fs::read(f.root().join("CHANGELOG.md")).unwrap(), before);
}

#[test]
fn nested_bold_in_existing_title_is_a_valid_migration_item() {
    let f = Fixture::new();
    f.changelog("## 2026-06\n- **設定を **短縮** した。（2026-06-13）**: 要約\n");
    f.commit("add nested bold item");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["invalid"], 0);
    assert_eq!(plan["items"][0]["stamp"], "⏳");
}

#[test]
fn tagless_items_are_pending_and_missing_github_release_has_no_link() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 最初\n");
    f.commit("add A");
    let (out, json) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json["items"][0]["stamp"], "⏳");
    git(f.root(), &["tag", "v1.0.0"]);
    let (out, json) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json["items"][0]["stamp"], "v1.0.0");
    assert!(json["items"][0]["releaseUrl"].is_null());
}

#[test]
fn writes_only_with_matching_plan_and_preserves_other_bytes() {
    let f = Fixture::new();
    let original = "## 2026-11\n\n- **A（2026-11-01、補足）**: 本文 [abc](https://github.com/fixture/demo/commit/abc)\n\n<!-- untouched -->\n";
    f.changelog(original);
    f.commit("add A");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let hash = plan["planHash"].as_str().unwrap();
    let (out, _) = f.run("[]", &["--write", &format!("--plan-hash={hash}x")]);
    assert!(!out.status.success());
    assert_eq!(
        fs::read_to_string(f.root().join("CHANGELOG.md")).unwrap(),
        original
    );
    let (out, _) = f.run("[]", &["--write", &format!("--plan-hash={hash}")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = original.replace("（2026-11-01、補足）", "（2026-11-01/⏳、補足）");
    assert_eq!(
        fs::read_to_string(f.root().join("CHANGELOG.md")).unwrap(),
        expected
    );
    f.commit("migrate A");
    let (out, new_plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (out, _) = f.run(
        "[]",
        &[
            "--write",
            &format!("--plan-hash={}", new_plan["planHash"].as_str().unwrap()),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(f.root().join("CHANGELOG.md")).unwrap(),
        expected
    );
}

#[test]
fn rename_is_held_and_resolution_must_match_original_blob_and_key() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **旧題（2026-11-01）**: 本文\n");
    f.commit("add old");
    f.changelog("## 2026-11\n- **新題（2026-11-01）**: 本文\n");
    f.commit("rename");
    let before = fs::read(f.root().join("CHANGELOG.md")).unwrap();
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
    assert!(
        plan["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("removed")
    );
    let hash = plan["planHash"].as_str().unwrap();
    let (out, _) = f.run("[]", &["--write", &format!("--plan-hash={hash}")]);
    assert!(!out.status.success());
    assert_eq!(fs::read(f.root().join("CHANGELOG.md")).unwrap(), before);

    let resolution = f.root().join("resolution.json");
    let mut data = serde_json::json!({
        "sourceBlob": "wrong",
        "planHash": hash,
        "items": [{
            "key": plan["items"][0]["key"],
            "line": plan["items"][0]["line"],
            "stamp": "⏳",
            "reason": "旧題からの改題を確認"
        }]
    });
    fs::write(&resolution, serde_json::to_vec(&data).unwrap()).unwrap();
    let (out, _) = f.run(
        "[]",
        &[
            "--write",
            &format!("--plan-hash={hash}"),
            &format!("--resolutions={}", resolution.display()),
        ],
    );
    assert!(!out.status.success());
    assert_eq!(fs::read(f.root().join("CHANGELOG.md")).unwrap(), before);
    data["sourceBlob"] = plan["sourceBlob"].clone();
    data["items"][0]["key"] = serde_json::json!("別の題\u{1f}2026-11-01");
    fs::write(&resolution, serde_json::to_vec(&data).unwrap()).unwrap();
    let (out, _) = f.run(
        "[]",
        &[
            "--write",
            &format!("--plan-hash={hash}"),
            &format!("--resolutions={}", resolution.display()),
        ],
    );
    assert!(!out.status.success());
    assert_eq!(fs::read(f.root().join("CHANGELOG.md")).unwrap(), before);
    data["items"][0]["key"] = plan["items"][0]["key"].clone();
    fs::write(&resolution, serde_json::to_vec(&data).unwrap()).unwrap();
    let (out, _) = f.run(
        "[]",
        &[
            "--write",
            &format!("--plan-hash={hash}"),
            &format!("--resolutions={}", resolution.display()),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        fs::read_to_string(f.root().join("CHANGELOG.md"))
            .unwrap()
            .contains("新題（2026-11-01/⏳）")
    );
    f.commit("resolved migration");
    let (out, second_plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(second_plan["holds"], 0);
    let before_second = fs::read(f.root().join("CHANGELOG.md")).unwrap();
    let (out, _) = f.run(
        "[]",
        &[
            "--write",
            &format!("--plan-hash={}", second_plan["planHash"].as_str().unwrap()),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read(f.root().join("CHANGELOG.md")).unwrap(),
        before_second
    );
}

#[test]
fn old_prod_stamp_must_agree_with_history() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01、v2.0.0 で prod 反映）**: 本文\n");
    f.commit("add A");
    git(f.root(), &["tag", "v1.0.0"]);
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
    assert!(
        plan["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("conflicts")
    );
}

#[test]
fn picks_earliest_containing_tag_even_when_tag_names_sort_differently() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("add A");
    git(f.root(), &["tag", "v9.0.0"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n- **B（2026-11-02）**: 本文\n");
    f.commit("add B");
    git(f.root(), &["tag", "v1.0.0"]);
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["items"][0]["stamp"], "v9.0.0");
    assert_eq!(plan["items"][1]["stamp"], "v1.0.0");
}

#[test]
fn deletion_and_later_readdition_gets_new_introduction() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 初回\n");
    f.commit("add A");
    f.changelog("## 2026-11\n");
    f.commit("remove A");
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 再追加\n");
    let readdition = f.commit("readd A");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["items"][0]["introduction"], readdition);
}

#[test]
fn supplementary_version_conflict_is_held() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01、v9.9.9 対応）**: 本文\n");
    f.commit("add A");
    git(f.root(), &["tag", "v1.0.0"]);
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn malformed_current_item_is_not_silently_skipped() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（bad-date）**: 本文\n");
    f.commit("add malformed A");
    let (out, plan) = f.check("[]", &[]);
    assert!(!out.status.success());
    assert_eq!(plan["invalid"], 1);
    assert_eq!(plan["invalidLines"], serde_json::json!([2]));
    let mut paths = vec![f.root().join("bin")];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let plain = Command::new(env!("CARGO_BIN_EXE_changelog-migrate"))
        .arg(format!("--repo={}", f.root().display()))
        .arg("--check")
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("MIGRATION_TEST_RELEASES_JSON", "[]")
        .output()
        .unwrap();
    assert!(!plain.status.success());
    assert!(String::from_utf8_lossy(&plain.stdout).contains("1 invalid"));
    let (out, _) = f.run(
        "[]",
        &[
            "--write",
            &format!("--plan-hash={}", plan["planHash"].as_str().unwrap()),
        ],
    );
    assert!(!out.status.success());
}

#[test]
fn merge_does_not_reintroduce_parent_item() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    let introduction = f.commit("add A");
    git(f.root(), &["branch", "side"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n- **B（2026-11-02）**: main\n");
    f.commit("main B");
    git(f.root(), &["checkout", "-q", "side"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n- **C（2026-11-03）**: side\n");
    f.commit("side C");
    git(f.root(), &["checkout", "-q", "main"]);
    git(
        f.root(),
        &[
            "merge",
            "-q",
            "--no-ff",
            "-s",
            "ours",
            "side",
            "-m",
            "merge side",
        ],
    );
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["items"][0]["introduction"], introduction);
}

#[test]
fn incomparable_first_tags_hold_the_item() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("add A");
    git(f.root(), &["branch", "side"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n- **B（2026-11-02）**: main\n");
    f.commit("main B");
    git(f.root(), &["tag", "v1.0.0"]);
    git(f.root(), &["checkout", "-q", "side"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n- **C（2026-11-03）**: side\n");
    f.commit("side C");
    git(f.root(), &["tag", "v2.0.0"]);
    git(f.root(), &["checkout", "-q", "main"]);
    git(
        f.root(),
        &[
            "merge",
            "-q",
            "--no-ff",
            "-s",
            "ours",
            "side",
            "-m",
            "merge side",
        ],
    );
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
    assert!(
        plan["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("incomparable")
    );
}

#[test]
fn release_lookup_failure_fails_closed() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("add A");
    let (out, _) = f.check("not-json", &[]);
    assert!(!out.status.success());
}

#[test]
fn write_rejects_a_changed_source_blob() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("add A");
    let (_, plan) = f.check("[]", &[]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 別の本文\n");
    let (out, _) = f.run(
        "[]",
        &[
            "--write",
            &format!("--plan-hash={}", plan["planHash"].as_str().unwrap()),
        ],
    );
    assert!(!out.status.success());
    assert_eq!(
        fs::read_to_string(f.root().join("CHANGELOG.md")).unwrap(),
        "## 2026-11\n- **A（2026-11-01）**: 別の本文\n"
    );
}

#[test]
fn unrelated_removal_in_another_hunk_does_not_hold_a_new_item() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **旧題（2026-11-01）**: 消す\n\n## 2026-10\n- **維持（2026-10-01）**: 維持\n");
    f.commit("base");
    f.changelog("## 2026-11\n\n## 2026-10\n- **維持（2026-10-01）**: 維持\n- **新題（2026-10-02）**: 追加\n");
    f.commit("remove and add separately");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 0);
}

#[test]
fn date_change_in_same_hunk_is_held() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("add A");
    f.changelog("## 2026-11\n- **A（2026-11-02）**: 本文\n");
    f.commit("correct date");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn release_set_change_invalidates_the_plan_hash() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("add A");
    git(f.root(), &["tag", "v1.0.0"]);
    let (_, old_plan) = f.check("[]", &[]);
    let old_hash = old_plan["planHash"].as_str().unwrap();
    let (out, _) = f.run(
        r#"[{"tagName":"v1.0.0","isDraft":false}]"#,
        &["--write", &format!("--plan-hash={old_hash}")],
    );
    assert!(!out.status.success());
    assert_eq!(
        fs::read_to_string(f.root().join("CHANGELOG.md")).unwrap(),
        "## 2026-11\n- **A（2026-11-01）**: 本文\n"
    );
}

#[test]
fn shallow_history_stops_before_a_plan() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("add A");
    let clone = f.root().join("shallow");
    let out = Command::new("git")
        .args([
            "clone",
            "--depth=1",
            &format!("file://{}", f.root().display()),
        ])
        .arg(&clone)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = Command::new(env!("CARGO_BIN_EXE_changelog-migrate"))
        .arg(format!("--repo={}", clone.display()))
        .args(["--check", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("shallow"));
}

#[test]
fn version_section_repos_are_outside_this_migration() {
    let f = Fixture::new();
    fs::write(
        f.root().join(".config/develop-tools.json"),
        br#"{"release":{"versionFiles":["VERSION"],"notes":{"fallback":"changelog-section"}}}"#,
    )
    .unwrap();
    f.changelog("## v1.0.0\n- old entry\n");
    f.commit("section changelog");
    let (out, _) = f.check("[]", &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("outside migration scope"));
}

#[test]
fn old_pending_and_matching_prod_forms_are_fully_replaced() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01、v1.0.0 で prod 反映）**: 公開済み\n");
    f.commit("add A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog("## 2026-11\n- **A（2026-11-01、v1.0.0 で prod 反映）**: 公開済み\n- **B（2026-11-02、未リリース）**: 待機\n");
    f.commit("add B");
    let (out, plan) = f.check(r#"[{"tagName":"v1.0.0","isDraft":false}]"#, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 0);
    let (out, _) = f.run(
        r#"[{"tagName":"v1.0.0","isDraft":false}]"#,
        &[
            "--write",
            &format!("--plan-hash={}", plan["planHash"].as_str().unwrap()),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let changed = fs::read_to_string(f.root().join("CHANGELOG.md")).unwrap();
    assert!(
        changed.contains(
            "A（2026-11-01/[v1.0.0](https://github.com/fixture/demo/releases/tag/v1.0.0)）"
        )
    );
    assert!(changed.contains("B（2026-11-02/⏳）"));
    assert!(!changed.contains("未リリース"));
    assert!(!changed.contains("prod 反映"));
}

#[test]
fn changelog_rename_does_not_claim_the_rename_as_introduction() {
    let f = Fixture::new();
    fs::write(
        f.root().join("OLD.md"),
        "## 2026-11\n- **A（2026-11-01）**: 本文\n",
    )
    .unwrap();
    git(
        f.root(),
        &[
            "add",
            "--",
            "OLD.md",
            "VERSION",
            ".config/develop-tools.json",
        ],
    );
    git(f.root(), &["commit", "-qm", "old changelog"]);
    git(f.root(), &["mv", "OLD.md", "CHANGELOG.md"]);
    git(f.root(), &["commit", "-qm", "rename changelog"]);
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
    assert!(
        plan["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("rename")
    );
}

#[test]
fn moved_tag_invalidates_a_previous_plan() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("add A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n- **B（2026-11-02）**: 後続\n");
    f.commit("add B");
    let (_, plan) = f.check("[]", &[]);
    let old_hash = plan["planHash"].as_str().unwrap();
    git(f.root(), &["tag", "-f", "v1.0.0"]);
    let (out, _) = f.run("[]", &["--write", &format!("--plan-hash={old_hash}")]);
    assert!(!out.status.success());
    assert!(
        !fs::read_to_string(f.root().join("CHANGELOG.md"))
            .unwrap()
            .contains("/⏳")
    );
}

#[test]
fn split_of_an_old_item_holds_both_new_keys() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **元題（2026-11-01）**: 本文\n");
    f.commit("original");
    f.changelog("## 2026-11\n- **一（2026-11-01）**: 前半\n- **二（2026-11-01）**: 後半\n");
    f.commit("split");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 2);
}

#[test]
fn old_undated_history_does_not_become_a_false_introduction() {
    for old in [
        "- **A**: 古い本文",
        "- **A** 古い本文",
        "- **A**：古い本文",
        "- **A** — 古い本文",
        "- **A** 本文 **B**: x",
    ] {
        let f = Fixture::new();
        f.changelog(&format!("## v0.1.0\n{old}\n"));
        f.commit("old undated entry");
        f.changelog("## 2026-11\n- **A（2026-11-01）**: 古い本文\n");
        f.commit("convert to dated entry");
        let (out, plan) = f.check("[]", &[]);
        assert!(
            out.status.success(),
            "{old}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(plan["holds"], 1, "{old}");
        assert!(
            plan["items"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("unparsed"),
            "{old}"
        );
    }
}

#[test]
fn pending_legacy_stamp_with_containing_tag_requires_resolution() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01、未リリース）**: 本文\n");
    f.commit("add pending A");
    git(f.root(), &["tag", "v1.0.0"]);
    let (out, plan) = f.check(r#"[{"tagName":"v1.0.0","isDraft":false}]"#, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
    let (out, _) = f.run(
        r#"[{"tagName":"v1.0.0","isDraft":false}]"#,
        &[
            "--write",
            &format!("--plan-hash={}", plan["planHash"].as_str().unwrap()),
        ],
    );
    assert!(!out.status.success());
}

#[test]
fn equal_tree_merge_preserves_both_possible_introductions_as_a_hold() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n");
    f.commit("base");
    git(f.root(), &["branch", "side"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("main A");
    git(f.root(), &["tag", "v1.0.0"]);
    git(f.root(), &["checkout", "-q", "side"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("side A");
    git(f.root(), &["tag", "v2.0.0"]);
    git(f.root(), &["checkout", "-q", "main"]);
    git(
        f.root(),
        &[
            "merge",
            "-q",
            "--no-ff",
            "-s",
            "ours",
            "side",
            "-m",
            "equal tree merge",
        ],
    );
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
    assert!(
        plan["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("multiple introductions")
    );
}

#[test]
fn old_stamp_with_supplement_does_not_leave_old_words_behind() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01、v1.0.0 で prod 反映、補足）**: 公開\n");
    f.commit("add A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog("## 2026-11\n- **A（2026-11-01、v1.0.0 で prod 反映、補足）**: 公開\n- **B（2026-11-02、補足、未リリース）**: 待機\n");
    f.commit("add B");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 0);
    let (out, _) = f.run(
        "[]",
        &[
            "--write",
            &format!("--plan-hash={}", plan["planHash"].as_str().unwrap()),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let changed = fs::read_to_string(f.root().join("CHANGELOG.md")).unwrap();
    assert!(changed.contains("A（2026-11-01/v1.0.0、補足）"));
    assert!(changed.contains("B（2026-11-02/⏳、補足）"));
    assert!(!changed.contains("未リリース"));
    assert!(!changed.contains("prod 反映"));
}

#[test]
fn draft_release_does_not_produce_a_link() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("add A");
    git(f.root(), &["tag", "v1.0.0"]);
    let (out, plan) = f.check(r#"[{"tagName":"v1.0.0","isDraft":true}]"#, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["items"][0]["stamp"], "v1.0.0");
    assert!(plan["items"][0]["releaseUrl"].is_null());
}

#[test]
fn help_explains_default_and_write_boundary() {
    let out = Command::new(env!("CARGO_BIN_EXE_changelog-migrate"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(help.contains("Usage:"));
    assert!(help.contains("Default:"));
    assert!(help.contains("Writes:"));
}

#[test]
fn dated_and_undated_branch_merge_holds_the_dated_key() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n");
    f.commit("base");
    git(f.root(), &["branch", "side"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("main dated A");
    git(f.root(), &["checkout", "-q", "side"]);
    f.changelog("## 2026-11\n- **A**: 古い本文\n");
    f.commit("side undated A");
    git(f.root(), &["checkout", "-q", "main"]);
    git(
        f.root(),
        &[
            "merge",
            "-q",
            "--no-ff",
            "-s",
            "ours",
            "side",
            "-m",
            "merge branches",
        ],
    );
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn correcting_malformed_historical_date_is_not_a_new_introduction() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026/11/01）**: 本文\n");
    f.commit("bad date A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("fix date A");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
    assert!(
        plan["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("historical")
    );
}

#[test]
fn all_supplement_versions_must_match_the_selected_tag() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01、v1.0.0 と v2.0.0 を比較）**: 本文\n");
    f.commit("add A");
    git(f.root(), &["tag", "v1.0.0"]);
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn supplement_version_is_checked_alongside_legacy_prod_stamp() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01、v1.0.0 で prod 反映、v2.0.0 の補足）**: 本文\n");
    f.commit("add A");
    git(f.root(), &["tag", "v1.0.0"]);
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn parenthesized_undated_title_moved_to_another_hunk_is_held() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **ログ（監査）を追加**: 本文\n- **B（2026-11-02）**: 後続\n");
    f.commit("undated A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog(
        "## 2026-11\n- **B（2026-11-02）**: 後続\n- **ログ（監査）を追加（2026-11-01）**: 本文\n",
    );
    f.commit("date and move A");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn unknown_removed_in_same_hunk_holds_even_without_title_match() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A 2026/11/01**: 本文\n");
    f.commit("malformed ASCII date");
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("fix date and punctuation");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn nested_parentheses_in_old_metadata_and_movement_are_held() {
    let f = Fixture::new();
    f.changelog(
        "## 2026-11\n- **A（2026/11/01、PR (#1) で反映）**: 本文\n- **B（2026-11-02）**: 後続\n",
    );
    f.commit("old malformed A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog(
        "## 2026-11\n- **B（2026-11-02）**: 後続\n- **A（2026-11-01、PR (#1) で反映）**: 本文\n",
    );
    f.commit("date and move A");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn unknown_date_without_brackets_moved_between_hunks_is_held() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A 2026/11/01**: 本文\n- **B（2026-11-02）**: 後続\n");
    f.commit("old malformed A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog("## 2026-11\n- **B（2026-11-02）**: 後続\n- **A（2026-11-01）**: 本文\n");
    f.commit("date and move A");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn legacy_compact_history_uses_its_original_introduction() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A** 2026-11-01/v1.0.0: 本文\n");
    let introduction = f.commit("old compact A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog("## 2026-11\n- **A（2026-11-01）**: 本文\n");
    f.commit("normalize A");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["items"][0]["introduction"], introduction);
    assert_eq!(plan["items"][0]["stamp"], "v1.0.0");
    assert_eq!(plan["holds"], 0);
}

#[test]
fn stamped_input_must_match_core_parser_and_origin() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A（2026-11-01/[v1.0.0](https://github.com/fixture/demo/releases/tag/v1.0.1)）**: 本文\n");
    f.commit("invalid stamped A");
    let (out, plan) = f.check("[]", &[]);
    assert!(!out.status.success());
    assert_eq!(plan["invalid"], 1);

    f.changelog("## 2026-11\n- **A（2026-11-01/[v1.0.0](https://github.com/other/repo/releases/tag/v1.0.0)）**: 本文\n");
    f.commit("wrong owner A");
    let (out, _) = f.check("[]", &[]);
    assert!(!out.status.success());
}

#[test]
fn compact_rename_before_bracket_conversion_is_held() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A** 2026-11-01/v1.0.0: 本文\n");
    f.commit("compact A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog("## 2026-11\n- **B** 2026-11-01/v1.0.0: 本文\n");
    f.commit("rename compact A to B");
    f.changelog("## 2026-11\n- **B（2026-11-01）**: 本文\n");
    f.commit("convert B to bracket");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}

#[test]
fn compact_date_correction_before_bracket_conversion_is_held() {
    let f = Fixture::new();
    f.changelog("## 2026-11\n- **A** 2026-11-01/v1.0.0: 本文\n");
    f.commit("compact A");
    git(f.root(), &["tag", "v1.0.0"]);
    f.changelog("## 2026-11\n- **A** 2026-11-02/v1.0.0: 本文\n");
    f.commit("correct compact date");
    f.changelog("## 2026-11\n- **A（2026-11-02）**: 本文\n");
    f.commit("convert compact A");
    let (out, plan) = f.check("[]", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(plan["holds"], 1);
}
