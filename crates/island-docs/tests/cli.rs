//! `develop-docs-check` の入口を、一時 repo の git index に対して実際に起動して確かめる。

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_develop-docs-check");

fn git(dir: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    develop_core::strip_git_env(&mut command);
    let out = command
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo(origin: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    if let Some(url) = origin {
        git(dir.path(), &["remote", "add", "origin", url]);
    }
    // crate の中の fixture（example/* の repo だけの表）を写しとして置く。
    let scopes =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/scope-fixture.json"))
            .expect("scope fixture");
    write(dir.path(), "config/island-docs-scopes.json", &scopes);
    dir
}

/// 島の検査項目をすべて満たす repo（必須 file、索引、TODO / CHANGELOG の形）。
fn island_repo(origin: Option<&str>) -> tempfile::TempDir {
    let dir = repo(origin);
    for (path, text) in [
        ("README.md", "# 入口\n"),
        ("AGENTS.md", "# agents\n"),
        ("CLAUDE.md", "# claude\n"),
        ("CONTEXT.md", "# context\n"),
        ("docs/README.md", "# docs\n\n[ADR](adr/README.md)\n"),
        ("docs/adr/README.md", "# ADR\n"),
        (
            "TODO.md",
            "# TODO\n\n## 進行中\n\n- [ ] **項目**。現在地。\n",
        ),
        (
            "CHANGELOG.md",
            "# Changelog\n\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n\n- **成果（2026-09-30）**: 要約。\n",
        ),
    ] {
        write(dir.path(), path, text);
    }
    dir
}

fn site_adrs(dir: &Path) {
    write(
        dir,
        "docs/adr/README.md",
        "# ADR\n- [0016](0016-site-production-adr-tdd-workflow.md)\n- [0017](0017-animation-stack.md)\n",
    );
    for (path, number) in [
        ("docs/adr/0016-site-production-adr-tdd-workflow.md", "0016"),
        ("docs/adr/0017-animation-stack.md", "0017"),
    ] {
        write(
            dir,
            path,
            &format!(
                "---\nstatus: accepted\ndate: 2026-09-30\n---\n\n# ADR-{number}: Test\n\nChosen option: A\n\n- Good: A\n- Bad: B\n- Neutral: C\n"
            ),
        );
    }
}

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

fn run(dir: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(BIN);
    develop_core::strip_git_env(&mut command);
    command.args(args).current_dir(dir).output().expect("run")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn head(dir: &Path) -> String {
    let out = develop_core::git(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("rev-parse");
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

#[test]
fn bun_references_follow_tracked_staged_and_range_snapshots() {
    let dir = repo(None);
    write(
        dir.path(),
        "package.json",
        r#"{"scripts":{"valid":"true"}}"#,
    );
    write(dir.path(), "scripts/valid.ts", "// valid\n");
    write(
        dir.path(),
        "README.md",
        "`bun run valid` and `bun scripts/valid.ts`\n",
    );
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = head(dir.path());

    write(
        dir.path(),
        "README.md",
        "`bun run missing` and `sh scripts/gone.sh`\n",
    );
    git(dir.path(), &["add", "README.md"]);
    let staged = run(dir.path(), &["--staged"]);
    assert_eq!(staged.status.code(), Some(1), "{}", stderr(&staged));
    assert!(stderr(&staged).contains("missing"), "{}", stderr(&staged));
    assert!(
        stderr(&staged).contains("scripts/gone.sh"),
        "{}",
        stderr(&staged)
    );
    let tracked = run(dir.path(), &["--tracked"]);
    assert_eq!(tracked.status.code(), Some(1), "{}", stderr(&tracked));

    git(dir.path(), &["commit", "-qm", "broken"]);
    let current = head(dir.path());
    let range = run(dir.path(), &["--range", &base, &current]);
    assert_eq!(range.status.code(), Some(1), "{}", stderr(&range));
    assert!(
        stderr(&range).contains("scripts/gone.sh"),
        "{}",
        stderr(&range)
    );
}

#[test]
fn changed_package_scripts_recheck_unchanged_markdown() {
    let dir = repo(None);
    write(
        dir.path(),
        "package.json",
        r#"{"scripts":{"valid":"true"}}"#,
    );
    write(dir.path(), "README.md", "`bun run valid`\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = head(dir.path());
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    git(dir.path(), &["add", "package.json"]);
    let staged = run(dir.path(), &["--staged"]);
    assert_eq!(staged.status.code(), Some(1), "{}", stderr(&staged));
    assert!(stderr(&staged).contains("valid"), "{}", stderr(&staged));
    git(dir.path(), &["commit", "-qm", "remove script"]);
    let range = run(dir.path(), &["--range", &base, &head(dir.path())]);
    assert_eq!(range.status.code(), Some(1), "{}", stderr(&range));
}

#[test]
fn bun_references_use_workspace_and_historical_exceptions() {
    let dir = repo(None);
    write(dir.path(), "package.json", r#"{"scripts":{"root":"true"}}"#);
    write(
        dir.path(),
        "apps/web/package.json",
        r#"{"name":"web","scripts":{"build":"true"}}"#,
    );
    write(dir.path(), "scripts/real.ts", "// valid\n");
    write(
        dir.path(),
        "README.md",
        "`bun run --filter web build`\n`bun run --cwd apps/web build`\n`cd apps/web && bun run build`\n```sh\nbun run absent\n```\n`bun run <name>`\n`bun run scripts/example.ts`\n`bash scripts/missing.sh`\n",
    );
    write(dir.path(), "CHANGELOG.md", "`bun run old`\n");
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("absent"), "{err}");
    assert!(err.contains("scripts/missing.sh"), "{err}");
    assert!(!err.contains("-> old"), "{err}");
    assert!(!err.contains("apps/web:build"), "{err}");
    assert!(!err.contains("<name>"), "{err}");
}

#[test]
fn rust_only_repo_skips_bun_references() {
    let dir = island_repo(None);
    write(
        dir.path(),
        "README.md",
        "`bun run absent` and `sh scripts/missing.sh`\n",
    );
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn changed_workspace_package_and_script_path_recheck_referrers() {
    let dir = repo(None);
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    write(
        dir.path(),
        "apps/web/package.json",
        r#"{"name":"web","scripts":{"build":"true"}}"#,
    );
    write(dir.path(), "scripts/build.sh", "echo build\n");
    write(
        dir.path(),
        "README.md",
        "`bun run --filter web build`\n`sh scripts/build.sh`\n",
    );
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = head(dir.path());

    write(
        dir.path(),
        "apps/web/package.json",
        r#"{"name":"web","scripts":{}}"#,
    );
    git(dir.path(), &["rm", "scripts/build.sh"]);
    git(dir.path(), &["add", "apps/web/package.json"]);
    let staged = run(dir.path(), &["--staged"]);
    assert_eq!(staged.status.code(), Some(1), "{}", stderr(&staged));
    assert!(
        stderr(&staged).contains("apps/web:build"),
        "{}",
        stderr(&staged)
    );
    assert!(
        stderr(&staged).contains("scripts/build.sh"),
        "{}",
        stderr(&staged)
    );

    git(dir.path(), &["commit", "-qm", "remove targets"]);
    let range = run(dir.path(), &["--range", &base, &head(dir.path())]);
    assert_eq!(range.status.code(), Some(1), "{}", stderr(&range));
    assert!(
        stderr(&range).contains("apps/web:build"),
        "{}",
        stderr(&range)
    );
    assert!(
        stderr(&range).contains("scripts/build.sh"),
        "{}",
        stderr(&range)
    );
}

#[test]
fn workspace_readme_uses_its_own_package_and_scripts_directory() {
    let dir = island_repo(None);
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    write(
        dir.path(),
        "apps/web/package.json",
        r#"{"name":"web","scripts":{"build":"true"}}"#,
    );
    write(dir.path(), "apps/web/scripts/local.sh", "echo local\n");
    write(
        dir.path(),
        "apps/web/README.md",
        "`bun run build`\n`sh scripts/local.sh`\n",
    );
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn root_script_removed_from_workspace_readme_is_reported_in_diff_modes() {
    let dir = repo(None);
    write(dir.path(), "package.json", r#"{"scripts":{"lint":"true"}}"#);
    write(
        dir.path(),
        "apps/web/package.json",
        r#"{"name":"web","scripts":{}}"#,
    );
    write(dir.path(), "apps/web/README.md", "`bun run lint`\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = head(dir.path());
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    git(dir.path(), &["add", "package.json"]);
    let staged = run(dir.path(), &["--staged"]);
    assert_eq!(staged.status.code(), Some(1), "{}", stderr(&staged));
    assert!(
        stderr(&staged).contains("apps/web:lint"),
        "{}",
        stderr(&staged)
    );
    git(dir.path(), &["commit", "-qm", "remove lint"]);
    let range = run(dir.path(), &["--range", &base, &head(dir.path())]);
    assert_eq!(range.status.code(), Some(1), "{}", stderr(&range));
    assert!(
        stderr(&range).contains("apps/web:lint"),
        "{}",
        stderr(&range)
    );
}

#[test]
fn workspace_name_change_rechecks_filter_references() {
    let dir = repo(None);
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    write(
        dir.path(),
        "apps/web/package.json",
        r#"{"name":"web","scripts":{"build":"true"}}"#,
    );
    write(dir.path(), "README.md", "`bun run --filter web build`\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = head(dir.path());
    write(
        dir.path(),
        "apps/web/package.json",
        r#"{"name":"site","scripts":{"build":"true"}}"#,
    );
    git(dir.path(), &["add", "apps/web/package.json"]);
    let staged = run(dir.path(), &["--staged"]);
    assert_eq!(staged.status.code(), Some(1), "{}", stderr(&staged));
    assert!(stderr(&staged).contains("web:build"), "{}", stderr(&staged));
    git(dir.path(), &["commit", "-qm", "rename workspace"]);
    let range = run(dir.path(), &["--range", &base, &head(dir.path())]);
    assert_eq!(range.status.code(), Some(1), "{}", stderr(&range));
}

#[test]
fn historical_and_external_examples_do_not_hide_local_missing_scripts() {
    let dir = island_repo(Some("https://github.com/example/current.git"));
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    write(dir.path(), "docs/reviews/old.md", "# old\n`bun run old`\n");
    write(
        dir.path(),
        "README.md",
        "他 repo（sample）の `bun run external`\n`bun run device:rtx|swx` は選択肢\n`bun run local_missing`\n",
    );
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("local_missing"), "{err}");
    assert!(!err.contains("external"), "{err}");
    assert!(!err.contains("device:rtx|swx"), "{err}");
    assert!(!err.contains("old"), "{err}");
}

#[test]
fn ci_and_lefthook_contexts_do_not_hide_missing_bun_scripts() {
    let dir = island_repo(Some("https://github.com/example/current.git"));
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    write(
        dir.path(),
        "README.md",
        "CI の `bun run missing1`\nlefthook の `bun run missing2`\n",
    );
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("missing1"), "{err}");
    assert!(err.contains("missing2"), "{err}");
}

#[test]
fn repo_scope_copy_controls_limits_and_unknown_external_repo_examples() {
    let dir = island_repo(Some("https://github.com/example/current.git"));
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    write(
        dir.path(),
        "config/island-docs-scopes.json",
        r#"{"version":1,"byteLimits":[{"scope":"all","path":"README.md","maxBytes":20}],"linkSourceExclusions":[],"hostPathLinkAllowScopes":[],"indexRecordDirs":[{"scope":"all","dirs":["reviews"]}],"indexRequirements":[],"docsLayouts":[]}"#,
    );
    write(
        dir.path(),
        "README.md",
        "# a deliberately long README\n他 repo（sample）の `bun run external`\n子 repo の `bun run child_example`\n配布する `bun run distributed_example`\n`bun run missing`\n",
    );
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("README.md: 容量上限"), "{err}");
    assert!(err.contains("missing"), "{err}");
    assert!(!err.contains("external"), "{err}");
    assert!(!err.contains("child_example"), "{err}");
    assert!(!err.contains("distributed_example"), "{err}");
}

/// 内蔵の表は無い。写しの無い repo は、どの mode でも検査せずに失敗する。
#[test]
fn missing_repo_scope_copy_fails_without_checking() {
    let dir = island_repo(None);
    fs::remove_file(dir.path().join("config/island-docs-scopes.json")).expect("remove fixture");
    git(dir.path(), &["add", "."]);
    for args in [&["--tracked"][..], &["--staged"][..]] {
        write(dir.path(), "README.md", "# 入口\n[gone](gone.md)\n");
        git(dir.path(), &["add", "README.md"]);
        let out = run(dir.path(), args);
        let err = stderr(&out);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {err}");
        assert!(
            err.contains("config/island-docs-scopes.json がありません"),
            "{args:?}: {err}"
        );
        assert!(!err.contains("gone.md"), "{args:?}: {err}");
        assert!(!err.contains("panicked"), "{args:?}: {err}");
    }
}

#[test]
fn staged_and_range_read_scope_copy_from_their_snapshot() {
    let dir = island_repo(None);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = head(dir.path());
    let scope = |limit| {
        format!(
            "{{\"version\":1,\"byteLimits\":[{{\"scope\":\"all\",\"path\":\"README.md\",\"maxBytes\":{limit}}}],\"linkSourceExclusions\":[],\"hostPathLinkAllowScopes\":[],\"indexRecordDirs\":[{{\"scope\":\"all\",\"dirs\":[\"reviews\"]}}],\"indexRequirements\":[],\"docsLayouts\":[]}}"
        )
    };
    write(dir.path(), "config/island-docs-scopes.json", &scope(10));
    write(dir.path(), "README.md", "# more than ten bytes\n");
    git(
        dir.path(),
        &["add", "README.md", "config/island-docs-scopes.json"],
    );
    write(dir.path(), "config/island-docs-scopes.json", &scope(1000));
    let staged = run(dir.path(), &["--staged"]);
    assert_eq!(staged.status.code(), Some(1), "{}", stderr(&staged));
    assert!(
        stderr(&staged).contains("README.md: 容量上限"),
        "{}",
        stderr(&staged)
    );

    git(dir.path(), &["commit", "-qm", "limited"]);
    let current = head(dir.path());
    let range = run(dir.path(), &["--range", &base, &current]);
    assert_eq!(range.status.code(), Some(1), "{}", stderr(&range));
    assert!(
        stderr(&range).contains("README.md: 容量上限"),
        "{}",
        stderr(&range)
    );
}

#[test]
fn invalid_scope_copy_reports_an_error_without_panicking() {
    let dir = island_repo(None);
    write(dir.path(), "config/island-docs-scopes.json", "not json");
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("config/island-docs-scopes.json"));
    assert!(!stderr(&out).contains("panicked"));
}

#[test]
fn repo_name_with_underscore_does_not_hide_its_own_script() {
    let dir = island_repo(Some("https://github.com/example/unknown_project.git"));
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    write(
        dir.path(),
        "README.md",
        "unknown_project の `bun run own_missing`\n",
    );
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("own_missing"), "{}", stderr(&out));
}

#[test]
fn named_repo_context_without_explicit_marker_checks_missing_script() {
    let dir = island_repo(Some("https://github.com/example/current.git"));
    write(dir.path(), "package.json", r#"{"scripts":{}}"#);
    write(
        dir.path(),
        "README.md",
        "sample 調査では通常の `bun run ship` を使う\nsample の `bun run named_missing`\n",
    );
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(
        err.contains("README.md:1: [script] bun run script がありません -> ship"),
        "{err}"
    );
    assert!(
        err.contains("README.md:2: [script] bun run script がありません -> named_missing"),
        "{err}"
    );
}

#[test]
fn help_uses_the_shared_sections() {
    let out = Command::new(BIN).arg("--help").output().expect("help");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for section in [
        "Usage: develop-docs-check",
        "--range <base> <head>",
        "Default:",
        "Writes:",
        "Exit:",
        "Examples:",
    ] {
        assert!(text.contains(section), "{section}\n{text}");
    }
}

#[test]
fn a_mode_is_required_and_modes_are_exclusive() {
    let dir = repo(None);
    for args in [
        &[][..],
        &["--tracked", "--staged"],
        &["--tracked", "--index-file", "x"],
    ] {
        let out = run(dir.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
    }
}

#[test]
fn tracked_reports_broken_links_and_ignores_untracked_files() {
    let dir = repo(None);
    write(
        dir.path(),
        "README.md",
        "[a](docs/a.md) [u](untracked.md)\n",
    );
    write(
        dir.path(),
        "docs/a.md",
        "# A\n[top](../README.md#nothing)\n",
    );
    git(
        dir.path(),
        &[
            "add",
            "README.md",
            "docs/a.md",
            "config/island-docs-scopes.json",
        ],
    );
    // 手元にはあるが tracked ではない。
    write(dir.path(), "untracked.md", "# u\n");

    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("README.md:1: リンク先が tracked にありません -> untracked.md"),
        "{err}"
    );
    assert!(
        err.contains("docs/a.md:2: リンク先に見出しがありません -> ../README.md#nothing"),
        "{err}"
    );
}

#[test]
fn tracked_rejects_host_paths_in_markdown_references_and_html() {
    let dir = island_repo(None);
    write(
        dir.path(),
        "README.md",
        "# 入口\n[home](/home/u/out.md)\n[users](/Users/u/out.md)\n[tilde](~/out.md)\n[file][local]\n[local]: file:///home/u/out.md\n<a class=\"nav\" href='/home/u/out.md'>host</a>\n[inside](docs/README.md) [web](https://example.com/)\n`[code](/home/u/out.md)`\n```markdown\n[fence](/home/u/out.md)\n```\n",
    );
    git(dir.path(), &["add", "."]);

    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    for (line, target) in [
        (2, "/home/u/out.md"),
        (3, "/Users/u/out.md"),
        (4, "~/out.md"),
        (6, "file:///home/u/out.md"),
        (7, "/home/u/out.md"),
    ] {
        assert!(
            err.contains(&format!(
                "README.md:{line}: 端末の path を指しています -> {target}"
            )),
            "{err}"
        );
    }
    assert!(!err.contains("README.md:8:"), "{err}");
    assert!(!err.contains("README.md:9:"), "{err}");
    assert!(!err.contains("README.md:11:"), "{err}");
}

#[test]
fn tracked_allows_home_path_links_only_for_the_allowed_repo() {
    let content = "# 入口\n[home](/home/u/out.md) [users](/Users/u/out.md) [tilde](~/out.md)\n";
    for (origin, allowed) in [
        (Some("git@github.com:example/home.git"), true),
        (Some("git@github.com:owner/other.git"), false),
        (None, false),
    ] {
        let dir = island_repo(origin);
        write(dir.path(), "README.md", content);
        git(dir.path(), &["add", "."]);
        let out = run(dir.path(), &["--tracked"]);
        let err = stderr(&out);
        if allowed {
            assert!(out.status.success(), "{origin:?}: {err}");
        } else {
            assert_eq!(out.status.code(), Some(1), "{origin:?}: {err}");
            for target in ["/home/u/out.md", "/Users/u/out.md", "~/out.md"] {
                assert!(err.contains(target), "{origin:?}: {err}");
            }
        }
    }

    let home = island_repo(Some("git@github.com:example/home.git"));
    write(
        home.path(),
        "README.md",
        "# 入口\n[file](file:///home/u/out.md)\n",
    );
    git(home.path(), &["add", "."]);
    let out = run(home.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("file:///home/u/out.md"));
}

#[test]
fn tracked_passes_a_clean_repository() {
    let dir = island_repo(None);
    write(
        dir.path(),
        "README.md",
        "# 入口\n[a](docs/a.md#a) [top](#入口)\n",
    );
    write(
        dir.path(),
        "docs/README.md",
        "# docs\n\n[ADR](adr/README.md) [a](a.md)\n",
    );
    write(dir.path(), "docs/a.md", "# A\n");
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "develop-docs-check: OK (9 文書)\n"
    );
}

#[test]
fn tracked_checks_adr_index_count_and_successor() {
    let dir = island_repo(None);
    let adr = "---\nstatus: superseded by ADR-0002\ndate: 2026-09-30\n---\n\n# ADR-0001: First\n\nChosen option: A\n- Good: A\n- Bad: B\n- Neutral: C\n";
    write(dir.path(), "docs/adr/0001-first.md", adr);
    write(
        dir.path(),
        "docs/adr/README.md",
        "# ADR\n- [First](0001-first.md)\n- [Again](0001-first.md#adr-0001-first)\n",
    );
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    let err = stderr(&out);
    assert!(err.contains("現在 2 回"), "{err}");
    assert!(err.contains("後継 ADR-0002 が存在しません"), "{err}");

    write(
        dir.path(),
        "docs/adr/0002-second.md",
        &adr.replace("superseded by ADR-0002", "accepted")
            .replace("ADR-0001", "ADR-0002"),
    );
    write(
        dir.path(),
        "docs/adr/README.md",
        "# ADR\n- [First](0001-first.md)\n- [Second](0002-second.md)\n",
    );
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn changed_adr_index_and_successor_are_checked_in_staged_and_range() {
    let dir = island_repo(None);
    let adr = "---\nstatus: superseded by ADR-0002\ndate: 2026-09-30\n---\n\n# ADR-0001: First\n\nChosen option: A\n- Good: A\n- Bad: B\n- Neutral: C\n";
    write(dir.path(), "docs/adr/0001-first.md", adr);
    write(
        dir.path(),
        "docs/adr/0002-second.md",
        &adr.replace("superseded by ADR-0002", "accepted")
            .replace("ADR-0001", "ADR-0002"),
    );
    write(
        dir.path(),
        "docs/adr/README.md",
        "# ADR\n- [First](0001-first.md)\n- [Second](0002-second.md)\n",
    );
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "baseline"]);
    let base = head(dir.path());

    write(
        dir.path(),
        "docs/adr/README.md",
        "# ADR\n- [First](0001-first.md)\n- [Again](0001-first.md)\n- [Second](0002-second.md)\n",
    );
    git(dir.path(), &["add", "docs/adr/README.md"]);
    let staged = run(dir.path(), &["--staged"]);
    assert!(stderr(&staged).contains("現在 2 回"), "{}", stderr(&staged));
    git(dir.path(), &["commit", "-qm", "duplicate index"]);
    let duplicate = head(dir.path());
    let range = run(dir.path(), &["--range", &base, &duplicate]);
    assert!(stderr(&range).contains("現在 2 回"), "{}", stderr(&range));

    git(dir.path(), &["rm", "docs/adr/0002-second.md"]);
    let staged = run(dir.path(), &["--staged"]);
    assert!(
        stderr(&staged).contains("後継 ADR-0002 が存在しません"),
        "{}",
        stderr(&staged)
    );
    git(dir.path(), &["commit", "-qm", "remove successor"]);
    let removed = head(dir.path());
    let range = run(dir.path(), &["--range", &duplicate, &removed]);
    assert!(
        stderr(&range).contains("後継 ADR-0002 が存在しません"),
        "{}",
        stderr(&range)
    );
}

#[test]
fn tracked_checks_document_names_and_staged_limits_them_to_changed_paths() {
    let dir = island_repo(None);
    write(dir.path(), "docs/Bad_Name.md", "# Bad\n");
    git(dir.path(), &["add", "."]);
    let tracked = run(dir.path(), &["--tracked"]);
    assert_eq!(tracked.status.code(), Some(1), "{}", stderr(&tracked));
    assert!(stderr(&tracked).contains("docs/Bad_Name.md: [docs-name]"));

    git(dir.path(), &["commit", "-qm", "fixture"]);
    write(dir.path(), "README.md", "# 入口\n");
    git(dir.path(), &["add", "README.md"]);
    let staged = run(dir.path(), &["--staged"]);
    assert!(
        !stderr(&staged).contains("[docs-name]"),
        "{}",
        stderr(&staged)
    );

    write(dir.path(), "docs/Bad_Name.md", "# Changed\n");
    git(dir.path(), &["add", "docs/Bad_Name.md"]);
    let changed = run(dir.path(), &["--staged"]);
    assert_eq!(changed.status.code(), Some(1), "{}", stderr(&changed));
    assert!(
        stderr(&changed).contains("docs/Bad_Name.md: [docs-name]"),
        "{}",
        stderr(&changed)
    );
}

#[test]
fn document_names_allow_helpers_and_numeric_versions_only() {
    let dir = island_repo(None);
    write(dir.path(), "docs/releases/_template.md", "# Template\n");
    write(dir.path(), "docs/releases/0.1.144.md", "# Release\n");
    write(dir.path(), "docs/plans/herdr-0.9.0-wait.md", "# Plan\n");
    write(dir.path(), "docs/plans/SPEC.md", "# Spec\n");
    write(dir.path(), "docs/plans/alpha.beta.md", "# Bad\n");
    write(dir.path(), "docs/plans/0.1b.md", "# Bad version\n");
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    let err = stderr(&out);
    for valid in ["_template.md", "0.1.144.md", "herdr-0.9.0-wait.md"] {
        assert!(!err.contains(&format!("{valid}: [docs-name]")), "{err}");
    }
    for invalid in ["SPEC.md", "alpha.beta.md", "0.1b.md"] {
        assert!(err.contains(&format!("{invalid}: [docs-name]")), "{err}");
    }
}

#[test]
fn tracked_accepts_a_large_docs_readme_with_valid_links() {
    let dir = island_repo(Some("https://github.com/owner/other.git"));
    let docs_readme = format!(
        "# docs\n\n[ADR](adr/README.md)\n\n{}\n",
        "index entry text ".repeat(400)
    );
    assert!(docs_readme.len() > 5_000);
    write(dir.path(), "docs/README.md", &docs_readme);
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn tracked_exempts_record_directories_only_from_the_docs_index() {
    let dir = island_repo(None);
    let records = [
        "docs/reviews/x.md",
        "docs/review/x.md",
        "docs/foo/plans/y.md",
        "docs/research/x.md",
        "docs/experiments/x.md",
        "docs/archive/x.md",
    ];
    for path in records {
        write(dir.path(), path, "# Record\n");
    }
    write(dir.path(), "docs/reviews/x.md", "Record\n");
    write(dir.path(), "docs/guides/z.md", "# Guide\n");
    git(dir.path(), &["add", "."]);

    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    for path in records {
        assert!(!err.contains(&format!("{path}: [index]")), "{err}");
    }
    assert!(err.contains("docs/reviews/x.md: [h1]"), "{err}");
    assert!(err.contains("docs/guides/z.md: [index]"), "{err}");
}

#[test]
fn configured_indexes_are_all_required_only_for_the_matching_repo() {
    let origin = "https://github.com/example/site.git";
    let dir = island_repo(Some(origin));
    site_adrs(dir.path());
    write(
        dir.path(),
        "docs/README.md",
        "# docs\n[ADR](adr/README.md)\n[conventions](conventions/README.md)\n[design](design/README.md)\n",
    );
    write(
        dir.path(),
        "docs/conventions/README.md",
        "# conventions\n[workflow](site-production-workflow.md)\n",
    );
    write(
        dir.path(),
        "docs/design/README.md",
        "# design\n[page](site-page-plan.md)\n[animation](animation-stack-plan.md)\n",
    );
    for path in [
        "docs/conventions/site-production-workflow.md",
        "docs/design/site-page-plan.md",
        "docs/design/animation-stack-plan.md",
    ] {
        write(dir.path(), path, "# Document\n");
    }
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    for path in [
        "docs/conventions/site-production-workflow.md",
        "docs/design/site-page-plan.md",
        "docs/design/animation-stack-plan.md",
    ] {
        assert!(err.contains(&format!("{path}: [index]")), "{err}");
    }

    // 上位索引から届いても、ADR 固有の索引が欠ければ違反になる。
    write(
        dir.path(),
        "docs/README.md",
        "# docs\n[ADR](adr/README.md)\n[0016](adr/0016-site-production-adr-tdd-workflow.md)\n[conventions](conventions/README.md)\n[design](design/README.md)\n",
    );
    write(
        dir.path(),
        "docs/adr/README.md",
        "# ADR\n- [0017](0017-animation-stack.md)\n",
    );
    git(dir.path(), &["add", "docs/README.md", "docs/adr/README.md"]);
    let adr = run(dir.path(), &["--tracked"]);
    assert!(
        stderr(&adr).contains("docs/adr/0016-site-production-adr-tdd-workflow.md: [index]"),
        "{}",
        stderr(&adr)
    );

    write(
        dir.path(),
        "docs/adr/README.md",
        "# ADR\n- [0016](0016-site-production-adr-tdd-workflow.md)\n- [0017](0017-animation-stack.md)\n",
    );
    git(dir.path(), &["add", "docs/adr/README.md"]);

    git(
        dir.path(),
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/owner/other.git",
        ],
    );
    let other = run(dir.path(), &["--tracked"]);
    assert!(other.status.success(), "{}", stderr(&other));
}

#[test]
fn range_checks_a_configured_index_when_only_the_index_changes() {
    let dir = island_repo(Some("https://github.com/example/site.git"));
    site_adrs(dir.path());
    write(
        dir.path(),
        "docs/README.md",
        "# docs\n[ADR](adr/README.md)\n[conventions](conventions/README.md)\n[workflow](conventions/site-production-workflow.md)\n",
    );
    write(
        dir.path(),
        "docs/conventions/README.md",
        "# conventions\n[workflow](site-production-workflow.md)\n",
    );
    write(
        dir.path(),
        "docs/conventions/site-production-workflow.md",
        "# Workflow\n",
    );
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = head(dir.path());

    write(
        dir.path(),
        "docs/README.md",
        "# docs\n[ADR](adr/README.md)\n[conventions](conventions/README.md)\n",
    );
    git(dir.path(), &["add", "docs/README.md"]);
    git(dir.path(), &["commit", "-qm", "head"]);
    let current = head(dir.path());
    let out = run(dir.path(), &["--range", &base, &current]);
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(
        err.contains("docs/conventions/site-production-workflow.md: [index]"),
        "{err}"
    );
    assert!(err.contains("docs/README.md"), "{err}");
}

#[test]
fn tracked_checks_crlf_todo_limit_and_record_formats() {
    let dir = island_repo(None);
    let todo_prefix = "# TODO\r\n\r\n## 進行中\r\n\r\n- [ ] **項目**: ";
    let exact = format!(
        "{todo_prefix}{}\r\n",
        "x".repeat(36_000 - todo_prefix.len() - 2)
    );
    assert_eq!(exact.len(), 36_000);
    write(dir.path(), "TODO.md", &exact);
    write(
        dir.path(),
        "CHANGELOG.md",
        "# Changelog\r\n\r\n> 古い分は git 履歴に任せる。\r\n\r\n## 2026-09\r\n\r\n- **成果（2026-09-30）**: 要約。\r\n",
    );
    git(dir.path(), &["add", "."]);
    let valid = run(dir.path(), &["--tracked"]);
    assert!(valid.status.success(), "{}", stderr(&valid));

    write(dir.path(), "TODO.md", &format!("{exact}x"));
    git(dir.path(), &["add", "TODO.md"]);
    let over = run(dir.path(), &["--tracked"]);
    assert_eq!(over.status.code(), Some(1), "{}", stderr(&over));
    assert!(
        stderr(&over).contains("TODO.md: 容量上限を超えています 36001B > 36000B"),
        "{}",
        stderr(&over)
    );

    write(
        dir.path(),
        "TODO.md",
        "# TODO\r\n\r\n## 進行中\r\n\r\n- [x] **完了**\r\n",
    );
    write(
        dir.path(),
        "CHANGELOG.md",
        "# Changelog\r\n\r\n> 古い分は git 履歴に任せる。\r\n\r\n## 2026-09\r\n\r\n- **成果（2026-08-30）**: 要約。\r\n",
    );
    git(dir.path(), &["add", "TODO.md", "CHANGELOG.md"]);
    let invalid = run(dir.path(), &["--tracked"]);
    assert_eq!(invalid.status.code(), Some(1), "{}", stderr(&invalid));
    let err = stderr(&invalid);
    assert!(
        err.contains("TODO.md:5: [todo] TODO.md に完了済みの - [x]"),
        "{err}"
    );
    assert!(
        err.contains(
            "CHANGELOG.md:7: [changelog] 成果項目の日付の年月が見出し 2026-09 と一致しません"
        ),
        "{err}"
    );
}

#[test]
fn tracked_rejects_changelog_items_in_ascending_date_order() {
    let dir = island_repo(None);
    write(
        dir.path(),
        "CHANGELOG.md",
        "# Changelog\n\n> 古い分は git 履歴に任せる。\n\n## 2026-10\n\n- **先の成果（2026-10-03）**: 要約。\n- **後の成果（2026-10-04）**: 要約。\n",
    );
    git(dir.path(), &["add", "."]);

    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out)
            .contains("CHANGELOG.md:8: [changelog] 成果項目は月内で新しい日付から並べてください"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn tracked_accepts_changelog_items_in_descending_or_same_date_order() {
    let dir = island_repo(None);
    write(
        dir.path(),
        "CHANGELOG.md",
        "# Changelog\n\n> 古い分は git 履歴に任せる。\n\n## 2026-10\n\n- **新しい成果（2026-10-04）**: 要約。\n- **同日の成果（2026-10-04）**: 要約。\n- **古い成果（2026-10-03）**: 要約。\n\n## 2026-09\n\n- **前月の成果（2026-09-30）**: 要約。\n",
    );
    git(dir.path(), &["add", "."]);

    let out = run(dir.path(), &["--tracked"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

/// Markdown 以外の registry も index から読む（退役語の語は repo、検査は島）。
#[test]
fn tracked_reads_the_retired_term_registry_from_the_index() {
    let dir = island_repo(None);
    write(
        dir.path(),
        "docs/agents/retired-identifiers.json",
        r#"[{"identifier": "/old-skill", "retiredOn": "2026-08-11", "reason": "改名した"}]"#,
    );
    write(dir.path(), "AGENTS.md", "# agents\n/old-skill を呼ぶ\n");
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("AGENTS.md:2: [retired-term] 退役語 /old-skill があります"),
        "{}",
        stderr(&out)
    );
    // registry を tracked から外すと検査しない（手元の file は読まない）。
    git(
        dir.path(),
        &[
            "rm",
            "-q",
            "--cached",
            "docs/agents/retired-identifiers.json",
        ],
    );
    assert!(run(dir.path(), &["--tracked"]).status.success());
}

/// 既存の切れは staged に関係しなければ報告しない。見出しを消した commit は、触っていない文書の
/// link が切れることも報告する。
#[test]
fn staged_reports_only_links_touching_changed_paths() {
    let dir = repo(None);
    write(dir.path(), "old.md", "[x](missing.md)\n");
    write(dir.path(), "a.md", "[b](b.md#keep)\n");
    write(dir.path(), "b.md", "# keep\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "init"]);

    let out = run(dir.path(), &["--staged"]);
    assert!(out.status.success(), "変更なし: {}", stderr(&out));

    write(dir.path(), "b.md", "# renamed\n");
    git(dir.path(), &["add", "b.md"]);
    let out = run(dir.path(), &["--staged"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("a.md:1: リンク先に見出しがありません -> b.md#keep"),
        "{err}"
    );
    assert!(!err.contains("old.md"), "{err}");
}

#[test]
fn removed_or_renamed_targets_report_unchanged_referrers_in_staged_and_range() {
    for operation in ["rm", "mv"] {
        let dir = repo(None);
        write(dir.path(), "a.md", "[b](b.md)\n");
        write(dir.path(), "b.md", "# b\n");
        write(dir.path(), "old.md", "[old](missing.md)\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "base"]);
        let base = head(dir.path());

        if operation == "rm" {
            git(dir.path(), &["rm", "b.md"]);
        } else {
            git(dir.path(), &["mv", "b.md", "renamed.md"]);
        }
        let staged = run(dir.path(), &["--staged"]);
        assert_eq!(
            staged.status.code(),
            Some(1),
            "{operation}: {}",
            stderr(&staged)
        );
        let err = stderr(&staged);
        assert!(
            err.contains("a.md:1: リンク先が tracked にありません -> b.md"),
            "{err}"
        );
        assert!(!err.contains("old.md"), "{err}");

        git(dir.path(), &["commit", "-qm", operation]);
        let current = head(dir.path());
        let range = run(dir.path(), &["--range", &base, &current]);
        assert_eq!(
            range.status.code(),
            Some(1),
            "{operation}: {}",
            stderr(&range)
        );
        let err = stderr(&range);
        assert!(
            err.contains("a.md:1: リンク先が tracked にありません -> b.md"),
            "{err}"
        );
        assert!(!err.contains("old.md"), "{err}");
    }
}

#[test]
fn range_reports_changed_violations_but_not_existing_ones() {
    let dir = island_repo(None);
    write(dir.path(), "old.md", "[x](missing.md)\n");
    write(dir.path(), "docs/Bad_Name.md", "# bad\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = head(dir.path());

    write(dir.path(), "README.md", "# 入口\n[x](absent.md)\n");
    git(dir.path(), &["add", "README.md"]);
    git(dir.path(), &["commit", "-qm", "changed"]);
    let current = head(dir.path());
    let out = run(dir.path(), &["--range", &base, &current]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("README.md:2: リンク先が tracked にありません"),
        "{err}"
    );
    assert!(!err.contains("old.md"), "{err}");
    assert!(!err.contains("Bad_Name.md"), "{err}");
}

#[test]
fn range_with_missing_or_zero_base_checks_only_the_last_commit() {
    let dir = island_repo(None);
    write(dir.path(), "old.md", "[x](missing.md)\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    write(dir.path(), "README.md", "# 入口\n[x](absent.md)\n");
    git(dir.path(), &["add", "README.md"]);
    git(dir.path(), &["commit", "-qm", "latest"]);
    let current = head(dir.path());
    for base in [
        "0000000000000000000000000000000000000000",
        "ffffffffffffffffffffffffffffffffffffffff",
    ] {
        let out = run(dir.path(), &["--range", base, &current]);
        assert_eq!(out.status.code(), Some(1), "{base}: {}", stderr(&out));
        let err = stderr(&out);
        assert!(
            err.contains("README.md:2: リンク先が tracked にありません"),
            "{err}"
        );
        assert!(!err.contains("old.md"), "{err}");
    }
}

#[test]
fn range_of_root_commit_checks_its_added_files() {
    let dir = repo(None);
    write(dir.path(), "README.md", "[x](missing.md)\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    let current = head(dir.path());
    let out = run(
        dir.path(),
        &[
            "--range",
            "0000000000000000000000000000000000000000",
            &current,
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("README.md:1: リンク先が tracked にありません"));
}

#[test]
fn range_reads_the_head_tree_and_reports_links_to_changed_targets() {
    let dir = island_repo(None);
    write(dir.path(), "docs/a.md", "[b](b.md#keep)\n");
    write(dir.path(), "docs/b.md", "# keep\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = head(dir.path());
    write(dir.path(), "docs/b.md", "# renamed\n");
    git(dir.path(), &["add", "docs/b.md"]);
    git(dir.path(), &["commit", "-qm", "head"]);
    let current = head(dir.path());

    // index には修正済み本文があるが、range は head の tree を読む。
    write(dir.path(), "docs/b.md", "# keep\n");
    git(dir.path(), &["add", "docs/b.md"]);
    let out = run(dir.path(), &["--range", &base, &current]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stderr(&out).contains("docs/a.md:1: リンク先に見出しがありません"));
}

/// pre-commit が渡す一時 index を読む。git dir の外の index は拒否する。
#[test]
fn staged_reads_an_explicit_index_file_inside_the_git_dir() {
    let dir = repo(None);
    write(dir.path(), "a.md", "# a\n");
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "init"]);
    write(dir.path(), "a.md", "[x](missing.md)\n");

    let index = dir.path().join(".git/next-index.lock");
    let mut command = Command::new("git");
    develop_core::strip_git_env(&mut command);
    // 新しい index は空から始まるので、scope 表の写しも入れる。
    let out = command
        .args(["add", "a.md", "config/island-docs-scopes.json"])
        .env("GIT_INDEX_FILE", &index)
        .env("GIT_DIR", dir.path().join(".git"))
        .env("GIT_WORK_TREE", dir.path())
        .current_dir(dir.path())
        .output()
        .expect("git add");
    assert!(out.status.success());
    // 通常の index は変えていないので、明示しなければ変更は見えない。
    let default = run(dir.path(), &["--staged"]);
    assert!(default.status.success(), "{}", stderr(&default));

    let explicit = run(
        dir.path(),
        &["--staged", "--index-file", index.to_str().expect("utf8")],
    );
    assert_eq!(explicit.status.code(), Some(1), "{}", stderr(&explicit));
    assert!(stderr(&explicit).contains("a.md:1: リンク先が tracked にありません -> missing.md"));

    let outside = tempfile::NamedTempFile::new().expect("outside index");
    let rejected = run(
        dir.path(),
        &[
            "--staged",
            "--index-file",
            outside.path().to_str().expect("utf8"),
        ],
    );
    assert_eq!(rejected.status.code(), Some(2));
    assert!(
        stderr(&rejected).contains("git dir の下"),
        "{}",
        stderr(&rejected)
    );
}

/// 上限は島の定数表で決まり、repo に置いた設定 file は読まない（spec の scenario）。島の入口文書の
/// 上限は repo によらず同じ（2026-09-30 決定）。
#[test]
fn limits_follow_the_island_table_and_ignore_repository_settings() {
    for origin in [
        Some("https://github.com/owner/other.git"),
        Some("git@github.com:owner/sample.git"),
        None,
    ] {
        let dir = repo(origin);
        write(dir.path(), "AGENTS.md", &"x".repeat(12_001));
        write(
            dir.path(),
            ".develop-tools.json",
            r#"{"docs":{"limits":{"AGENTS.md":100000}}}"#,
        );
        git(dir.path(), &["add", "."]);
        let out = run(dir.path(), &["--tracked"]);
        assert_eq!(out.status.code(), Some(1), "{origin:?}");
        assert!(
            stderr(&out).contains("AGENTS.md: 容量上限を超えています 12001B > 12000B"),
            "{origin:?}: {}",
            stderr(&out)
        );
    }

    // 上限ちょうどは通る（repo ごとの旧い上限行は島の値へ統合した）。
    let dir = island_repo(Some("https://github.com/owner/other.git"));
    write(dir.path(), "AGENTS.md", &"x".repeat(12_000));
    git(dir.path(), &["add", "."]);
    assert!(run(dir.path(), &["--tracked"]).status.success());
}

/// 島が配った管理区画は `AGENTS.md` の容量に数えない（2026-10-11 決定）。配布先は区画を
/// 削れないので、上限は repo の自前の本文だけに掛ける。
#[test]
fn the_distributed_block_is_not_counted_toward_the_agents_limit() {
    use develop_core::managed_block::{self, AGENTS_MD};

    let with_block = |own: usize| {
        let own = "x".repeat(own - 1) + "\n";
        let plan = managed_block::plan(AGENTS_MD, b"### 3.1 purpose\nkeep\n", own.as_bytes(), None)
            .expect("plan");
        String::from_utf8(plan.updated_target().to_vec()).expect("utf8")
    };
    // 追加時に補う空行 1 byte は自前の本文に数えるので、自前 11,999B + 空行でちょうど上限。
    let at_limit = with_block(11_999);
    assert!(at_limit.len() > 12_000);
    let dir = island_repo(None);
    write(dir.path(), "AGENTS.md", &at_limit);
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert!(out.status.success(), "{}", stderr(&out));

    // 自前の本文が 1 byte 超えれば、区画を除いた値で報告する。
    write(dir.path(), "AGENTS.md", &with_block(12_000));
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("AGENTS.md: 容量上限を超えています 12001B > 12000B"),
        "{}",
        stderr(&out)
    );

    // marker で囲んでも本文が hash と合わなければ除外しない。
    let edited = at_limit.replace("keep", "KEEP");
    write(dir.path(), "AGENTS.md", &edited);
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(&format!(
            "AGENTS.md: 容量上限を超えています {}B > 12000B",
            edited.len()
        )),
        "{}",
        stderr(&out)
    );
}

/// 今の目印でない区画は、配った区画として除かず、ふつうの本文として容量に数える
/// （publish-develop-tools design §1.2 の 4）。
#[test]
fn a_block_in_another_namespace_is_counted_toward_the_agents_limit() {
    use develop_core::managed_block::{self, AGENTS_MD};

    let own = "x".repeat(11_998) + "\n";
    let current = managed_block::plan(AGENTS_MD, b"### 3.1 purpose\nkeep\n", own.as_bytes(), None)
        .expect("plan");
    let current = String::from_utf8(current.updated_target().to_vec()).expect("utf8");
    assert_eq!(
        island_docs::check::limited_bytes("AGENTS.md", current.as_bytes()),
        12_000
    );
    let other = current.replace("develop-tools:AGENTS.md", "prior-tools:AGENTS.md");
    assert_eq!(
        island_docs::check::limited_bytes("AGENTS.md", other.as_bytes()),
        other.len()
    );
    let dir = island_repo(None);
    write(dir.path(), "AGENTS.md", &other);
    git(dir.path(), &["add", "."]);
    let out = run(dir.path(), &["--tracked"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(&format!(
            "AGENTS.md: 容量上限を超えています {}B > 12000B",
            other.len()
        )),
        "{}",
        stderr(&out)
    );
}

fn binary_version() -> String {
    let out = Command::new(BIN)
        .arg("--version")
        .output()
        .expect("version");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(out.stderr.is_empty(), "{}", stderr(&out));
    let version = String::from_utf8(out.stdout).expect("utf8");
    let version = version.trim();
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
    version.to_owned()
}

fn parse_version(raw: &str) -> [u64; 3] {
    let mut parts = raw.split('.');
    let version = [
        parts.next().expect("major").parse().expect("major"),
        parts.next().expect("minor").parse().expect("minor"),
        parts.next().expect("patch").parse().expect("patch"),
    ];
    assert!(parts.next().is_none(), "{raw}");
    version
}

fn format_version(version: [u64; 3]) -> String {
    format!("{}.{}.{}", version[0], version[1], version[2])
}

/// 固定版より手元が新しい（無言になる）比較相手。`0.0.0` にはより古い版が無い。
fn older_than(version: [u64; 3]) -> String {
    if version[2] > 0 {
        format_version([version[0], version[1], version[2] - 1])
    } else if version[1] > 0 {
        format_version([version[0], version[1] - 1, 0])
    } else {
        assert!(version[0] > 0, "0.0.0 has no older pin");
        format_version([version[0] - 1, 0, 0])
    }
}

fn assert_silent_success(out: &Output) {
    assert!(out.status.success(), "{}", stderr(out));
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(stderr(out).is_empty(), "{}", stderr(out));
}

/// `--version` はこの binary の workspace 版だけを出す。
#[test]
fn version_flag_prints_this_binary_version() {
    let version = binary_version();
    assert!(parse_version(&version)[0] < 100, "{version}");
}

/// 移した置き場の固定 file も読み、新旧の両方があれば版の案内ではなく置き場の重複として止める。
#[test]
fn check_local_version_reads_the_moved_pin_and_stops_on_both() {
    let local = binary_version();
    let parsed = parse_version(&local);
    let newer_pin = format_version([parsed[0], parsed[1], parsed[2] + 1]);
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join(".config")).expect("config dir");
    std::fs::write(
        dir.path().join(".config/develop-tools.json"),
        format!(r#"{{"version":"{newer_pin}"}}"#),
    )
    .expect("pin");
    let out = run(dir.path(), &["--check-local-version"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains(&newer_pin), "{}", stderr(&out));

    std::fs::write(dir.path().join(".develop-tools.json"), "{}").expect("legacy pin");
    let out = run(dir.path(), &["--check-local-version"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("manifest_duplicated"),
        "{}",
        stderr(&out)
    );
}

/// 置き場を調べられない（`.config/` を読めない）だけなら、重複ではないので止めない。
#[cfg(unix)]
#[test]
fn check_local_version_stays_silent_when_the_lookup_cannot_read() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join(".config");
    std::fs::create_dir(&config).expect("config dir");
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let out = run(dir.path(), &["--check-local-version"]);
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).expect("restore");
    assert_silent_success(&out);
}

/// 古いときだけ警告し、同じ・新しい・版不明・固定 file 不在でも exit 0 のまま止める。
#[test]
fn check_local_version_warns_only_when_older_and_never_fails() {
    let local = binary_version();
    let parsed = parse_version(&local);
    let newer_pin = format_version([parsed[0], parsed[1], parsed[2] + 1]);
    let dir = tempfile::tempdir().expect("tempdir");
    let pin = dir.path().join(".develop-tools.json");

    assert_silent_success(&run(dir.path(), &["--check-local-version"]));

    for body in [
        "not-json",
        "{}",
        r#"{"version":"bad"}"#,
        r#"{"version":"1.2"}"#,
        r#"{"version":"01.2.3"}"#,
    ] {
        fs::write(&pin, body).expect("pin");
        assert_silent_success(&run(dir.path(), &["--check-local-version"]));
    }

    fs::write(&pin, format!(r#"{{"version":"{local}","sha256":{{}}}}"#)).expect("pin");
    assert_silent_success(&run(dir.path(), &["--check-local-version"]));

    let older_pin = older_than(parsed);
    fs::write(&pin, format!(r#"{{"version":"{older_pin}"}}"#)).expect("pin");
    assert_silent_success(&run(dir.path(), &["--check-local-version"]));

    fs::write(&pin, format!(r#"{{"version":"{newer_pin}"}}"#)).expect("pin");
    let out = run(dir.path(), &["--check-local-version"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(out.stdout.is_empty());
    let text = stderr(&out);
    assert!(text.contains(&local), "{text}");
    assert!(text.contains(&newer_pin), "{text}");
    assert!(text.contains("入れ直して"), "{text}");
    assert!(!text.contains("版を確認できません"), "{text}");
}

/// 旧形式の pin は上の test が持つ。足した欄は版の読み取りを変えない。
#[test]
fn check_local_version_skips_fields_added_beside_the_version() {
    let local = binary_version();
    let parsed = parse_version(&local);
    let newer_pin = format_version([parsed[0], parsed[1], parsed[2] + 1]);
    let dir = tempfile::tempdir().expect("tempdir");
    let pin = dir.path().join(".develop-tools.json");
    let with_added_fields = |version: &str| {
        format!(
            r#"{{"version":"{version}","sha256":{{}},"integrationBranch":"develop","release":{{"versionFiles":[]}}}}"#
        )
    };

    fs::write(&pin, with_added_fields(&local)).expect("pin");
    assert_silent_success(&run(dir.path(), &["--check-local-version"]));

    fs::write(&pin, with_added_fields(&newer_pin)).expect("pin");
    let out = run(dir.path(), &["--check-local-version"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(out.stdout.is_empty());
    let text = stderr(&out);
    assert!(text.contains(&local), "{text}");
    assert!(text.contains(&newer_pin), "{text}");
    assert!(text.contains("入れ直して"), "{text}");
}
