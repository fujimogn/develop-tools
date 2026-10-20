//! `release push` の振る舞い。遠隔は手元の bare repo、CI は偽の `gh`。
//!
//! skipped と neutral は成功とみなさない。design は「すべて成功」とだけ書いており、
//! この 2 つを成功に含めていない。取消とは分け、失敗として conclusion を報告する。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct Repo {
    _lock: std::sync::MutexGuard<'static, ()>,
    _temp: tempfile::TempDir,
    root: PathBuf,
    log: PathBuf,
    path_with_gh: PathBuf,
    path_without_gh: PathBuf,
    release_sha: String,
    previous_sha: String,
}

#[derive(Clone, Copy)]
enum Workflow {
    Tracked,
    UntrackedOnly,
}

#[derive(Clone, Copy)]
enum ServerHook {
    None,
    RejectMain,
    RewriteDevelop,
}

fn git_wrapper(git_bin: &Path) -> String {
    format!(
        "#!/bin/sh\nif [ \"${{1-}}\" = push ]; then\n  {{\n    printf 'git'\n    for arg in \"$@\"; do\n      printf ' %s' \"$arg\"\n    done\n    printf '\\n'\n  }} >> \"$RELEASE_PUSH_TEST_LOG\"\nfi\nexec '{git}' \"$@\"\n",
        git = git_bin.display()
    )
}

fn real_git() -> PathBuf {
    let output = Command::new("/bin/sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(output.status.success());
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = develop_tools_core::git(root).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn executable(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    executable(path);
}

fn gh_script() -> &'static str {
    r#"#!/bin/sh
log=$RELEASE_PUSH_TEST_LOG
mode=$RELEASE_PUSH_TEST_GH_MODE
{
  printf 'gh'
  for arg in "$@"; do
    printf ' %s' "$arg"
  done
  printf '\n'
} >> "$log"

if [ "${1-}" = "auth" ]; then
  if [ "$mode" = "auth-fail" ]; then
    echo "not logged in" >&2
    exit 1
  fi
  exit 0
fi

commit=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--commit" ]; then
    commit=$arg
  fi
  prev=$arg
done
if [ -z "$commit" ]; then
  echo "commit がありません" >&2
  exit 2
fi

json_run() {
  id=$1
  name=$2
  status=$3
  conclusion=$4
  branch=${5:-main}
  if [ "$conclusion" = "null" ]; then
    conclusion_json=null
  else
    conclusion_json="\"$conclusion\""
  fi
  printf '{"databaseId":%s,"name":"%s","status":"%s","conclusion":%s,"event":"push","headSha":"%s","headBranch":"%s","url":"https://example.invalid/runs/%s"}' \
    "$id" "$name" "$status" "$conclusion_json" "$commit" "$branch" "$id"
}

case "$mode" in
  success)
    printf '[%s,%s]\n' "$(json_run 11 ci completed success)" "$(json_run 12 release-check completed success)"
    ;;
  failure)
    printf '[%s,%s]\n' "$(json_run 11 ci completed success)" "$(json_run 12 release-check completed failure)"
    ;;
  cancelled)
    printf '[%s]\n' "$(json_run 12 release-check completed cancelled)"
    ;;
  skipped)
    printf '[%s]\n' "$(json_run 12 release-check completed skipped)"
    ;;
  neutral)
    printf '[%s]\n' "$(json_run 13 release-check completed neutral)"
    ;;
  empty)
    printf '[]\n'
    ;;
  stuck)
    printf '[%s]\n' "$(json_run 7 ci in_progress null)"
    ;;
  push-and-pr)
    printf '[%s,{"databaseId":99,"name":"pr","status":"in_progress","conclusion":null,"event":"pull_request","headSha":"%s","headBranch":"main","url":"https://example.invalid/runs/99"}]\n' \
      "$(json_run 11 ci completed success)" "$commit"
    ;;
  develop-only)
    printf '[%s]\n' "$(json_run 11 ci completed success develop)"
    ;;
  late)
    count=0
    while IFS= read -r line; do
      case "$line" in
        "gh run list "*) count=$((count + 1)) ;;
      esac
    done < "$log"
    printf 'late %s\n' "$count" >> "$log"
    case "$count" in
      1) printf '[%s]\n' "$(json_run 11 ci in_progress null)" ;;
      2) printf '[%s]\n' "$(json_run 11 ci completed success)" ;;
      3)
        printf '[%s,%s]\n' "$(json_run 11 ci completed success)" "$(json_run 12 release-check in_progress null)"
        ;;
      *)
        printf '[%s,%s]\n' "$(json_run 11 ci completed success)" "$(json_run 12 release-check completed success)"
        ;;
    esac
    ;;
  *)
    echo "unknown mode $mode" >&2
    exit 2
    ;;
esac
"#
}

fn setup(integration: &str, workflow: Workflow, server_hook: ServerHook) -> Repo {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let bare = temp.path().join("bare.git");
    let root = temp.path().join("repo");
    let git_bin = real_git();
    std::fs::create_dir_all(&root).unwrap();
    git(
        temp.path(),
        &["init", "--bare", "-q", "-b", "main", bare.to_str().unwrap()],
    );
    git(
        &bare,
        &[
            "config",
            "core.hooksPath",
            bare.join("hooks").to_str().unwrap(),
        ],
    );
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["config", "tag.gpgSign", "false"]);
    let hooks = root.join(".git/hooks");
    git(
        &root,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    std::fs::write(
        root.join(".develop-tools.json"),
        format!("{{\"integrationBranch\":\"{integration}\"}}\n"),
    )
    .unwrap();
    if matches!(workflow, Workflow::Tracked) {
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(root.join(".github/workflows/ci.yml"), "name: ci\n").unwrap();
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "base"]);
    let previous_sha = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["push", "-q", "origin", "main"]);
    if integration != "main" {
        git(&root, &["branch", "develop"]);
        git(&root, &["push", "-q", "origin", "develop"]);
        git(&root, &["checkout", "-q", "develop"]);
    }
    std::fs::write(root.join("release.txt"), "release\n").unwrap();
    git(&root, &["add", "release.txt"]);
    git(&root, &["commit", "-qm", "release"]);
    if integration != "main" {
        git(&root, &["branch", "-f", "main", "HEAD"]);
    }
    git(&root, &["tag", "v0.1.1"]);
    let release_sha = git(&root, &["rev-parse", "HEAD"]);
    assert_ne!(release_sha, previous_sha);

    if matches!(workflow, Workflow::UntrackedOnly) {
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(root.join(".github/workflows/ci.yml"), "name: ci\n").unwrap();
    }

    let log = temp.path().join("push.log");
    std::fs::write(&log, "").unwrap();
    write_executable(
        &hooks.join("pre-push"),
        "#!/bin/sh\nwhile read local_ref local_sha remote_ref remote_sha; do\n  printf '%s %s\\n' \"$local_ref\" \"$remote_ref\" >> \"$RELEASE_PUSH_TEST_LOG\"\ndone\nexit 0\n",
    );
    match server_hook {
        ServerHook::None => {}
        ServerHook::RejectMain => write_executable(
            &bare.join("hooks/pre-receive"),
            "#!/bin/sh\nwhile read old new ref; do\n  if [ \"$ref\" = \"refs/heads/main\" ]; then\n    echo \"reject main\" >&2\n    exit 1\n  fi\ndone\nexit 0\n",
        ),
        ServerHook::RewriteDevelop => write_executable(
            &bare.join("hooks/post-update"),
            "#!/bin/sh\nfor ref in \"$@\"; do\n  if [ \"$ref\" = \"refs/heads/develop\" ]; then\n    parent=$(git rev-parse \"${ref}^\")\n    git update-ref \"$ref\" \"$parent\"\n  fi\ndone\n",
        ),
    }

    let path_with_gh = temp.path().join("fake-bin");
    let path_without_gh = temp.path().join("git-only");
    std::fs::create_dir_all(&path_with_gh).unwrap();
    std::fs::create_dir_all(&path_without_gh).unwrap();
    write_executable(&path_with_gh.join("git"), &git_wrapper(&git_bin));
    write_executable(&path_without_gh.join("git"), &git_wrapper(&git_bin));
    write_executable(&path_with_gh.join("gh"), gh_script());

    Repo {
        _lock,
        _temp: temp,
        root,
        log,
        path_with_gh,
        path_without_gh,
        release_sha,
        previous_sha,
    }
}

fn run_push(repo: &Repo, mode: &str, with_gh: bool, extra: &[&str]) -> Output {
    let _keep_lock = &repo._lock;
    let path = if with_gh {
        &repo.path_with_gh
    } else {
        &repo.path_without_gh
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_release"));
    develop_tools_core::strip_git_env(&mut command);
    command
        .arg("push")
        .args(extra)
        .current_dir(&repo.root)
        .env("PATH", path)
        .env("RELEASE_PUSH_TEST_LOG", &repo.log)
        .env("RELEASE_PUSH_TEST_GH_MODE", mode)
        .env("GIT_DIR", "/nowhere/.git")
        .env("GIT_WORK_TREE", "/nowhere")
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn remote_sha(root: &Path, spec: &str) -> String {
    let listed = git(root, &["ls-remote", "origin", spec]);
    listed.split_whitespace().next().unwrap_or("").to_owned()
}

fn pushed_refs(repo: &Repo) -> Vec<String> {
    std::fs::read_to_string(&repo.log)
        .unwrap()
        .lines()
        .filter(|line| line.starts_with("refs/"))
        .map(|line| line.split_whitespace().next().unwrap().to_owned())
        .collect()
}

fn log_text(repo: &Repo) -> String {
    std::fs::read_to_string(&repo.log).unwrap()
}

#[test]
fn release_push_checks_each_remote_sha_and_pushes_the_tag_only_after_ci() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::None);
    assert!(!repo.root.join("bin").exists());
    assert!(!repo.root.join("tools").exists());
    let output = run_push(&repo, "success", true, &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(!text(&output.stderr).contains("警告"));
    assert_eq!(
        pushed_refs(&repo),
        vec![
            "refs/heads/develop".to_owned(),
            "refs/heads/main".to_owned(),
            "refs/tags/v0.1.1".to_owned(),
        ]
    );
    let log = log_text(&repo);
    let main_at = log
        .lines()
        .position(|line| line.starts_with("refs/heads/main"))
        .unwrap();
    let gh_at = log
        .lines()
        .position(|line| line.starts_with("gh run list"))
        .unwrap();
    let tag_at = log
        .lines()
        .position(|line| line.starts_with("refs/tags/"))
        .unwrap();
    assert!(main_at < gh_at && gh_at < tag_at);
    let gh_line = log
        .lines()
        .find(|line| line.starts_with("gh run list"))
        .unwrap();
    assert!(gh_line.contains("--event push"), "{gh_line}");
    assert!(gh_line.contains("--branch main"), "{gh_line}");
    assert!(gh_line.contains("headBranch"), "{gh_line}");
    assert!(gh_line.contains(&repo.release_sha), "{gh_line}");
    assert!(
        log.contains("git push origin refs/heads/develop:refs/heads/develop"),
        "{log}"
    );
    assert!(
        log.contains("git push origin refs/heads/main:refs/heads/main"),
        "{log}"
    );
    assert!(
        log.contains("git push origin refs/tags/v0.1.1:refs/tags/v0.1.1"),
        "{log}"
    );
    assert_eq!(
        remote_sha(&repo.root, "refs/heads/develop"),
        repo.release_sha
    );
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.release_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), repo.release_sha);
}

#[test]
fn release_push_stops_before_main_when_the_integration_sha_does_not_match() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::RewriteDevelop);
    let output = run_push(&repo, "success", true, &[]);
    assert!(!output.status.success());
    let err = text(&output.stderr);
    assert!(err.contains("一致"), "{err}");
    assert!(err.contains(&repo.release_sha), "{err}");
    assert!(err.contains(&repo.previous_sha), "{err}");
    assert_eq!(pushed_refs(&repo), vec!["refs/heads/develop".to_owned()]);
    assert_eq!(
        remote_sha(&repo.root, "refs/heads/develop"),
        repo.previous_sha
    );
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.previous_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
    assert_eq!(
        git(&repo.root, &["rev-parse", "refs/tags/v0.1.1"]),
        repo.release_sha
    );
}

#[test]
fn release_push_reports_how_far_it_got_when_main_is_rejected() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::RejectMain);
    let output = run_push(&repo, "success", true, &[]);
    assert!(!output.status.success());
    let err = text(&output.stderr);
    assert!(err.contains("develop: 届いた"), "{err}");
    assert!(err.contains("main: 届いていない"), "{err}");
    assert!(err.contains("手元"), "{err}");
    assert_eq!(
        pushed_refs(&repo),
        vec![
            "refs/heads/develop".to_owned(),
            "refs/heads/main".to_owned()
        ]
    );
    assert_eq!(
        remote_sha(&repo.root, "refs/heads/develop"),
        repo.release_sha
    );
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.previous_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
}

#[test]
fn release_push_leaves_the_tag_local_when_a_run_fails() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::None);
    let output = run_push(&repo, "failure", true, &[]);
    assert!(!output.status.success());
    let err = text(&output.stderr);
    assert!(err.contains("失敗"), "{err}");
    assert!(err.contains("release-check"), "{err}");
    assert!(err.contains("#12"), "{err}");
    assert!(err.contains("https://example.invalid/runs/12"), "{err}");
    assert!(!err.contains("取消"), "{err}");
    assert!(!err.contains("時間切れ"), "{err}");
    assert!(!err.contains("run が無い"), "{err}");
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.release_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
    assert!(
        !pushed_refs(&repo)
            .iter()
            .any(|item| item.starts_with("refs/tags/"))
    );
}

#[test]
fn release_push_reports_cancellation_separately_from_failure() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::None);
    let output = run_push(&repo, "cancelled", true, &[]);
    assert!(!output.status.success());
    let err = text(&output.stderr);
    assert!(err.contains("取消"), "{err}");
    assert!(err.contains("release-check"), "{err}");
    assert!(err.contains("#12"), "{err}");
    assert!(!err.contains("失敗"), "{err}");
    assert!(!err.contains("時間切れ"), "{err}");
    assert!(!err.contains("run が無い"), "{err}");
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
}

#[test]
fn release_push_treats_skipped_and_neutral_as_failure() {
    for mode in ["skipped", "neutral"] {
        let repo = setup("develop", Workflow::Tracked, ServerHook::None);
        let output = run_push(&repo, mode, true, &[]);
        assert!(!output.status.success(), "{mode}");
        let err = text(&output.stderr);
        assert!(err.contains("失敗"), "{mode}: {err}");
        assert!(err.contains(mode), "{mode}: {err}");
        assert!(!err.contains("取消"), "{mode}: {err}");
        assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
        assert_eq!(
            git(&repo.root, &["rev-parse", "refs/tags/v0.1.1"]),
            repo.release_sha
        );
    }
}

#[test]
fn release_push_stops_without_gh_and_keeps_the_tag() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::None);
    let output = run_push(&repo, "success", false, &[]);
    assert!(!output.status.success());
    let err = text(&output.stderr);
    assert!(err.contains("gh が無い"), "{err}");
    assert!(err.contains("--tag-without-ci-check"), "{err}");
    assert!(!err.contains("認証"), "{err}");
    assert_eq!(
        remote_sha(&repo.root, "refs/heads/develop"),
        repo.release_sha
    );
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.release_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
    assert!(!log_text(&repo).contains("gh "));
}

#[test]
fn release_push_stops_when_gh_is_not_authenticated() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::None);
    let output = run_push(&repo, "auth-fail", true, &[]);
    assert!(!output.status.success());
    let err = text(&output.stderr);
    assert!(err.contains("認証"), "{err}");
    assert!(err.contains("--tag-without-ci-check"), "{err}");
    assert!(!err.contains("gh が無い"), "{err}");
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.release_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
}

#[test]
fn release_push_warns_when_the_ci_check_is_skipped() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::None);
    let output = run_push(&repo, "auth-fail", false, &["--tag-without-ci-check"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let err = text(&output.stderr);
    assert!(err.contains("警告"), "{err}");
    assert!(err.contains("CI"), "{err}");
    assert!(!log_text(&repo).contains("gh "));
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), repo.release_sha);
}

#[test]
fn release_push_does_not_wait_without_a_tracked_workflow() {
    let repo = setup("develop", Workflow::UntrackedOnly, ServerHook::None);
    let output = run_push(&repo, "success", false, &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(!text(&output.stderr).contains("警告"));
    assert!(!log_text(&repo).contains("gh "));
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), repo.release_sha);
    assert_eq!(
        pushed_refs(&repo),
        vec![
            "refs/heads/develop".to_owned(),
            "refs/heads/main".to_owned(),
            "refs/tags/v0.1.1".to_owned(),
        ]
    );
}

#[test]
fn release_push_pushes_main_once_when_it_is_the_integration_branch() {
    let repo = setup("main", Workflow::Tracked, ServerHook::None);
    let output = run_push(&repo, "success", true, &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        pushed_refs(&repo),
        vec!["refs/heads/main".to_owned(), "refs/tags/v0.1.1".to_owned()]
    );
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.release_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), repo.release_sha);
}

struct EnvGuard {
    keys: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn set(pairs: &[(&'static str, &std::ffi::OsStr)]) -> Self {
        let keys = pairs
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in pairs {
            // SAFETY: setup が `TEST_LOCK` を保持しているので、この file の他 test は
            // 環境変数を読まない。この thread だけが書き換える。
            unsafe { std::env::set_var(key, value) }
        }
        Self { keys }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.keys {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn run_with_clock(mode: &str) -> (Result<(), String>, std::time::Duration, Repo) {
    let repo = setup("develop", Workflow::Tracked, ServerHook::None);
    let _keep_lock = &repo._lock;
    let path = repo.path_with_gh.clone();
    let _env = EnvGuard::set(&[
        ("PATH", path.as_os_str()),
        ("GIT_DIR", std::ffi::OsStr::new("/nowhere/.git")),
        ("GIT_WORK_TREE", std::ffi::OsStr::new("/nowhere")),
        ("RELEASE_PUSH_TEST_LOG", repo.log.as_os_str()),
        ("RELEASE_PUSH_TEST_GH_MODE", std::ffi::OsStr::new(mode)),
    ]);
    let slept = std::cell::Cell::new(std::time::Duration::ZERO);
    let now = std::cell::Cell::new(std::time::Duration::ZERO);
    let result = develop_release::push::run_release_push(
        &repo.root,
        false,
        || now.get(),
        |step| {
            slept.set(slept.get() + step);
            now.set(now.get() + step);
        },
    );
    (result, slept.get(), repo)
}

#[test]
fn release_push_reports_no_runs_after_the_single_wait_budget() {
    let (result, slept, repo) = run_with_clock("empty");
    let err = result.expect_err("run が無いときは失敗する");
    assert!(err.contains("run が無い"), "{err}");
    assert!(!err.contains("時間切れ"), "{err}");
    assert_eq!(slept, develop_release::push::CI_WAIT);
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.release_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
}

#[test]
fn release_push_reports_timeout_separately_when_a_run_does_not_finish() {
    let (result, slept, repo) = run_with_clock("stuck");
    let err = result.expect_err("終わらない run は失敗する");
    assert!(err.contains("時間切れ"), "{err}");
    assert!(err.contains("ci"), "{err}");
    assert!(err.contains("#7"), "{err}");
    assert!(err.contains("in_progress"), "{err}");
    assert!(!err.contains("run が無い"), "{err}");
    assert_eq!(slept, develop_release::push::CI_WAIT);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
}

#[test]
fn release_push_waits_until_every_push_run_succeeds() {
    let (result, slept, repo) = run_with_clock("late");
    assert!(result.is_ok(), "{:?}", result.err());
    assert!(slept < develop_release::push::CI_WAIT);
    let log = log_text(&repo);
    let late5 = log.lines().position(|line| line == "late 5").expect(&log);
    let tag = log
        .lines()
        .position(|line| line.starts_with("refs/tags/"))
        .expect(&log);
    assert!(late5 < tag, "{log}");
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), repo.release_sha);
}

#[test]
fn release_push_ignores_runs_that_were_not_triggered_by_push() {
    let (result, slept, repo) = run_with_clock("push-and-pr");
    assert!(result.is_ok(), "{:?}", result.err());
    assert_eq!(slept, develop_release::push::POLL_INTERVAL);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), repo.release_sha);
}

#[test]
fn release_push_does_not_tag_when_only_the_develop_run_has_finished() {
    let (result, slept, repo) = run_with_clock("develop-only");
    let err = result.expect_err("develop の完了済み run だけでは tag を出さない");
    assert!(err.contains("run が無い"), "{err}");
    assert!(!err.contains("失敗"), "{err}");
    let log = log_text(&repo);
    let gh_line = log
        .lines()
        .find(|line| line.starts_with("gh run list"))
        .unwrap_or("");
    assert!(gh_line.contains("--branch main"), "{gh_line}");
    assert!(gh_line.contains(&repo.release_sha), "{gh_line}");
    assert_eq!(slept, develop_release::push::CI_WAIT);
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.release_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), "");
    assert_eq!(
        git(&repo.root, &["rev-parse", "refs/tags/v0.1.1"]),
        repo.release_sha
    );
}

#[test]
fn release_push_accepts_a_remote_tag_that_already_points_at_the_same_sha() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::None);
    git(
        &repo.root,
        &["push", "origin", "refs/tags/v0.1.1:refs/tags/v0.1.1"],
    );
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), repo.release_sha);
    let output = run_push(&repo, "success", true, &[]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.release_sha);
    assert_eq!(remote_sha(&repo.root, "refs/tags/v0.1.1"), repo.release_sha);
}

#[test]
fn release_push_rejects_a_remote_tag_that_points_at_another_sha() {
    let repo = setup("develop", Workflow::Tracked, ServerHook::None);
    let spec = format!("{}:refs/tags/v0.1.1", repo.previous_sha);
    git(&repo.root, &["push", "origin", &spec]);
    assert_eq!(
        remote_sha(&repo.root, "refs/tags/v0.1.1"),
        repo.previous_sha
    );
    let output = run_push(&repo, "success", true, &[]);
    assert!(!output.status.success());
    let err = text(&output.stderr);
    assert!(err.contains("拒否"), "{err}");
    assert!(err.contains("develop: 届いた"), "{err}");
    assert!(err.contains("main: 届いた"), "{err}");
    assert!(err.contains("手元"), "{err}");
    assert_eq!(
        remote_sha(&repo.root, "refs/heads/develop"),
        repo.release_sha
    );
    assert_eq!(remote_sha(&repo.root, "refs/heads/main"), repo.release_sha);
    assert_eq!(
        remote_sha(&repo.root, "refs/tags/v0.1.1"),
        repo.previous_sha
    );
    assert_eq!(
        git(&repo.root, &["rev-parse", "refs/tags/v0.1.1"]),
        repo.release_sha
    );
}
