//! `release check` の振る舞い。遠隔は手元の bare repo、GitHub Actions は偽の `gh`。
//!
//! 偽の `gh` が返す JSON は、実際の `gh run list --json` と `gh run view --json jobs` の出力から
//! field の名前と形を写している。`gh workflow run` は記録するだけで、本物の workflow は動かさない。

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
    path: PathBuf,
    head: String,
}

const DECLARED: &str = r#"{"integrationBranch":"develop","platforms":["linux","macos"],"release":{"preflightWorkflow":"ci.yml"}}"#;

fn git(root: &Path, args: &[&str]) -> String {
    let output = develop_tools_core::git(root).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn real_git() -> PathBuf {
    let output = Command::new("/bin/sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(output.status.success());
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
}

fn write_executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// mode ごとに、dispatch の前と後で `gh run list` の答えを変える。
/// 21 は同じ HEAD の以前の run、22 以降が dispatch の後に現れる run。
fn gh_script() -> &'static str {
    r#"#!/bin/sh
log=$RELEASE_CHECK_TEST_LOG
mode=$RELEASE_CHECK_TEST_GH_MODE
{
  printf 'gh'
  for arg in "$@"; do
    printf ' %s' "$arg"
  done
  printf '\n'
} >> "$log"

dispatched=0
lists=0
while IFS= read -r line; do
  case "$line" in
    "gh workflow run "*) dispatched=1 ;;
    "gh run list "*) [ "$dispatched" = 1 ] && lists=$((lists + 1)) ;;
  esac
done < "$log"

commit=""
prev=""
for arg in "$@"; do
  [ "$prev" = "--commit" ] && commit=$arg
  prev=$arg
done

run() {
  if [ "$4" = "null" ]; then conclusion=null; else conclusion="\"$4\""; fi
  printf '{"conclusion":%s,"databaseId":%s,"event":"workflow_dispatch","headBranch":"%s","headSha":"%s","name":"%s","status":"%s","url":"https://github.com/o/r/actions/runs/%s"}' \
    "$conclusion" "$1" "${5:-develop}" "$commit" "$2" "$3" "$1"
}

case "$1 ${2-}" in
  "auth status")
    [ "$mode" = noauth ] && { echo "You are not logged into any GitHub hosts" >&2; exit 1; }
    exit 0
    ;;
  "workflow run")
    # dispatch の直後に別の手が統合先を進めた形。
    if [ "$mode" = moved ]; then
      next=$(git commit-tree "HEAD^{tree}" -p HEAD -m moved)
      git push -q origin "$next:refs/heads/develop"
    fi
    exit 0
    ;;
  "run view")
    printf '{"jobs":[{"conclusion":"success","databaseId":901,"name":"prepare","status":"completed","url":"https://github.com/o/r/actions/runs/%s/job/901"},{"conclusion":"success","databaseId":902,"name":"ci / platform (linux)","status":"completed","url":"https://github.com/o/r/actions/runs/%s/job/902"},{"conclusion":"success","databaseId":903,"name":"ci / platform (macos)","status":"completed","url":"https://github.com/o/r/actions/runs/%s/job/903"}]}\n' "$3" "$3" "$3"
    exit 0
    ;;
  "run list") ;;
  *)
    echo "unexpected gh $*" >&2
    exit 2
    ;;
esac

case "$mode:$dispatched" in
  retry:0) printf '[%s]\n' "$(run 21 CI completed failure)" ;;
  retry:1) printf '[%s,%s]\n' "$(run 21 CI completed failure)" "$(run 22 CI completed success)" ;;
  regress:0) printf '[%s]\n' "$(run 21 CI completed success)" ;;
  regress:1) printf '[%s,%s]\n' "$(run 21 CI completed success)" "$(run 22 CI completed failure)" ;;
  late:0) printf '[%s]\n' "$(run 21 CI completed failure)" ;;
  late:1)
    case "$lists" in
      1) printf '[%s]\n' "$(run 21 CI completed failure)" ;;
      2) printf '[%s,%s]\n' "$(run 21 CI completed failure)" "$(run 22 CI in_progress null)" ;;
      *) printf '[%s,%s]\n' "$(run 21 CI completed failure)" "$(run 22 CI completed success)" ;;
    esac
    ;;
  never:*) printf '[%s]\n' "$(run 21 CI completed success)" ;;
  two:0) printf '[]\n' ;;
  main:0) printf '[]\n' ;;
  main:1) printf '[%s,%s]\n' "$(run 22 CI completed success)" "$(run 23 CI completed success main)" ;;
  moved:*) printf '[]\n' ;;
  two:1) printf '[%s,%s]\n' "$(run 22 CI completed success)" "$(run 23 CI completed success)" ;;
  *)
    echo "unknown mode $mode" >&2
    exit 2
    ;;
esac
"#
}

fn setup(manifest: &str) -> Repo {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let bare = temp.path().join("bare.git");
    let root = temp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(
        temp.path(),
        &["init", "--bare", "-q", "-b", "main", bare.to_str().unwrap()],
    );
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join(".develop-tools.json"), format!("{manifest}\n")).unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "base"]);
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["checkout", "-q", "-b", "develop"]);
    git(&root, &["push", "-q", "origin", "main", "develop"]);
    let head = git(&root, &["rev-parse", "HEAD"]);

    let log = temp.path().join("gh.log");
    std::fs::write(&log, "").unwrap();
    let path = temp.path().join("fake-bin");
    std::fs::create_dir_all(&path).unwrap();
    write_executable(
        &path.join("git"),
        &format!("#!/bin/sh\nexec '{}' \"$@\"\n", real_git().display()),
    );
    write_executable(&path.join("gh"), gh_script());
    Repo {
        _lock,
        _temp: temp,
        root,
        log,
        path,
        head,
    }
}

fn run_check(repo: &Repo, mode: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_release"));
    develop_tools_core::strip_git_env(&mut command);
    command
        .arg("check")
        .current_dir(&repo.root)
        .env("PATH", &repo.path)
        .env("HOME", home(repo))
        .env("RELEASE_CHECK_TEST_LOG", &repo.log)
        .env("RELEASE_CHECK_TEST_GH_MODE", mode)
        .env("GIT_DIR", "/nowhere/.git")
        .env("GIT_WORK_TREE", "/nowhere")
        .output()
        .unwrap()
}

/// 環境側の設定を置く HOME。利用者の本物の設定を読まないよう、test ごとに分ける。
fn home(repo: &Repo) -> PathBuf {
    repo.root.parent().unwrap().join("home")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn log_lines(repo: &Repo) -> Vec<String> {
    std::fs::read_to_string(&repo.log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
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
    let repo = setup(DECLARED);
    let path = repo.path.clone();
    let _env = EnvGuard::set(&[
        ("PATH", path.as_os_str()),
        ("GIT_DIR", std::ffi::OsStr::new("/nowhere/.git")),
        ("GIT_WORK_TREE", std::ffi::OsStr::new("/nowhere")),
        ("RELEASE_CHECK_TEST_LOG", repo.log.as_os_str()),
        ("RELEASE_CHECK_TEST_GH_MODE", std::ffi::OsStr::new(mode)),
    ]);
    let slept = std::cell::Cell::new(std::time::Duration::ZERO);
    let now = std::cell::Cell::new(std::time::Duration::ZERO);
    let result = develop_release::preflight::run_release_check(
        &repo.root,
        || now.get(),
        |step| {
            slept.set(slept.get() + step);
            now.set(now.get() + step);
        },
    );
    (result, slept.get(), repo)
}

#[test]
fn release_check_judges_only_the_run_it_dispatched_after_a_failed_run_on_the_same_head() {
    let repo = setup(DECLARED);
    let output = run_check(&repo, "retry");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(stdout.contains("#22"), "{stdout}");
    assert!(stdout.contains("ci / platform (macos)"), "{stdout}");

    let lines = log_lines(&repo);
    let dispatch = lines
        .iter()
        .position(|line| line == "gh workflow run ci.yml --ref develop")
        .unwrap_or_else(|| panic!("{lines:?}"));
    let first_list = lines
        .iter()
        .position(|line| line.starts_with("gh run list "))
        .unwrap();
    assert!(first_list < dispatch, "記録は dispatch の前: {lines:?}");
    let list = &lines[first_list];
    for part in [
        "--workflow ci.yml",
        "--event workflow_dispatch",
        "--branch develop",
        &format!("--commit {}", repo.head),
    ] {
        assert!(list.contains(part), "{list}");
    }
    assert!(
        lines.contains(&"gh run view 22 --json jobs".to_owned()),
        "{lines:?}"
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.starts_with("gh workflow run"))
            .count(),
        1
    );
}

#[test]
fn release_check_fails_on_the_new_run_even_when_an_earlier_run_succeeded() {
    let repo = setup(DECLARED);
    let output = run_check(&repo, "regress");
    let stderr = text(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("#22"), "{stderr}");
    assert!(stderr.contains("conclusion=failure"), "{stderr}");
    assert!(!stderr.contains("#21"), "{stderr}");
}

#[test]
fn release_check_does_not_dispatch_when_head_is_not_on_origin() {
    let repo = setup(DECLARED);
    std::fs::write(repo.root.join("local.txt"), "local\n").unwrap();
    git(&repo.root, &["add", "local.txt"]);
    git(&repo.root, &["commit", "-qm", "local"]);
    let output = run_check(&repo, "retry");
    let stderr = text(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("origin に届いていません"), "{stderr}");
    assert!(stderr.contains("push"), "{stderr}");
    assert!(
        !log_lines(&repo)
            .iter()
            .any(|line| line.starts_with("gh workflow run")),
        "{:?}",
        log_lines(&repo)
    );
}

#[test]
fn release_check_fails_without_a_preflight_workflow() {
    let repo = setup(r#"{"integrationBranch":"develop"}"#);
    let output = run_check(&repo, "retry");
    let stderr = text(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("release.preflightWorkflow"), "{stderr}");
    assert!(log_lines(&repo).is_empty(), "{:?}", log_lines(&repo));
}

#[test]
fn release_check_works_without_version_files() {
    // 版の file を宣言しない repo でも確認だけはできる。
    let repo = setup(
        r#"{"integrationBranch":"develop","release":{"preflightWorkflow":"platform-check.yml"}}"#,
    );
    assert!(develop_release::version_files::VersionFiles::load(&repo.root).is_err());
    let output = run_check(&repo, "retry");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        log_lines(&repo).contains(&"gh workflow run platform-check.yml --ref develop".to_owned()),
        "{:?}",
        log_lines(&repo)
    );
}

#[test]
fn release_check_waits_for_the_dispatched_run_to_appear_and_finish() {
    let (result, slept, _repo) = run_with_clock("late");
    assert!(result.is_ok(), "{:?}", result.err());
    assert!(slept > std::time::Duration::ZERO);
    assert!(slept < develop_release::ci_runs::CI_WAIT);
}

#[test]
fn release_check_uses_the_release_push_wait_budget_when_no_new_run_appears() {
    let (result, slept, repo) = run_with_clock("never");
    let err = result.expect_err("新しい run が現れなければ失敗する");
    assert!(err.contains("run が無い"), "{err}");
    assert_eq!(slept, develop_release::push::CI_WAIT);
    assert!(
        !log_lines(&repo)
            .iter()
            .any(|line| line.starts_with("gh run view")),
        "以前の run を判定に使わない"
    );
}

#[test]
fn release_check_refuses_to_guess_when_two_new_runs_appear() {
    let repo = setup(DECLARED);
    let output = run_check(&repo, "two");
    let stderr = text(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("特定できません"), "{stderr}");
}

#[test]
fn release_check_ignores_a_dispatch_of_the_same_sha_on_main() {
    let repo = setup(DECLARED);
    let output = run_check(&repo, "main");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(stdout.contains("#22"), "{stdout}");
    assert!(!stdout.contains("#23"), "{stdout}");
}

#[test]
fn release_check_stops_without_waiting_when_the_branch_moves_during_dispatch() {
    let (result, slept, repo) = run_with_clock("moved");
    let err = result.expect_err("統合先が動いたら止める");
    assert!(err.contains("動いた"), "{err}");
    assert!(err.contains("release check をやり直して"), "{err}");
    assert_eq!(slept, std::time::Duration::ZERO, "run が無いまま待たない");
    let lines = log_lines(&repo);
    let dispatch = lines
        .iter()
        .position(|line| line.starts_with("gh workflow run"))
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert!(
        !lines[dispatch..]
            .iter()
            .any(|line| line.starts_with("gh run list")),
        "{lines:?}"
    );
}

#[test]
fn release_check_does_not_dispatch_without_gh_auth() {
    let repo = setup(DECLARED);
    let output = run_check(&repo, "noauth");
    let stderr = text(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("gh の認証ができない"), "{stderr}");
    assert!(
        !log_lines(&repo)
            .iter()
            .any(|line| line.starts_with("gh workflow run") || line.starts_with("gh run list")),
        "{:?}",
        log_lines(&repo)
    );
}

const AI_DECLARED: &str = r#"{"integrationBranch":"develop","release":{"preflightWorkflow":"ci.yml","notes":{"mode":"ai","fallback":"changelog-additions","audience":"public"}}}"#;

#[test]
fn release_check_reports_missing_llm_settings_for_an_ai_repo() {
    let repo = setup(AI_DECLARED);
    let output = run_check(&repo, "retry");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        stdout.contains(".config/develop-tools/llm.json"),
        "{stdout}"
    );
    assert!(stdout.contains("無し"), "{stdout}");
    assert!(stdout.contains("bin/install-tools"), "{stdout}");
}

#[test]
fn release_check_reports_each_llm_settings_field_for_an_ai_repo() {
    let repo = setup(AI_DECLARED);
    let settings = home(&repo).join(".config/develop-tools/llm.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        r#"{"anthropic_api_key_ref":"op://vault/item/field","forbidden_terms":["司令塔"],"usage_ledger":"/state/usage.jsonl"}"#,
    )
    .unwrap();
    let output = run_check(&repo, "retry");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let field = |name: &str| {
        stdout
            .lines()
            .find(|line| line.contains(name))
            .unwrap_or_else(|| panic!("{name}: {stdout}"))
            .to_owned()
    };
    assert!(field("anthropic_api_key_ref").contains("有り"), "{stdout}");
    assert!(field("forbidden_terms").contains("有り"), "{stdout}");
    assert!(field("internal_names").contains("無し"), "{stdout}");
    assert!(field("usage_ledger").contains("有り"), "{stdout}");
    assert!(stdout.contains("bin/install-tools"), "{stdout}");
    assert!(
        !stdout.contains("op://vault"),
        "鍵の参照の値は出さない: {stdout}"
    );
}

/// 欄の無い古い設定では台帳に書けず送らないので、欠けた欄として報告する。
#[test]
fn release_check_reports_a_settings_file_without_usage_ledger_as_incomplete() {
    let repo = setup(AI_DECLARED);
    let settings = home(&repo).join(".config/develop-tools/llm.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(
        &settings,
        r#"{"anthropic_api_key_ref":"op://vault/item/field","forbidden_terms":["司令塔"],"internal_names":["inner"]}"#,
    )
    .unwrap();
    let output = run_check(&repo, "retry");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let ledger = stdout
        .lines()
        .find(|line| line.contains("usage_ledger"))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(ledger.contains("無し"), "{stdout}");
    assert!(stdout.contains("bin/install-tools"), "{stdout}");
}

#[test]
fn release_check_does_not_report_llm_settings_without_ai_notes() {
    let repo = setup(DECLARED);
    let output = run_check(&repo, "retry");
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(!stdout.contains("llm.json"), "{stdout}");
}

#[test]
fn release_check_refuses_an_ai_declaration_without_audience() {
    let repo = setup(
        r#"{"integrationBranch":"develop","release":{"preflightWorkflow":"ci.yml","notes":{"mode":"ai","fallback":"changelog-additions"}}}"#,
    );
    let output = run_check(&repo, "retry");
    let stderr = text(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("release.notes.audience"), "{stderr}");
    assert!(log_lines(&repo).is_empty(), "{:?}", log_lines(&repo));
}
