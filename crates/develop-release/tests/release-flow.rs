use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
}

impl Fixture {
    fn path(&self) -> &Path {
        &self.root
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = develop_tools_core::git(root).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn declaration(root: &Path, integration_branch: Option<&str>, version_files: &str) {
    let branch = integration_branch
        .map(|branch| format!(",\n  \"integrationBranch\": \"{branch}\""))
        .unwrap_or_default();
    std::fs::write(
        root.join(".develop-tools.json"),
        format!(
            "{{\n  \"release\": {{\n    \"versionFiles\": [{version_files}]\n  }}{branch}\n}}\n"
        ),
    )
    .unwrap();
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    let root_path = root.clone();
    let root = root.as_path();
    std::fs::create_dir(root.join("bin")).unwrap();
    std::fs::write(root.join("bin/.keep"), "").unwrap();
    std::fs::create_dir(root.join("tools")).unwrap();
    std::fs::create_dir_all(root.join("tools/stub/src")).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{\"version\":\"0.1.0\",\"private\":true}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tools/Cargo.toml"),
        "[workspace]\nmembers = [\"stub\"]\n[workspace.package]\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tools/stub/Cargo.toml"),
        "[package]\nname = \"stub\"\nversion.workspace = true\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tools/Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"stub\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(root.join("tools/stub/src/lib.rs"), "pub fn stub() {}\n").unwrap();
    declaration(
        root,
        Some("develop"),
        r#"{"path":"package.json","kind":"package-json"},{"path":"tools/Cargo.toml","kind":"cargo-workspace"},{"path":"tools/Cargo.lock","kind":"cargo-lock"}"#,
    );
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgSign", "false"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "initial"]);
    git(root, &["branch", "develop"]);
    Fixture {
        _temp: dir,
        root: root_path,
    }
}

fn git_fixture(dir: tempfile::TempDir, root: std::path::PathBuf) -> Fixture {
    let root_path = root.clone();
    let root = root.as_path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    git(root, &["config", "tag.gpgSign", "false"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "initial"]);
    git(root, &["branch", "develop"]);
    Fixture {
        _temp: dir,
        root: root_path,
    }
}

fn bun_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bun-repo");
    std::fs::create_dir_all(root.join("packages/worker")).unwrap();
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::create_dir_all(root.join("tools")).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"bun-repo\",\n  \"version\": \"1.2.3\",\n  \"private\": true\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("packages/worker/package.json"),
        "{\n  \"name\": \"worker\",\n  \"version\": \"1.2.3\"\n}\n",
    )
    .unwrap();
    declaration(
        &root,
        Some("develop"),
        r#"{"path":"package.json","kind":"package-json"},{"path":"packages/worker/package.json","kind":"package-json"}"#,
    );
    git_fixture(dir, root)
}

fn child_repo_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("child-repo");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("package.json"), "{\"version\":\"1.2.3\"}\n").unwrap();
    std::fs::write(root.join("worker.json"), "{\"version\":\"1.2.3\"}\n").unwrap();
    declaration(
        &root,
        None,
        r#"{"path":"package.json","kind":"package-json"},{"path":"worker.json","kind":"package-json"}"#,
    );
    git_fixture(dir, root)
}

#[test]
fn version_guard_checks_child_repo_without_develop_meta_markers() {
    let dir = child_repo_fixture();
    let root = dir.path();
    assert!(!root.join("bin").exists());
    assert!(!root.join("tools").exists());

    let matching = check_version(root);
    assert!(
        matching.status.success(),
        "{}",
        String::from_utf8_lossy(&matching.stderr)
    );

    std::fs::write(root.join("package.json"), "{\"version\":\"1.2.4\"}\n").unwrap();
    let mismatched = check_version(root);
    assert!(!mismatched.status.success());
    assert!(String::from_utf8_lossy(&mismatched.stderr).contains("1.2.4"));
}

#[test]
fn version_guard_rejects_git_failure_inside_child_repo() {
    let dir = child_repo_fixture();
    let root = dir.path();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/missing\n").unwrap();
    let output = check_version(root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("check-release-version:"));
}

#[test]
fn version_guard_rejects_git_launch_failure_inside_child_repo() {
    let dir = child_repo_fixture();
    let empty_path = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_check-release-version"));
    develop_tools_core::strip_git_env(&mut command);
    let output = command
        .current_dir(dir.path())
        .env("PATH", empty_path.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("git rev-parse を起動できません"));
}

#[test]
fn version_guard_soft_skips_outside_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    let fake_bin = dir.path().join("fake-bin");
    std::fs::create_dir(&fake_bin).unwrap();
    let found = Command::new("/bin/sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(found.status.success());
    let real_git = String::from_utf8(found.stdout).unwrap().trim().to_owned();
    let wrapper = fake_bin.join("git");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ \"$1\" = tag ]; then printf 'v9.9.9\\n'; exit 0; fi\nexec '{}' \"$@\"\n",
            real_git
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_check-release-version"));
    develop_tools_core::strip_git_env(&mut command);
    let output = command
        .current_dir(dir.path())
        .env("PATH", &fake_bin)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn release_commands_report_when_run_outside_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    for args in [&["patch"][..], &["push"][..]] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_release"));
        develop_tools_core::strip_git_env(&mut command);
        let output = command.args(args).current_dir(dir.path()).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Git repo の外"));
    }
}

#[test]
fn version_guard_rejects_broken_git_directory_reference() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".git"), "gitdir: /nonexistent\n").unwrap();
    let output = check_version(dir.path());
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("check-release-version:"));
}

fn single_crate_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("single-crate");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::create_dir_all(root.join("tools")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"single-crate\"\nversion = \"1.2.3\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n").unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"single-crate\"\nversion = \"1.2.3\"\n\n[[package]]\nname = \"single-crate\"\nversion = \"1.2.3\"\nsource = \"registry+https://example.invalid/index\"\n\n[[package]]\nname = \"dependency\"\nversion = \"9.8.7\"\n",
    )
    .unwrap();
    declaration(
        &root,
        Some("develop"),
        r#"{"path":"Cargo.toml","kind":"cargo-package"},{"path":"Cargo.lock","kind":"cargo-lock"}"#,
    );
    git_fixture(dir, root)
}

fn main_only_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("main-only");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::create_dir_all(root.join("tools")).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"main-only\",\n  \"version\": \"1.2.3\"\n}\n",
    )
    .unwrap();
    declaration(
        &root,
        None,
        r#"{"path":"package.json","kind":"package-json"}"#,
    );
    git_fixture(dir, root)
}

#[test]
fn release_fails_without_repo_pin() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["switch", "-q", "develop"]);
    git(root, &["rm", "-q", ".develop-tools.json"]);
    git(root, &["commit", "-qm", "drop pin"]);
    let snapshot = || {
        (
            git(
                root,
                &["rev-parse", "refs/heads/develop", "refs/heads/main"],
            ),
            std::fs::read_to_string(root.join("package.json")).unwrap(),
            std::fs::read_to_string(root.join("tools/Cargo.toml")).unwrap(),
            git(root, &["status", "--porcelain"]),
        )
    };
    let before = snapshot();

    let output = release(root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("integrationBranch"), "{stderr}");
    assert!(git(root, &["tag", "--list"]).is_empty());
    assert_eq!(snapshot(), before, "{stderr}");
}

fn release(root: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_release"));
    develop_tools_core::strip_git_env(&mut command);
    command
        .arg("patch")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

#[test]
fn release_patch_updates_child_repo_without_develop_meta_markers() {
    let dir = child_repo_fixture();
    let root = dir.path();
    assert!(!root.join("bin").exists());
    assert!(!root.join("tools").exists());
    git(root, &["switch", "develop"]);
    declaration(
        root,
        Some("develop"),
        r#"{"path":"package.json","kind":"package-json"},{"path":"worker.json","kind":"package-json"}"#,
    );
    git(root, &["add", ".develop-tools.json"]);
    git(root, &["commit", "-qm", "declare integration branch"]);

    let output = release(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        std::fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("1.2.4")
    );
    assert!(
        std::fs::read_to_string(root.join("worker.json"))
            .unwrap()
            .contains("1.2.4")
    );
    assert_eq!(git(root, &["tag", "--points-at", "HEAD"]), "v1.2.4");
    assert_eq!(
        git(root, &["rev-parse", "main"]),
        git(root, &["rev-parse", "HEAD"])
    );
}

fn check_version(root: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_check-release-version"));
    develop_tools_core::strip_git_env(&mut command);
    command.current_dir(root).output().unwrap()
}

fn check_pushed(root: &Path, stdin: &str) -> Output {
    use std::io::Write;
    let mut command = Command::new(env!("CARGO_BIN_EXE_check-release-version"));
    develop_tools_core::strip_git_env(&mut command);
    let mut child = command
        .arg("--pre-push")
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// child repo の版 file 2 つを書き換えて commit し、その SHA を返す。
fn commit_version(root: &Path, version: &str) -> String {
    for file in ["package.json", "worker.json"] {
        std::fs::write(root.join(file), format!("{{\"version\":\"{version}\"}}\n")).unwrap();
    }
    git(root, &["commit", "-qam", version]);
    git(root, &["rev-parse", "HEAD"])
}

fn tag_line(tag: &str, sha: &str) -> String {
    format!("refs/tags/{tag} {sha} refs/tags/{tag} {}\n", "0".repeat(40))
}

#[test]
fn pushed_tag_whose_commit_declares_another_version_is_rejected() {
    let dir = child_repo_fixture();
    let root = dir.path();
    let first = git(root, &["rev-parse", "HEAD"]);
    git(root, &["tag", "v1.2.4", &first]);
    commit_version(root, "1.2.4");

    assert!(
        check_version(root).status.success(),
        "HEAD に tag が無いので stdin 無しの照合は通る"
    );
    let output = check_pushed(root, &tag_line("v1.2.4", &first));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("v1.2.4") && stderr.contains("1.2.3"),
        "{stderr}"
    );
}

#[test]
fn pushed_tag_outside_main_is_rejected() {
    let dir = child_repo_fixture();
    let root = dir.path();
    git(root, &["checkout", "-qb", "feature"]);
    let feature = commit_version(root, "1.2.4");
    git(root, &["tag", "-a", "-m", "v1.2.4", "v1.2.4"]);
    let tag_object = git(root, &["rev-parse", "v1.2.4"]);
    assert_ne!(tag_object, feature);
    git(root, &["checkout", "-q", "main"]);

    let output = check_pushed(root, &tag_line("v1.2.4", &tag_object));
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("main に含まれていません"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pushed_tags_matching_their_commit_on_main_pass() {
    let dir = child_repo_fixture();
    let root = dir.path();
    let first = git(root, &["rev-parse", "HEAD"]);
    git(root, &["tag", "v1.2.3", &first]);
    commit_version(root, "1.2.4");
    git(root, &["tag", "-a", "-m", "v1.2.4", "v1.2.4"]);
    let annotated = git(root, &["rev-parse", "v1.2.4"]);
    commit_version(root, "1.2.5");

    let stdin = tag_line("v1.2.3", &first) + &tag_line("v1.2.4", &annotated);
    let output = check_pushed(root, &stdin);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("v1.2.3") && stdout.contains("v1.2.4"),
        "{stdout}"
    );
}

#[test]
fn pushed_tag_is_named_by_the_remote_ref() {
    let dir = child_repo_fixture();
    let root = dir.path();
    let head = git(root, &["rev-parse", "HEAD"]);
    git(root, &["tag", "v1.2.3", &head]);
    let zero = "0".repeat(40);
    for local_ref in ["refs/tags/v1.2.3", "HEAD"] {
        let output = check_pushed(
            root,
            &format!("{local_ref} {head} refs/tags/v9.9.9 {zero}\n"),
        );
        assert!(!output.status.success(), "{local_ref}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("v9.9.9"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn pushed_tag_reads_workspace_members_written_with_dot_segments() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    std::fs::create_dir_all(root.join("tools/app")).unwrap();
    std::fs::write(
        root.join("tools/Cargo.toml"),
        "[workspace]\nmembers = [\"./app\"]\n[workspace.package]\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tools/app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion.workspace = true\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tools/Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    declaration(
        &root,
        None,
        r#"{"path":"tools/Cargo.toml","kind":"cargo-workspace"},{"path":"tools/Cargo.lock","kind":"cargo-lock"}"#,
    );
    let fixture = git_fixture(dir, root);
    let root = fixture.path();
    let head = git(root, &["rev-parse", "HEAD"]);
    git(root, &["tag", "v0.1.0", &head]);

    let output = check_pushed(root, &tag_line("v0.1.0", &head));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pushed_branch_and_tag_deletion_lines_are_skipped() {
    let dir = child_repo_fixture();
    let root = dir.path();
    let head = git(root, &["rev-parse", "HEAD"]);
    let zero = "0".repeat(40);
    let stdin = format!(
        "refs/heads/main {head} refs/heads/main {zero}\n(delete) {zero} refs/tags/v9.9.9 {head}\nrefs/tags/not-a-release {head} refs/tags/not-a-release {zero}\n"
    );
    let output = check_pushed(root, &stdin);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("v9.9.9"));

    // tag 以外の行だけでも、作業木の版 file の照合は今まで通り走る。
    std::fs::write(root.join("worker.json"), "{\"version\":\"9.9.9\"}\n").unwrap();
    assert!(!check_pushed(root, &stdin).status.success());
}

#[test]
fn release_refuses_a_branch_other_than_develop_without_writing() {
    let dir = fixture();
    let root = dir.path();
    let before = git(root, &["rev-parse", "HEAD"]);
    let output = release(root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("develop"));
    assert_eq!(git(root, &["rev-parse", "HEAD"]), before);
    assert_eq!(git(root, &["rev-parse", "main"]), before);
    assert!(git(root, &["tag", "-l"]).is_empty());
}

#[test]
fn release_help_points_at_release_push_instead_of_a_manual_tag_push() {
    let output = Command::new(env!("CARGO_BIN_EXE_release"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("It does not push"), "{help}");
    assert!(help.contains("no remote push"), "{help}");
    assert!(help.matches("release push").count() >= 2, "{help}");
    assert!(!help.contains("git push origin"), "{help}");
}

#[test]
fn release_refuses_diverged_main_without_writing() {
    let dir = fixture();
    let root = dir.path();
    std::fs::write(root.join("main-only"), "main\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "main only"]);
    let main = git(root, &["rev-parse", "main"]);
    git(root, &["switch", "develop"]);
    let develop = git(root, &["rev-parse", "HEAD"]);
    let output = release(root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("main"));
    assert_eq!(git(root, &["rev-parse", "main"]), main);
    assert_eq!(git(root, &["rev-parse", "develop"]), develop);
    assert!(git(root, &["tag", "-l"]).is_empty());
}

#[test]
fn release_refuses_missing_declared_package_json() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);
    std::fs::remove_file(root.join("package.json")).unwrap();
    git(root, &["add", "-u"]);
    git(root, &["commit", "-qm", "remove package.json"]);

    let before = git(root, &["rev-parse", "develop"]);
    let output = release(root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("package.json"));
    assert_eq!(git(root, &["rev-parse", "develop"]), before);
    assert!(git(root, &["tag", "-l"]).is_empty());
}

#[test]
fn version_guard_checks_package_version_without_a_tag() {
    let dir = fixture();
    let root = dir.path();
    std::fs::write(root.join("package.json"), "{\"private\":true}\n").unwrap();
    assert!(
        !check_version(root).status.success(),
        "version 欠落は停止する"
    );

    std::fs::write(root.join("package.json"), "{\"version\":\"0.1.0\"}\n").unwrap();
    assert!(
        check_version(root).status.success(),
        "Cargo と一致すれば通す"
    );

    std::fs::write(root.join("package.json"), "{\"version\":\"0.2.0\"}\n").unwrap();
    assert!(!check_version(root).status.success(), "不一致は停止する");

    std::fs::remove_file(root.join("package.json")).unwrap();
    assert!(
        !check_version(root).status.success(),
        "宣言済み file の欠落は停止する"
    );
}

#[test]
fn release_refuses_package_json_without_version() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);
    std::fs::write(root.join("package.json"), "{\n  \"private\": true\n}\n").unwrap();
    git(root, &["add", "package.json"]);
    git(root, &["commit", "-qm", "package without version"]);
    let before = git(root, &["rev-parse", "develop"]);
    let main_before = git(root, &["rev-parse", "main"]);

    let output = release(root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("package.json"));
    assert_eq!(git(root, &["rev-parse", "develop"]), before);
    assert_eq!(git(root, &["rev-parse", "main"]), main_before);
    assert!(git(root, &["tag", "-l"]).is_empty());
}

#[test]
fn release_refuses_package_version_mismatch_without_writing() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);
    std::fs::write(root.join("package.json"), "{\"version\":\"0.2.0\"}\n").unwrap();
    git(root, &["add", "package.json"]);
    git(root, &["commit", "-qm", "mismatched package version"]);
    let before = git(root, &["rev-parse", "develop"]);

    let output = release(root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("package.json"));
    assert_eq!(git(root, &["rev-parse", "develop"]), before);
    assert_eq!(
        git(root, &["rev-parse", "main"]),
        git(root, &["rev-parse", "HEAD~"])
    );
    assert!(git(root, &["tag", "-l"]).is_empty());
}

#[test]
fn release_commits_matching_cargo_and_package_versions_without_reformatting() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);
    let package = "{\n\t\"name\": \"fixture\",\n\t\"version\" : \"0.1.0\",\n\t\"private\": true\n}";
    std::fs::write(root.join("package.json"), package).unwrap();
    git(root, &["add", "package.json"]);
    git(root, &["commit", "-qm", "matching package version"]);

    let output = release(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let released = git(root, &["rev-parse", "develop"]);
    assert_eq!(git(root, &["rev-parse", "main"]), released);
    assert_eq!(git(root, &["rev-parse", "v0.1.1"]), released);
    assert_eq!(
        std::fs::read_to_string(root.join("package.json")).unwrap(),
        package.replace("0.1.0", "0.1.1")
    );
    assert!(
        std::fs::read_to_string(root.join("tools/Cargo.toml"))
            .unwrap()
            .contains("version = \"0.1.1\"")
    );
    let changed = git(
        root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            &released,
        ],
    );
    assert_eq!(
        changed.lines().collect::<Vec<_>>(),
        vec!["package.json", "tools/Cargo.lock", "tools/Cargo.toml"]
    );
}

#[test]
fn release_fast_forwards_main_and_tags_the_develop_commit_without_push() {
    let dir = fixture();
    let root = dir.path();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-q"]);
    git(
        root,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(root, &["push", "-q", "origin", "main", "develop"]);
    let remote_main = git(remote.path(), &["rev-parse", "refs/heads/main"]);
    let remote_develop = git(remote.path(), &["rev-parse", "refs/heads/develop"]);
    git(root, &["switch", "develop"]);
    let main_checkout = tempfile::tempdir().unwrap();
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            main_checkout.path().to_str().unwrap(),
            "main",
        ],
    );
    let main_tree_before = std::fs::read(main_checkout.path().join("tools/Cargo.toml")).unwrap();
    std::fs::write(root.join("feature"), "feature\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "feature"]);

    let output = release(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let released = git(root, &["rev-parse", "develop"]);
    assert_eq!(git(root, &["rev-parse", "main"]), released);
    assert_eq!(git(root, &["rev-parse", "v0.1.1"]), released);
    assert_eq!(
        std::fs::read(main_checkout.path().join("tools/Cargo.toml")).unwrap(),
        main_tree_before
    );
    assert_eq!(
        git(remote.path(), &["rev-parse", "refs/heads/main"]),
        remote_main
    );
    assert_eq!(
        git(remote.path(), &["rev-parse", "refs/heads/develop"]),
        remote_develop
    );
    assert!(git(remote.path(), &["tag", "-l"]).is_empty());
    assert!(check_version(root).status.success());
}

#[test]
fn release_from_linked_develop_worktree_updates_canonical_refs() {
    let dir = fixture();
    let root = dir.path();
    let linked = root.parent().unwrap().join("linked-develop");
    git(
        root,
        &["worktree", "add", "-q", linked.to_str().unwrap(), "develop"],
    );
    let output = release(&linked);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let released = git(&linked, &["rev-parse", "HEAD"]);
    assert_eq!(git(root, &["rev-parse", "main"]), released);
    assert_eq!(git(root, &["rev-parse", "v0.1.1"]), released);
}

#[test]
fn version_guard_rejects_a_matching_tag_outside_main() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);
    std::fs::write(root.join("feature"), "feature\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "feature"]);
    git(root, &["tag", "v0.1.0"]);
    assert!(!check_version(root).status.success());
}

#[test]
fn mismatched_tag_suggests_reissuing_with_release() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["tag", "v0.1.1"]);
    let output = check_version(root);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("release <patch | minor | major | x.y.z>")
    );
}

#[test]
fn detached_tag_checkout_can_check_the_fetched_main_ref() {
    let dir = fixture();
    let root = dir.path();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-q"]);
    git(
        root,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(root, &["push", "-q", "origin", "main"]);
    git(root, &["tag", "v0.1.0"]);
    git(root, &["switch", "--detach"]);
    git(root, &["branch", "-D", "main"]);
    assert!(!check_version(root).status.success());
    git(root, &["fetch", "origin", "main:refs/heads/main"]);
    assert!(check_version(root).status.success());
}

#[test]
fn bun_only_release_updates_declared_package_json_files() {
    let dir = bun_fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);

    let output = release(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let released = git(root, &["rev-parse", "HEAD"]);
    assert_eq!(git(root, &["rev-parse", "main"]), released);
    assert_eq!(git(root, &["rev-parse", "v1.2.4"]), released);
    assert_eq!(git(root, &["log", "-1", "--format=%s"]), "🔖 v1.2.4");
    assert!(check_version(root).status.success());
    assert!(
        std::fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("\"version\": \"1.2.4\"")
    );
    assert!(
        std::fs::read_to_string(root.join("packages/worker/package.json"))
            .unwrap()
            .contains("\"version\": \"1.2.4\"")
    );
}

#[test]
fn single_crate_release_updates_only_its_lock_package() {
    let dir = single_crate_fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);

    let output = release(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        std::fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("version = \"1.2.4\"")
    );
    let released = git(root, &["rev-parse", "HEAD"]);
    assert_eq!(git(root, &["rev-parse", "main"]), released);
    assert_eq!(git(root, &["rev-parse", "v1.2.4"]), released);
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
    assert_eq!(git(root, &["log", "-1", "--format=%s"]), "🔖 v1.2.4");
    assert!(lock.contains("name = \"single-crate\"\nversion = \"1.2.4\""));
    assert!(lock.contains("name = \"single-crate\"\nversion = \"1.2.3\"\nsource = \"registry+https://example.invalid/index\""));
    assert!(lock.contains("name = \"dependency\"\nversion = \"9.8.7\""));
    assert!(check_version(root).status.success());
}

#[test]
fn toml_root_release_changes_only_the_top_level_version() {
    let dir = main_only_fixture();
    let root = dir.path();
    let plugin = "# plugin version\nversion = \"1.2.3\"\n\n[metadata]\nversion = \"9.9.9\"\n";
    std::fs::write(root.join("plugin.toml"), plugin).unwrap();
    declaration(
        root,
        None,
        r#"{"path":"package.json","kind":"package-json"},{"path":"plugin.toml","kind":"toml-root"}"#,
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "declare plugin version"]);

    let output = release(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("plugin.toml")).unwrap(),
        plugin.replacen("1.2.3", "1.2.4", 1)
    );
    assert!(check_version(root).status.success());
}

#[test]
fn version_guard_rejects_a_plugin_manifest_version_that_disagrees_with_cargo() {
    let dir = single_crate_fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);
    let plugin = "id = \"single-crate\"\nversion = \"1.2.3\"\nmin_herdr_version = \"0.9.3\"\n\n[[build]]\ncommand = [\"cargo\", \"build\"]\n";
    std::fs::write(root.join("herdr-plugin.toml"), plugin).unwrap();
    declaration(
        root,
        Some("develop"),
        r#"{"path":"Cargo.toml","kind":"cargo-package"},{"path":"herdr-plugin.toml","kind":"toml-root"},{"path":"Cargo.lock","kind":"cargo-lock"}"#,
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "declare plugin manifest version"]);
    assert!(check_version(root).status.success());

    std::fs::write(
        root.join("herdr-plugin.toml"),
        plugin.replacen("1.2.3", "1.2.4", 1),
    )
    .unwrap();
    git(root, &["commit", "-qam", "bump only the plugin manifest"]);
    let output = check_version(root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("herdr-plugin.toml"));
}

#[test]
fn workspace_fixture_releases_with_declared_workspace_and_lock_files() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);

    let output = release(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let released = git(root, &["rev-parse", "HEAD"]);
    assert_eq!(git(root, &["rev-parse", "main"]), released);
    assert_eq!(git(root, &["rev-parse", "v0.1.1"]), released);
    assert!(check_version(root).status.success());
    assert!(
        std::fs::read_to_string(root.join("tools/Cargo.toml"))
            .unwrap()
            .contains("version = \"0.1.1\"")
    );
}

/// `tools/` を持たず、repo root の Cargo.toml が workspace の Rust repo。
fn root_workspace_fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root-workspace");
    for member in ["core", "cli"] {
        std::fs::create_dir_all(root.join("crates").join(member).join("src")).unwrap();
        std::fs::write(
            root.join("crates").join(member).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{member}\"\nversion.workspace = true\nedition = \"2021\"\n"
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("crates").join(member).join("src/lib.rs"),
            "pub fn value() -> u8 { 1 }\n",
        )
        .unwrap();
    }
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/core\", \"crates/cli\"]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"2.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"cli\"\nversion = \"2.0.0\"\n\n[[package]]\nname = \"core\"\nversion = \"2.0.0\"\n\n[[package]]\nname = \"dependency\"\nversion = \"2.0.0\"\nsource = \"registry+https://example.invalid/index\"\n",
    )
    .unwrap();
    declaration(
        &root,
        Some("develop"),
        r#"{"path":"Cargo.toml","kind":"cargo-workspace"},{"path":"Cargo.lock","kind":"cargo-lock"}"#,
    );
    git_fixture(dir, root)
}

#[test]
fn root_workspace_release_bumps_the_workspace_and_its_members_in_the_lock() {
    let dir = root_workspace_fixture();
    let root = dir.path();
    assert!(!root.join("tools").exists());
    git(root, &["switch", "develop"]);
    let member = std::fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap();

    let output = release(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let released = git(root, &["rev-parse", "HEAD"]);
    assert_eq!(git(root, &["rev-parse", "main"]), released);
    assert_eq!(git(root, &["rev-parse", "v2.0.1"]), released);
    assert_eq!(git(root, &["log", "-1", "--format=%s"]), "🔖 v2.0.1");
    assert!(
        std::fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("[workspace.package]\nversion = \"2.0.1\"")
    );
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
    assert!(
        lock.contains("name = \"cli\"\nversion = \"2.0.1\""),
        "{lock}"
    );
    assert!(
        lock.contains("name = \"core\"\nversion = \"2.0.1\""),
        "{lock}"
    );
    assert!(
        lock.contains("name = \"dependency\"\nversion = \"2.0.0\"\nsource"),
        "{lock}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("crates/core/Cargo.toml")).unwrap(),
        member
    );
    let changed = git(
        root,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            &released,
        ],
    );
    assert_eq!(
        changed.lines().collect::<Vec<_>>(),
        vec!["Cargo.lock", "Cargo.toml"]
    );
    assert!(check_version(root).status.success());
}

#[test]
fn version_guard_rejects_a_root_workspace_member_that_disagrees_in_the_lock() {
    let dir = root_workspace_fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);
    assert!(check_version(root).status.success());

    let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
    std::fs::write(
        root.join("Cargo.lock"),
        lock.replacen(
            "name = \"core\"\nversion = \"2.0.0\"",
            "name = \"core\"\nversion = \"2.0.1\"",
            1,
        ),
    )
    .unwrap();
    git(
        root,
        &["commit", "-qam", "bump only one member in the lock"],
    );
    let before = git(root, &["rev-parse", "HEAD"]);

    let output = check_version(root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cargo.lock"));
    let output = release(root);
    assert!(!output.status.success());
    assert_eq!(git(root, &["rev-parse", "HEAD"]), before);
    assert!(git(root, &["tag", "-l"]).is_empty());
}

#[test]
fn release_fails_when_the_declaration_has_no_version_files() {
    let dir = fixture();
    let root = dir.path();
    git(root, &["switch", "-q", "develop"]);
    std::fs::write(
        root.join(".develop-tools.json"),
        "{\n  \"integrationBranch\": \"develop\"\n}\n",
    )
    .unwrap();
    git(root, &["commit", "-qam", "drop version files"]);
    let before = git(root, &["rev-parse", "HEAD"]);
    let cargo = std::fs::read_to_string(root.join("tools/Cargo.toml")).unwrap();

    let output = release(root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("release"), "{stderr}");
    assert_eq!(git(root, &["rev-parse", "HEAD"]), before);
    assert_eq!(
        std::fs::read_to_string(root.join("tools/Cargo.toml")).unwrap(),
        cargo
    );
    assert!(git(root, &["tag", "-l"]).is_empty());
    assert!(!check_version(root).status.success());
}

#[test]
fn main_integration_release_keeps_remote_refs_unchanged() {
    let dir = main_only_fixture();
    let root = dir.path();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-q"]);
    git(
        root,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(root, &["push", "-q", "origin", "main"]);
    let remote_main = git(remote.path(), &["rev-parse", "refs/heads/main"]);

    let output = release(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let released = git(root, &["rev-parse", "main"]);
    assert_eq!(git(root, &["rev-parse", "v1.2.4"]), released);
    assert_eq!(
        git(remote.path(), &["rev-parse", "refs/heads/main"]),
        remote_main
    );
    assert!(git(remote.path(), &["tag", "-l"]).is_empty());
}

#[test]
fn root_package_json_must_be_declared() {
    let dir = bun_fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);
    declaration(
        root,
        Some("develop"),
        r#"{"path":"packages/worker/package.json","kind":"package-json"}"#,
    );
    git(root, &["add", ".develop-tools.json"]);
    git(root, &["commit", "-qm", "omit root version declaration"]);
    let before = git(root, &["rev-parse", "HEAD"]);

    let output = release(root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("package.json"));
    assert_eq!(git(root, &["rev-parse", "HEAD"]), before);
    assert!(git(root, &["tag", "-l"]).is_empty());
}

#[test]
fn unknown_version_file_kind_is_rejected_without_writing() {
    let dir = bun_fixture();
    let root = dir.path();
    git(root, &["switch", "develop"]);
    declaration(
        root,
        Some("develop"),
        r#"{"path":"package.json","kind":"unknown-kind"}"#,
    );
    git(root, &["add", ".develop-tools.json"]);
    git(root, &["commit", "-qm", "unknown version kind"]);
    let before = git(root, &["rev-parse", "HEAD"]);

    let output = release(root);
    assert!(!output.status.success());
    assert_eq!(git(root, &["rev-parse", "HEAD"]), before);
    assert!(git(root, &["tag", "-l"]).is_empty());
}

/// 版上げ前の照合が読む偽の `gh`。mode ごとに HEAD の dispatch の run と job を返し、呼び出しを記録する。
/// JSON の field は実際の `gh run list --json` と `gh run view --json jobs` の出力から写している。
fn platform_gh_script() -> &'static str {
    r#"#!/bin/sh
printf 'gh %s\n' "$*" >> "$RELEASE_PLATFORM_TEST_LOG"
mode=$RELEASE_PLATFORM_TEST_GH_MODE
commit=""
prev=""
for arg in "$@"; do
  [ "$prev" = "--commit" ] && commit=$arg
  prev=$arg
done
run() {
  if [ "$4" = "null" ]; then conclusion=null; else conclusion="\"$4\""; fi
  printf '{"conclusion":%s,"databaseId":%s,"event":"workflow_dispatch","headBranch":"develop","headSha":"%s","name":"%s","status":"%s","url":"https://github.com/o/r/actions/runs/%s"}' \
    "$conclusion" "$1" "$commit" "$2" "$3" "$1"
}
job() {
  printf '{"conclusion":"%s","databaseId":%s,"name":"%s","status":"completed","url":"https://github.com/o/r/actions/runs/1/job/%s"}' "$3" "$1" "$2" "$1"
}
green="$(job 901 'ci / platform (linux)' success),$(job 902 'ci / platform (macos)' success)"
case "$1 ${2-}" in
  "auth status") exit 0 ;;
  "run list")
    case "$mode" in
      norun) printf '[]\n' ;;
      # 古い順に返す。先頭を取ると古い #21 を読んで落ちるので、ID の最大で選ぶことを示せる。
      newer) printf '[%s,%s]\n' "$(run 21 CI completed failure)" "$(run 22 CI completed success)" ;;
      running) printf '[%s,%s]\n' "$(run 22 CI in_progress null)" "$(run 21 CI completed success)" ;;
      *) printf '[%s]\n' "$(run 21 CI completed success)" ;;
    esac
    ;;
  "run view")
    case "$mode:$3" in
      red:*) jobs="$(job 901 'ci / platform (linux)' success),$(job 902 'ci / platform (macos)' failure)" ;;
      newer:21) jobs="$(job 901 'ci / platform (linux)' success),$(job 902 'ci / platform (macos)' failure)" ;;
      macos-only:*) jobs="$(job 901 'ci / platform (linux)' failure),$(job 902 'ci / platform (macos)' success)" ;;
      bare:*) jobs="$(job 901 'platform (linux)' success),$(job 902 'platform (macos)' success)" ;;
      twice:*) jobs="$green,$(job 903 'other / platform (macos)' success)" ;;
      *) jobs="$green" ;;
    esac
    printf '{"jobs":[%s]}\n' "$jobs"
    ;;
  *)
    echo "unexpected gh $*" >&2
    exit 2
    ;;
esac
"#
}

struct PlatformRepo {
    fixture: Fixture,
    log: std::path::PathBuf,
    bin: std::path::PathBuf,
}

/// `platforms` を宣言した（または宣言しない）repo を develop に置き、偽の `gh` を用意する。
fn platform_repo(platforms: Option<&str>) -> PlatformRepo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("platform-repo");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("package.json"), "{\"version\":\"1.2.3\"}\n").unwrap();
    let declared = platforms
        .map(|list| {
            format!(
                ",\n  \"platforms\": {list},\n  \"release\": {{\"preflightWorkflow\": \"ci.yml\", \"versionFiles\": [{{\"path\":\"package.json\",\"kind\":\"package-json\"}}]}}"
            )
        })
        .unwrap_or_else(|| {
            ",\n  \"release\": {\"versionFiles\": [{\"path\":\"package.json\",\"kind\":\"package-json\"}]}".to_owned()
        });
    std::fs::write(
        root.join(".develop-tools.json"),
        format!("{{\n  \"integrationBranch\": \"develop\"{declared}\n}}\n"),
    )
    .unwrap();
    let log = dir.path().join("gh.log");
    std::fs::write(&log, "").unwrap();
    let bin = dir.path().join("fake-bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(bin.join("gh"), platform_gh_script()).unwrap();
    std::fs::set_permissions(bin.join("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let fixture = git_fixture(dir, root);
    git(fixture.path(), &["switch", "-q", "develop"]);
    PlatformRepo { fixture, log, bin }
}

fn release_with_platforms(repo: &PlatformRepo, mode: &str) -> Output {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![repo.bin.clone()];
    paths.extend(std::env::split_paths(&path));
    let mut command = Command::new(env!("CARGO_BIN_EXE_release"));
    develop_tools_core::strip_git_env(&mut command);
    command
        .arg("patch")
        .current_dir(repo.fixture.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("RELEASE_PLATFORM_TEST_LOG", &repo.log)
        .env("RELEASE_PLATFORM_TEST_GH_MODE", mode)
        .output()
        .unwrap()
}

fn gh_calls(repo: &PlatformRepo) -> Vec<String> {
    std::fs::read_to_string(&repo.log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn assert_released(repo: &PlatformRepo, output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let root = repo.fixture.path();
    assert_eq!(git(root, &["tag", "--points-at", "HEAD"]), "v1.2.4");
    assert!(
        std::fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("1.2.4")
    );
}

/// 失敗した版上げは版 file・commit・tag を変えない。
fn assert_not_released(repo: &PlatformRepo, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!output.status.success(), "版を上げてしまった: {stderr}");
    let root = repo.fixture.path();
    assert_eq!(git(root, &["tag", "--list"]), "", "{stderr}");
    assert_eq!(
        git(root, &["rev-parse", "develop"]),
        git(root, &["rev-parse", "main"])
    );
    assert!(
        std::fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("1.2.3")
    );
    stderr
}

#[test]
fn release_bumps_when_every_declared_platform_job_is_green() {
    let repo = platform_repo(Some(r#"["linux", "macos"]"#));
    let output = release_with_platforms(&repo, "green");
    assert_released(&repo, &output);
    let calls = gh_calls(&repo);
    let head = git(repo.fixture.path(), &["rev-parse", "HEAD~1"]);
    let list = calls
        .iter()
        .find(|line| line.starts_with("gh run list "))
        .unwrap_or_else(|| panic!("{calls:?}"));
    for part in [
        "--workflow ci.yml",
        "--event workflow_dispatch",
        "--branch develop",
        &format!("--commit {head}"),
    ] {
        assert!(list.contains(part), "{list}");
    }
    assert!(
        calls.contains(&"gh run view 21 --json jobs".to_owned()),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|line| line.starts_with("gh workflow run")),
        "版上げは dispatch しない: {calls:?}"
    );
}

#[test]
fn release_refuses_when_one_platform_job_is_red() {
    let repo = platform_repo(Some(r#"["linux", "macos"]"#));
    let output = release_with_platforms(&repo, "red");
    let stderr = assert_not_released(&repo, &output);
    assert!(
        stderr.contains("ci / platform (macos) が failure"),
        "{stderr}"
    );
    assert!(stderr.contains("release check"), "{stderr}");
}

#[test]
fn release_judges_only_the_newest_run_when_an_older_one_failed() {
    let repo = platform_repo(Some(r#"["linux", "macos"]"#));
    let output = release_with_platforms(&repo, "newer");
    assert_released(&repo, &output);
    let calls = gh_calls(&repo);
    assert!(
        calls.contains(&"gh run view 22 --json jobs".to_owned()),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|line| line.starts_with("gh run view 21")),
        "{calls:?}"
    );
}

#[test]
fn release_refuses_while_the_newest_run_is_still_running() {
    let repo = platform_repo(Some(r#"["linux", "macos"]"#));
    let output = release_with_platforms(&repo, "running");
    let stderr = assert_not_released(&repo, &output);
    assert!(stderr.contains("#22 がまだ in_progress"), "{stderr}");
    assert!(stderr.contains("release check の完了を待って"), "{stderr}");
}

#[test]
fn release_refuses_when_a_newly_declared_platform_has_no_job() {
    let repo = platform_repo(Some(r#"["linux", "macos", "windows"]"#));
    let output = release_with_platforms(&repo, "green");
    let stderr = assert_not_released(&repo, &output);
    assert!(
        stderr.contains("windows: platform (windows) の job がありません"),
        "{stderr}"
    );
}

#[test]
fn release_refuses_when_head_has_no_dispatched_run() {
    let repo = platform_repo(Some(r#"["linux", "macos"]"#));
    let output = release_with_platforms(&repo, "norun");
    let stderr = assert_not_released(&repo, &output);
    assert!(stderr.contains("dispatch の run がありません"), "{stderr}");
    assert!(stderr.contains("先に release check を実行"), "{stderr}");
}

#[test]
fn release_without_platforms_never_calls_gh() {
    let repo = platform_repo(None);
    let output = release_with_platforms(&repo, "red");
    assert_released(&repo, &output);
    assert!(gh_calls(&repo).is_empty(), "{:?}", gh_calls(&repo));
}

#[test]
fn release_with_only_macos_declared_ignores_the_linux_job() {
    let repo = platform_repo(Some(r#"["macos"]"#));
    let output = release_with_platforms(&repo, "macos-only");
    assert_released(&repo, &output);
}

#[test]
fn release_matches_platform_jobs_with_or_without_a_caller_prefix() {
    let nested = platform_repo(Some(r#"["linux", "macos"]"#));
    assert_released(&nested, &release_with_platforms(&nested, "green"));
    let bare = platform_repo(Some(r#"["linux", "macos"]"#));
    assert_released(&bare, &release_with_platforms(&bare, "bare"));
}

#[test]
fn release_refuses_when_two_jobs_match_one_platform() {
    let repo = platform_repo(Some(r#"["linux", "macos"]"#));
    let output = release_with_platforms(&repo, "twice");
    let stderr = assert_not_released(&repo, &output);
    assert!(
        stderr.contains("macos: platform (macos) に当たる job が 2 個"),
        "{stderr}"
    );
}

/// 刻印の宣言の有無だけを変えた Bun の repo。`CHANGELOG.md` は `None` なら置かない。
fn changelog_fixture(stamp: bool, changelog: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("changelog-repo");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"changelog-repo\",\n  \"version\": \"1.2.3\"\n}\n",
    )
    .unwrap();
    let stamp = if stamp {
        ",\n    \"changelogStamp\": true"
    } else {
        ""
    };
    std::fs::write(
        root.join(".develop-tools.json"),
        format!(
            "{{\n  \"release\": {{\n    \"versionFiles\": [{{\"path\":\"package.json\",\"kind\":\"package-json\"}}]{stamp}\n  }},\n  \"integrationBranch\": \"develop\"\n}}\n"
        ),
    )
    .unwrap();
    if let Some(changelog) = changelog {
        std::fs::write(root.join("CHANGELOG.md"), changelog).unwrap();
    }
    let fixture = git_fixture(dir, root);
    git(fixture.path(), &["switch", "develop"]);
    fixture
}

/// 読めない CHANGELOG.md（同名 directory）を置いた Bun の repo。
fn unreadable_changelog_fixture(stamp: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("changelog-repo");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"changelog-repo\",\n  \"version\": \"1.2.3\"\n}\n",
    )
    .unwrap();
    let stamp = if stamp {
        ",\n    \"changelogStamp\": true"
    } else {
        ""
    };
    std::fs::write(
        root.join(".develop-tools.json"),
        format!(
            "{{\n  \"release\": {{\n    \"versionFiles\": [{{\"path\":\"package.json\",\"kind\":\"package-json\"}}]{stamp}\n  }},\n  \"integrationBranch\": \"develop\"\n}}\n"
        ),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("CHANGELOG.md")).unwrap();
    std::fs::write(root.join("CHANGELOG.md/keep"), "").unwrap();
    let fixture = git_fixture(dir, root);
    git(fixture.path(), &["switch", "develop"]);
    fixture
}

fn released_files(root: &Path) -> Vec<String> {
    git(root, &["show", "--format=", "--name-only", "HEAD"])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn assert_released_v124(root: &Path, output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(git(root, &["tag", "--points-at", "HEAD"]), "v1.2.4");
    assert_eq!(git(root, &["log", "-1", "--format=%s"]), "🔖 v1.2.4");
    assert_eq!(git(root, &["status", "--porcelain"]), "");
}

const UNRELEASED_CHANGELOG: &str = "# CHANGELOG\n\n## 2026-10\n\n- **刻印を足した（2026-10-20、未リリース）**: 本文。\n- **前の版で出した（2026-10-19、v1.2.3 で prod 反映）**: 本文。\n";

#[test]
fn declared_repo_stamps_dated_unreleased_titles_in_the_version_commit() {
    let dir = changelog_fixture(true, Some(UNRELEASED_CHANGELOG));
    let root = dir.path();

    let output = release(root);
    assert_released_v124(root, &output);
    assert_eq!(
        std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap(),
        UNRELEASED_CHANGELOG.replace(
            "（2026-10-20、未リリース）",
            "（2026-10-20、v1.2.4 で prod 反映）"
        )
    );
    assert_eq!(released_files(root), ["CHANGELOG.md", "package.json"]);
}

#[test]
fn undeclared_repo_does_not_read_changelog_when_it_is_a_directory() {
    let dir = unreadable_changelog_fixture(false);
    let root = dir.path();

    let output = release(root);
    assert_released_v124(root, &output);
    assert_eq!(released_files(root), ["package.json"]);

    let declared = unreadable_changelog_fixture(true);
    let declared_root = declared.path();
    let head = git(declared_root, &["rev-parse", "HEAD"]);
    let output = release(declared_root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("CHANGELOG.md"), "{stderr}");
    assert_eq!(git(declared_root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(declared_root, &["status", "--porcelain"]), "");
    assert!(git(declared_root, &["tag", "--list"]).is_empty());
}

#[test]
fn nested_body_heading_and_undated_markers_are_not_stamped() {
    let untouched = [
        "## 2026-10（2026-10-20、未リリース）",
        "- **日付の無い括弧（未リリース）**: 本文。",
        "  - **入れ子（2026-10-20、未リリース）**: 本文。",
        "未リリース（2026-10-20、未リリース）",
        "- **題の途中（2026-10-20、未リリース）だけ**: 本文。",
        "- **閉じない太字（2026-10-20、未リリース）",
        "- **日付でない括弧（段階 C、未リリース）**: 本文。",
        "- **日付で始まらない括弧（段階 C、2026-10-20、未リリース）**: 本文。",
    ];
    let stamped = "- **題（2026-10-20、未リリース）**: 本文の（2026-10-19、未リリース）は残す。";
    let changelog = format!("# CHANGELOG\n\n{}\n{stamped}\n", untouched.join("\n"));
    let dir = changelog_fixture(true, Some(&changelog));
    let root = dir.path();

    let output = release(root);
    assert_released_v124(root, &output);
    let written = std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
    assert_eq!(
        written,
        format!(
            "# CHANGELOG\n\n{}\n- **題（2026-10-20、v1.2.4 で prod 反映）**: 本文の（2026-10-19、未リリース）は残す。\n",
            untouched.join("\n")
        )
    );
}

#[test]
fn declared_repo_without_unreleased_titles_releases_without_changelog() {
    let changelog = "# CHANGELOG\n\n## 2026-10\n\n- **前の版で出した（2026-10-19、v1.2.3 で prod 反映）**: 本文。\n- **日付の無い括弧（未リリース）**: 本文。\n";
    let dir = changelog_fixture(true, Some(changelog));
    let root = dir.path();

    let output = release(root);
    assert_released_v124(root, &output);
    assert_eq!(
        std::fs::read(root.join("CHANGELOG.md")).unwrap(),
        changelog.as_bytes()
    );
    assert_eq!(released_files(root), ["package.json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("✓ package.json を v1.2.4 の版 commit 用に更新"),
        "{stdout}"
    );
    assert!(!stdout.contains("CHANGELOG.md"), "{stdout}");
}

#[test]
fn declared_repo_without_changelog_refuses_before_writing() {
    let dir = changelog_fixture(true, None);
    let root = dir.path();
    let head = git(root, &["rev-parse", "HEAD"]);

    let output = release(root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("CHANGELOG.md"), "{stderr}");
    assert_eq!(git(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(root, &["status", "--porcelain"]), "");
    assert!(git(root, &["tag", "--list"]).is_empty());
}
