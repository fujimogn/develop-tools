//! `develop-secrets` を、PATH の先頭に置いた**偽の `op`** で起動して確かめる。実物の op は呼ばない
//! （PATH の先頭が偽物で、token も偽の値。呼ばれた記録が残ることで偽物が使われたと分かる）。
//!
//! 値は `sentinel-…` の偽の値だけを使い、stdout / stderr に出ないことを確かめる。

use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_develop-secrets");

/// 偽の `op`。`$FAKE_OP_DIR/refs/<参照の / と : を _ にした名前>/` の file で振る舞いを決める。
///
/// - `value`: 値（op と同じく末尾に改行を足して出す）
/// - `exit` / `stderr`: 失敗させるときの exit code と stderr
/// - `fail-times`: 最初の n 回だけ失敗させる（無ければ毎回）
/// - `$FAKE_OP_DIR/deny-vault-<vault>`: `op vault get` を失敗させる
/// - `$FAKE_OP_DIR/capture-stdin`: 在れば `op read` が受けた stdin を `stdin-seen` へ写す
const FAKE_OP: &str = r#"#!/bin/sh
dir="$FAKE_OP_DIR"
printf '%s\n' "$*" >> "$dir/calls"
printf '%s\n' "${OP_SERVICE_ACCOUNT_TOKEN:+token-set}" >> "$dir/token-seen"
case "$1" in
  vault)
    if [ -e "$dir/deny-vault-$3" ]; then
      echo "[ERROR] \"$3\" isn't a vault in this account (sentinel-stderr)" >&2
      exit 1
    fi
    exit 0
    ;;
  read)
    ref_dir="$dir/refs/$(printf '%s' "$2" | tr '/:' '__')"
    [ -e "$dir/capture-stdin" ] && cat > "$dir/stdin-seen"
    count=$(( $(cat "$ref_dir/count" 2>/dev/null || echo 0) + 1 ))
    echo "$count" > "$ref_dir/count"
    if [ -e "$ref_dir/exit" ]; then
      limit=$(cat "$ref_dir/fail-times" 2>/dev/null || echo 999)
      if [ "$count" -le "$limit" ]; then
        cat "$ref_dir/stderr" >&2
        exit "$(cat "$ref_dir/exit")"
      fi
    fi
    cat "$ref_dir/value"
    echo
    ;;
  *) exit 64 ;;
esac
"#;

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for dir in ["bin", "op", "op/refs", "work", "empty-path"] {
            fs::create_dir_all(root.path().join(dir)).unwrap();
        }
        let op = root.path().join("bin/op");
        fs::write(&op, FAKE_OP).unwrap();
        fs::set_permissions(&op, fs::Permissions::from_mode(0o755)).unwrap();
        Self { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    fn ref_dir(&self, reference: &str) -> PathBuf {
        let dir = self
            .path("op/refs")
            .join(reference.replace(['/', ':'], "_"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn value(&self, reference: &str, value: &str) -> &Self {
        fs::write(self.ref_dir(reference).join("value"), value).unwrap();
        self
    }

    fn fail(&self, reference: &str, exit: i32, stderr: &str, times: Option<u32>) -> &Self {
        let dir = self.ref_dir(reference);
        fs::write(dir.join("exit"), exit.to_string()).unwrap();
        fs::write(dir.join("stderr"), stderr).unwrap();
        if let Some(times) = times {
            fs::write(dir.join("fail-times"), times.to_string()).unwrap();
        }
        self
    }

    fn reads(&self, reference: &str) -> u32 {
        fs::read_to_string(self.ref_dir(reference).join("count"))
            .map(|n| n.trim().parse().unwrap())
            .unwrap_or(0)
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.path("op/calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn refs(&self, json: &str) -> PathBuf {
        let path = self.path("work/refs.json");
        fs::write(&path, json).unwrap();
        path
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .current_dir(self.path("work"))
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env("FAKE_OP_DIR", self.path("op"))
            .env("OP_SERVICE_ACCOUNT_TOKEN", "fake-token-not-real");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn run_stdin(&self, args: &[&str], value: &[u8]) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // 出力先を拒む mode は stdin を読まずに終わるため、書き切る前に pipe が閉じうる。
        if let Err(error) = child.stdin.take().unwrap().write_all(value) {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "{error}");
        }
        child.wait_with_output().unwrap()
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_no_leak(output: &Output) {
    let all = text(output);
    assert!(!all.contains("sentinel"), "値か生の stderr が出た: {all}");
}

fn temp_leftovers(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".secret-tmp"))
        .collect()
}

#[test]
fn stdin_env_replaces_only_the_selected_key() {
    let fx = Fixture::new();
    let target = fx.path("work/.env");
    fs::write(&target, "# memo\nOTHER=keep\nTOKEN=old\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    let mut child = fx
        .command(&["--put-env", ".env", "TOKEN"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"sentinel-$HOME-new")
        .unwrap();

    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{}", text(&output));
    assert_no_leak(&output);
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "# memo\nOTHER=keep\nTOKEN='sentinel-\\$HOME-new'\n"
    );
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(fx.calls().is_empty());
}

#[test]
fn stdin_file_replaces_all_content_as_owner_only() {
    let fx = Fixture::new();
    fs::create_dir(fx.path("work/credentials")).unwrap();
    let target = fx.path("work/credentials/token.json");
    fs::write(&target, "old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();

    let output = fx.run_stdin(
        &["--put-file", "credentials/token.json"],
        b"{\"refresh_token\":\"sentinel-new\"}\n",
    );

    assert!(output.status.success(), "{}", text(&output));
    assert_no_leak(&output);
    assert_eq!(
        fs::read(&target).unwrap(),
        b"{\"refresh_token\":\"sentinel-new\"}\n"
    );
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(temp_leftovers(&fx.path("work/credentials")).is_empty());
    assert!(fx.calls().is_empty());
}

#[test]
fn stdin_modes_reject_outside_and_symlink_targets_without_leaking_values() {
    let fx = Fixture::new();
    let outside = fx.path("outside");
    fs::write(&outside, "untouched").unwrap();
    symlink(&outside, fx.path("work/link")).unwrap();
    symlink(fx.root.path(), fx.path("work/linked-dir")).unwrap();

    for args in [
        vec!["--put-file", "../outside"],
        vec!["--put-file", "link"],
        vec!["--put-file", "linked-dir/new"],
        vec!["--put-env", "link", "TOKEN"],
    ] {
        let output = fx.run_stdin(&args, b"sentinel-secret");
        assert_eq!(output.status.code(), Some(2), "{}", text(&output));
        assert_no_leak(&output);
    }
    assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched");
    assert!(!fx.path("new").exists());
}

/// `check-redacts-values` / `check-deterministic-output` / `check-read-only`:
/// key 順に ✓ を出し、値は出さず、何も書かない。
#[test]
fn check_lists_keys_in_order_without_values_or_writes() {
    let fx = Fixture::new();
    fx.value("op://v/b/f", "sentinel-b")
        .value("op://v/a/f", "sentinel-a");
    let refs = fx.refs(r#"{"B_KEY": "op://v/b/f", "A_KEY": "op://v/a/f"}"#);

    let output = fx.run(&["--refs", refs.to_str().unwrap(), "--check"]);

    assert!(output.status.success(), "{}", text(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(&lines[..2], ["✓ A_KEY", "✓ B_KEY"]);
    assert_no_leak(&output);
    let mut files: Vec<_> = fs::read_dir(fx.path("work"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    files.sort();
    assert_eq!(files, ["refs.json"]);
}

#[test]
fn refs_reject_derived_and_environment_literals_without_echoing_them() {
    for value in ["sentinel-derived-value", "sentinel-environment-value"] {
        let fx = Fixture::new();
        let refs = fx.refs(&format!(r#"{{"TOKEN":"{value}"}}"#));
        let output = fx
            .command(&["--refs", refs.to_str().unwrap(), "--check"])
            .env("TOKEN", value)
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(2), "{}", text(&output));
        assert!(fx.calls().is_empty(), "invalid refs must stop before op");
        assert!(!fx.path("work/.env").exists());
        assert!(
            !text(&output).contains(value),
            "literal leaked into diagnostics"
        );
    }
}

/// 全件そろえば既存の内容に merge して 0600 で書く。`$` を含む値も崩さない（§3.3-2）。
#[test]
fn write_merges_into_the_existing_file_as_owner_only() {
    let fx = Fixture::new();
    fx.value("op://v/token/f", "sentinel-$HOME-token")
        .value("op://v/new/f", "sentinel-new");
    let target = fx.path("work/.env");
    fs::write(&target, "# memo\nLOCAL=keep\nTOKEN=old\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    let refs = fx.refs(r#"{"TOKEN": "op://v/token/f", "NEW_KEY": "op://v/new/f"}"#);

    let output = fx.run(&["--refs", refs.to_str().unwrap(), "--write", ".env"]);

    assert!(output.status.success(), "{}", text(&output));
    assert_no_leak(&output);
    let written = fs::read_to_string(&target).unwrap();
    assert!(
        written.starts_with("# memo\nLOCAL=keep\nTOKEN='sentinel-\\$HOME-token'\n"),
        "{written}"
    );
    assert!(written.ends_with("NEW_KEY='sentinel-new'\n"), "{written}");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(temp_leftovers(&fx.path("work")).is_empty());
}

/// spec「1 件だけ取得に失敗する」/ `inject-no-write-on-read-failure`: 決定的な失敗は読み直さず、
/// 既存の `.env` の内容と mtime を変えずに非 0 で終わる。理由は固定カテゴリだけ（§3.2・§3.3-3）。
#[test]
fn one_deterministic_failure_writes_nothing_and_is_not_retried() {
    let fx = Fixture::new();
    fx.value("op://v/ok/f", "sentinel-ok").fail(
        "op://v/gone/f",
        1,
        "[ERROR] \"gone\" isn't an item in the \"v\" vault. sentinel-stderr",
        None,
    );
    let target = fx.path("work/.env");
    fs::write(&target, "EXISTING=1\n").unwrap();
    let before = fs::metadata(&target).unwrap().modified().unwrap();
    let refs = fx.refs(r#"{"OK": "op://v/ok/f", "GONE": "op://v/gone/f"}"#);

    let output = fx.run(&["--refs", refs.to_str().unwrap(), "--write", ".env"]);

    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert_no_leak(&output);
    assert!(
        text(&output).contains("✗ GONE — op://v/gone/f (参照先なし)"),
        "{}",
        text(&output)
    );
    assert_eq!(fx.reads("op://v/gone/f"), 1);
    assert_eq!(fs::read_to_string(&target).unwrap(), "EXISTING=1\n");
    assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), before);
    assert!(temp_leftovers(&fx.path("work")).is_empty());
}

/// `resolve-signin-short-circuit`: 最初の 1 件が exit 6 なら残りを読まない。
#[test]
fn a_first_exit_six_stops_before_the_rest() {
    let fx = Fixture::new();
    fx.fail("op://v/a/f", 6, "sentinel-stderr", None)
        .value("op://v/b/f", "sentinel-b")
        .value("op://v/c/f", "sentinel-c");
    let refs = fx.refs(r#"{"A": "op://v/a/f", "B": "op://v/b/f", "C": "op://v/c/f"}"#);

    let output = fx.run(&["--refs", refs.to_str().unwrap(), "--check"]);

    assert_eq!(output.status.code(), Some(1));
    assert_no_leak(&output);
    let reads: Vec<String> = fx
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("read"))
        .collect();
    assert_eq!(reads, ["read op://v/a/f"]);
    assert_eq!(text(&output).matches("(認証が必要)").count(), 3);
}

/// §3.3-5: `account` を含む文言でも exit 6 でなければ未認証にしない。一時失敗として読み直し、
/// 他の参照も読む（`resolve-transient-retry`）。
#[test]
fn account_wording_without_exit_six_is_retried_not_aborted() {
    let fx = Fixture::new();
    fx.value("op://v/first/f", "sentinel-first")
        .fail(
            "op://v/first/f",
            1,
            "[ERROR] request to your account failed. sentinel-stderr",
            Some(1),
        )
        .value("op://v/second/f", "sentinel-second");
    let refs = fx.refs(r#"{"FIRST": "op://v/first/f", "SECOND": "op://v/second/f"}"#);

    let output = fx.run(&["--refs", refs.to_str().unwrap(), "--write", ".env"]);

    assert!(output.status.success(), "{}", text(&output));
    assert_no_leak(&output);
    assert_eq!(fx.reads("op://v/first/f"), 2);
    assert_eq!(fx.reads("op://v/second/f"), 1);
    let written = fs::read_to_string(fx.path("work/.env")).unwrap();
    assert!(written.contains("FIRST='sentinel-first'"), "{written}");
}

/// `read-inherits-stdin`: `op read` は呼び出し元の stdin を受け取る（塞ぐと承認待ちに入れない）。
#[test]
fn op_read_inherits_stdin() {
    let fx = Fixture::new();
    fx.value("op://v/a/f", "sentinel-a");
    fs::write(fx.path("op/capture-stdin"), "").unwrap();
    let refs = fx.refs(r#"{"A": "op://v/a/f"}"#);

    let mut child = fx
        .command(&["--refs", refs.to_str().unwrap(), "--check"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"stdin-reached-op")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        fs::read_to_string(fx.path("op/stdin-seen")).unwrap(),
        "stdin-reached-op"
    );
}

/// spec「service account が無い」: token が無ければ op を 1 度も起動せずに止まる。
#[test]
fn a_missing_service_account_token_stops_before_op_runs() {
    let fx = Fixture::new();
    fx.value("op://v/a/f", "sentinel-a");
    let refs = fx.refs(r#"{"A": "op://v/a/f"}"#);

    let output = fx
        .command(&["--refs", refs.to_str().unwrap(), "--check"])
        .env_remove("OP_SERVICE_ACCOUNT_TOKEN")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output).contains("OP_SERVICE_ACCOUNT_TOKEN"),
        "{}",
        text(&output)
    );
    assert!(fx.calls().is_empty(), "{:?}", fx.calls());
}

/// spec「service account が無い」: 対象 vault を読めなければ、読み始める前に vault 名と理由で止まる。
#[test]
fn an_unreadable_vault_stops_before_any_read() {
    let fx = Fixture::new();
    fx.value("op://open/a/f", "sentinel-a")
        .value("op://closed/b/f", "sentinel-b");
    fs::write(fx.path("op/deny-vault-closed"), "").unwrap();
    let refs = fx.refs(r#"{"A": "op://open/a/f", "B": "op://closed/b/f"}"#);

    let output = fx.run(&["--refs", refs.to_str().unwrap(), "--write", ".env"]);

    assert_eq!(output.status.code(), Some(1));
    assert_no_leak(&output);
    assert!(
        text(&output).contains("vault \"closed\""),
        "{}",
        text(&output)
    );
    assert!(
        fx.calls().iter().all(|c| c.starts_with("vault get")),
        "{:?}",
        fx.calls()
    );
    assert!(!fx.path("work/.env").exists());
}

/// §3.3-1: env に同じ key の古い値があっても使わず、常に 1Password を読む（env の値を優先する旧実装との差）。
#[test]
fn a_stale_value_in_the_environment_is_ignored() {
    let fx = Fixture::new();
    fx.value("op://v/token/f", "sentinel-fresh");
    let refs = fx.refs(r#"{"TOKEN": "op://v/token/f"}"#);

    let output = fx
        .command(&["--refs", refs.to_str().unwrap(), "--write", ".env"])
        .env("TOKEN", "sentinel-stale")
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", text(&output));
    let written = fs::read_to_string(fx.path("work/.env")).unwrap();
    assert!(written.contains("TOKEN='sentinel-fresh'"), "{written}");
    assert_eq!(fx.reads("op://v/token/f"), 1);
    assert!(!fs::read_to_string(refs).unwrap().contains("sentinel"));
    assert_no_leak(&output);
}

/// `op` が無ければ事前確認の段階で止まる。
#[test]
fn a_missing_op_command_is_reported_as_such() {
    let fx = Fixture::new();
    let refs = fx.refs(r#"{"A": "op://v/a/f"}"#);

    let output = fx
        .command(&["--refs", refs.to_str().unwrap(), "--check"])
        .env("PATH", fx.path("empty-path"))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output).contains("opコマンドなし"),
        "{}",
        text(&output)
    );
}

/// 出力先が symlink なら、1Password を読む前に止まる。link の先は変えない。
#[test]
fn a_symlink_target_is_refused_before_reading() {
    let fx = Fixture::new();
    fx.value("op://v/a/f", "sentinel-a");
    fs::write(fx.path("victim"), "untouched\n").unwrap();
    symlink(fx.path("victim"), fx.path("work/.env")).unwrap();
    let refs = fx.refs(r#"{"A": "op://v/a/f"}"#);

    let output = fx.run(&["--refs", refs.to_str().unwrap(), "--write", ".env"]);

    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(fx.calls().is_empty(), "{:?}", fx.calls());
    assert_eq!(
        fs::read_to_string(fx.path("victim")).unwrap(),
        "untouched\n"
    );
}

/// `.env` に表せない値（CR を含む）は、崩して書かずに止まる。key は出すが値は出さない。
#[test]
fn an_unrepresentable_value_writes_nothing() {
    let fx = Fixture::new();
    fx.value("op://v/a/f", "sentinel-a\rtail");
    let refs = fx.refs(r#"{"A_KEY": "op://v/a/f"}"#);

    let output = fx.run(&["--refs", refs.to_str().unwrap(), "--write", ".env"]);

    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert_no_leak(&output);
    assert!(text(&output).contains("A_KEY"), "{}", text(&output));
    assert!(!fx.path("work/.env").exists());
    assert!(temp_leftovers(&fx.path("work")).is_empty());
}

/// 人の shell は env に token を持たない。`~/.config/op/service-account.token` が安全なら
/// それを読み、`op` の子プロセスにだけ渡す。
fn home_with_token(fx: &Fixture, mode: u32) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let home = fx.path("home");
    let dir = home.join(".config/op");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("service-account.token");
    fs::write(&file, "fake-token-from-file\n").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
    home
}

#[test]
fn a_private_token_file_is_used_when_the_env_has_none() {
    let fx = Fixture::new();
    fx.value("op://v/a/f", "sentinel-a");
    let refs = fx.refs(r#"{"A": "op://v/a/f"}"#);
    let home = home_with_token(&fx, 0o600);

    let output = fx
        .command(&["--refs", refs.to_str().unwrap(), "--check"])
        .env_remove("OP_SERVICE_ACCOUNT_TOKEN")
        .env("HOME", &home)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let seen = fs::read_to_string(fx.path("op/token-seen")).unwrap();
    assert!(
        !seen.is_empty() && seen.lines().all(|line| line == "token-set"),
        "op が token を受け取っていない: {seen:?}"
    );
    assert!(!text(&output).contains("fake-token-from-file"));
}

#[test]
fn a_token_file_readable_by_others_is_refused_before_op_runs() {
    let fx = Fixture::new();
    fx.value("op://v/a/f", "sentinel-a");
    let refs = fx.refs(r#"{"A": "op://v/a/f"}"#);
    let home = home_with_token(&fx, 0o644);

    let output = fx
        .command(&["--refs", refs.to_str().unwrap(), "--check"])
        .env_remove("OP_SERVICE_ACCOUNT_TOKEN")
        .env("HOME", &home)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output).contains("group か other"),
        "{}",
        text(&output)
    );
    assert!(fx.calls().is_empty(), "{:?}", fx.calls());
}

#[test]
fn a_symlinked_token_file_is_refused_before_op_runs() {
    let fx = Fixture::new();
    fx.value("op://v/a/f", "sentinel-a");
    let refs = fx.refs(r#"{"A": "op://v/a/f"}"#);
    let home = home_with_token(&fx, 0o600);
    let file = home.join(".config/op/service-account.token");
    let real = home.join(".config/op/real.token");
    fs::rename(&file, &real).unwrap();
    std::os::unix::fs::symlink(&real, &file).unwrap();

    let output = fx
        .command(&["--refs", refs.to_str().unwrap(), "--check"])
        .env_remove("OP_SERVICE_ACCOUNT_TOKEN")
        .env("HOME", &home)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output).contains("symlink"), "{}", text(&output));
    assert!(fx.calls().is_empty(), "{:?}", fx.calls());
}
