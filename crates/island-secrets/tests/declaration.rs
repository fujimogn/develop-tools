//! 宣言 mode の CLI 境界。PATH の先頭は偽 op、値は sentinel のみ。

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_develop-secrets");
const FAKE_OP: &str = r#"#!/bin/sh
dir="$FAKE_OP_DIR"
printf '%s\n' "$*" >> "$dir/calls"
case "$1" in
  vault) exit 0 ;;
  read)
    ref_dir="$dir/refs/$(printf '%s' "$2" | tr '/:' '__')"
    if [ -e "$ref_dir/fail" ]; then
      printf '%s\n' 'sentinel-private-stderr' >&2
      exit 4
    fi
    cat "$ref_dir/value"
    printf '\n'
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
        for dir in ["island/fixture", "bin", "op/refs", "home"] {
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

    fn work(&self, rel: &str) -> PathBuf {
        self.path("island/fixture").join(rel)
    }

    fn declaration(&self, text: &str) {
        fs::write(self.work(".develop-secrets.jsonc"), text).unwrap();
    }

    fn value(&self, reference: &str, value: &str) {
        let dir = self
            .path("op/refs")
            .join(reference.replace(['/', ':'], "_"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("value"), value).unwrap();
    }

    fn fail(&self, reference: &str) {
        let dir = self
            .path("op/refs")
            .join(reference.replace(['/', ':'], "_"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("fail"), "").unwrap();
    }

    fn run(&self, check: bool) -> Output {
        self.run_with(check, ".develop-secrets.jsonc", None)
    }

    fn run_with(&self, check: bool, declaration: &str, repo_root: Option<&str>) -> Output {
        let mut command = Command::new(BIN);
        command
            .arg("--declaration")
            .arg(declaration)
            .current_dir(self.path("island/fixture"))
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env("FAKE_OP_DIR", self.path("op"))
            .env("OP_SERVICE_ACCOUNT_TOKEN", "fake-token")
            .env("HOME", self.path("home"));
        if let Some(root) = repo_root {
            command.arg("--repo-root").arg(root);
        }
        if check {
            command.arg("--check");
        }
        command.output().unwrap()
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.path("op/calls")).unwrap_or_default()
    }
}

#[test]
fn nested_declaration_uses_explicit_repo_root_for_template_output_and_name() {
    let fx = Fixture::new();
    fs::create_dir(fx.work("config")).unwrap();
    fs::write(fx.work(".env.tpl"), "KEY={{ op://fixture/item/field }}\n").unwrap();
    fs::write(
        fx.work("config/develop-secrets.jsonc"),
        r#"{"version":1,"project":"fixture","targets":[{"name":"one","output":"~/.config/fixture/secret.env","template":".env.tpl"}]}"#,
    ).unwrap();
    fx.value("op://fixture/item/field", "sentinel-value");
    let output = fx.run_with(false, "config/develop-secrets.jsonc", Some("."));
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(fx.path("home/.config/fixture/secret.env").exists());
    assert_no_secret(&output);
}

#[test]
fn nested_declaration_uses_declared_project_for_vault_and_home_dir() {
    let fx = Fixture::new();
    fs::create_dir(fx.work("config")).unwrap();
    fs::write(
        fx.work("config/develop-secrets.jsonc"),
        r#"{"version":1,"project":"declared-vault","targets":[{"name":"one","output":"~/.config/declared-vault/secret.env","refs":{"A":"item/field"}}]}"#,
    ).unwrap();
    fx.value("op://declared-vault/item/field", "sentinel-value");
    let output = fx.run_with(false, "config/develop-secrets.jsonc", Some("."));
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(fx.calls().contains("op://declared-vault/item/field"));
    assert!(fx.path("home/.config/declared-vault/secret.env").exists());
    assert_no_secret(&output);

    let output = Command::new(BIN)
        .args([
            "--declaration",
            "config/develop-secrets.jsonc",
            "--repo-root",
            ".",
            "--check",
        ])
        .current_dir(fx.path("island/fixture"))
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", fx.path("bin").display()),
        )
        .env("FAKE_OP_DIR", fx.path("op"))
        .env("OP_SERVICE_ACCOUNT_TOKEN", "fake-token")
        .env("HOME", fx.path("home"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", output_text(&output));
    assert_no_secret(&output);
}

#[test]
fn invalid_project_stops_before_op_or_write() {
    for project in ["\"\"", "42", "null"] {
        let fx = Fixture::new();
        fx.declaration(&format!(
            r#"{{"version":1,"project":{project},"targets":[{{"name":"one","output":".env","refs":{{"A":"item/field"}}}}]}}"#
        ));
        let output = fx.run(false);
        assert_eq!(output.status.code(), Some(2), "{}", output_text(&output));
        assert!(fx.calls().is_empty());
        assert!(!fx.work(".env").exists());
    }
}

/// 台帳から名前を引く縮退は無い。project の無い宣言は op を呼ぶ前に失敗する。
#[test]
fn missing_project_fails_before_op_or_write() {
    let fx = Fixture::new();
    fx.declaration(
        r#"{"version":1,"targets":[{"name":"one","output":".env","refs":{"A":"item/field"}}]}"#,
    );
    fx.value("op://fixture/item/field", "sentinel-value");
    for check in [false, true] {
        let output = fx.run(check);
        assert_eq!(output.status.code(), Some(2), "{}", output_text(&output));
        assert!(
            output_text(&output).contains("missing-project"),
            "{}",
            output_text(&output)
        );
        assert!(fx.calls().is_empty(), "{}", fx.calls());
        assert!(!fx.work(".env").exists());
    }
}

#[test]
fn invalid_repo_root_or_declaration_outside_root_stops_before_op() {
    let fx = Fixture::new();
    fx.declaration(
        r#"{"version":1,"project":"fixture","targets":[{"name":"one","output":".env","refs":{"A":"item/field"}}]}"#,
    );
    fs::write(fx.work("file-root"), "not a directory").unwrap();
    for root in ["missing", "file-root", "config"] {
        fs::create_dir_all(fx.work("config")).unwrap();
        let output = fx.run_with(false, ".develop-secrets.jsonc", Some(root));
        assert_eq!(output.status.code(), Some(2), "{}", output_text(&output));
        assert!(fx.calls().is_empty());
        assert!(!fx.work(".env").exists());
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_no_secret(output: &Output) {
    let text = output_text(output);
    assert!(!text.contains("sentinel-value"), "{text}");
    assert!(!text.contains("sentinel-private-stderr"), "{text}");
}

fn two_targets(second_optional: bool) -> String {
    let optional = if second_optional {
        ",\"optional\":[\"SECOND\"]"
    } else {
        ""
    };
    format!(
        r#"{{"version":1,"project":"fixture","targets":[{{"name":"first","output":".env","refs":{{"FIRST":"one/f"}}}},{{"name":"second","output":".env2","refs":{{"SECOND":"two/f"}}{optional}}}]}}"#
    )
}

#[test]
fn seven_invalid_declarations_stop_before_op_or_write() {
    let cases = [
        r#"{"version":1,"project":"fixture","targets":[{"name":"a","output":".env","refs":{"A":"i/f"},"unexpected":"sentinel-value"}]}"#,
        r#"{"version":2,"project":"fixture","targets":[{"name":"a","output":".env","refs":{"A":"i/f"}}]}"#,
        r#"{"version":1,"project":"fixture","targets":[]}"#,
        r#"{"version":1,"project":"fixture","targets":[{"name":"a","output":".env"},{"name":"a","output":".env2"}]}"#,
        r#"{"version":1,"project":"fixture","targets":[{"name":"a","output":".env"},{"name":"b","output":".env"}]}"#,
        r#"{"version":1,"project":"fixture","targets":[{"name":"a","output":"../outside","refs":{"A":"i/f"}}]}"#,
        r#"{"version":1,"project":"fixture","targets":[{"name":"a","output":"/tmp/outside","refs":{"A":"i/f"}}]}"#,
        r#"{"version":1,"project":"fixture","targets":[{"name":"a","output":"~/.config/other/.env","refs":{"A":"i/f"}}]}"#,
        r#"{"version":1,"project":"fixture","targets":[{"name":"a","output":".env","template":".env.tpl","refs":{"A":"i/f"}}]}"#,
    ];
    for (index, declaration) in cases.iter().enumerate() {
        let fx = Fixture::new();
        fs::write(fx.work(".env.tpl"), "A={{ op://fixture/i/f }}\n").unwrap();
        fx.declaration(declaration);
        let output = fx.run(false);
        assert_ne!(
            output.status.code(),
            Some(0),
            "case {index}: {}",
            output_text(&output)
        );
        assert!(fx.calls().is_empty(), "case {index}: {}", fx.calls());
        assert!(!fx.work(".env").exists());
        assert_no_secret(&output);
    }
}

#[test]
fn template_interpolation_handles_spacing_and_explicit_vault() {
    let fx = Fixture::new();
    fx.declaration(
        r#"{"version":1,"project":"fixture","targets":[{"name":"template","output":".env","template":".env.tpl"}]}"#,
    );
    fs::write(
        fx.work(".env.tpl"),
        "A=left-{{op://other/item/field}}-{{ op://fixture/i/f }}\nB=literal\n",
    )
    .unwrap();
    fx.value("op://other/item/field", "sentinel-value");
    fx.value("op://fixture/i/f", "suffix");
    let output = fx.run(false);
    assert!(output.status.success(), "{}", output_text(&output));
    let env = fs::read_to_string(fx.work(".env")).unwrap();
    assert!(env.contains("left-sentinel-value-suffix"), "{env}");
    assert!(env.contains("B='literal'"), "{env}");
    assert_no_secret(&output);
}

#[test]
fn vault_omitted_op_uri_uses_declared_project() {
    let fx = Fixture::new();
    fx.declaration(
        r#"{"version":1,"project":"fixture","targets":[{"name":"one","output":".env","refs":{"A":"op://i/f"}}]}"#,
    );
    fx.value("op://fixture/i/f", "sentinel-value");
    let output = fx.run(false);
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(fx.calls().contains("op://fixture/i/f"));
    assert_no_secret(&output);
}

#[test]
fn symlinked_output_parent_is_rejected_before_op() {
    let fx = Fixture::new();
    symlink(fx.path("home"), fx.work("linked")).unwrap();
    fx.declaration(
        r#"{"version":1,"project":"fixture","targets":[{"name":"one","output":"linked/.env","refs":{"A":"i/f"}}]}"#,
    );
    let output = fx.run(false);
    assert_eq!(output.status.code(), Some(2), "{}", output_text(&output));
    assert!(fx.calls().is_empty());
    assert_no_secret(&output);
}

#[test]
fn symlinked_template_file_is_refused_before_reading() {
    let fx = Fixture::new();
    let outside = fx.path("outside.env.tpl");
    fs::write(&outside, "A={{ op://fixture/i/f }}\n").unwrap();
    symlink(&outside, fx.work(".env.tpl")).unwrap();
    fx.declaration(
        r#"{"version":1,"project":"fixture","targets":[{"name":"one","output":".env","template":".env.tpl"}]}"#,
    );
    fx.value("op://fixture/i/f", "sentinel-value");
    let output = fx.run(false);
    assert_eq!(output.status.code(), Some(2), "{}", output_text(&output));
    assert!(
        output_text(&output).contains("symlink"),
        "{}",
        output_text(&output)
    );
    assert!(fx.calls().is_empty(), "{}", fx.calls());
    assert!(!fx.work(".env").exists());
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "A={{ op://fixture/i/f }}\n"
    );
    assert_no_secret(&output);
}

#[test]
fn symlinked_template_directory_is_refused_before_reading() {
    let fx = Fixture::new();
    let outside = fx.path("outside-templates");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("env.tpl"), "A={{ op://fixture/i/f }}\n").unwrap();
    symlink(&outside, fx.work("templates")).unwrap();
    fx.declaration(
        r#"{"version":1,"project":"fixture","targets":[{"name":"one","output":".env","template":"templates/env.tpl"}]}"#,
    );
    fx.value("op://fixture/i/f", "sentinel-value");
    let output = fx.run(false);
    assert_eq!(output.status.code(), Some(2), "{}", output_text(&output));
    assert!(
        output_text(&output).contains("symlink"),
        "{}",
        output_text(&output)
    );
    assert!(fx.calls().is_empty(), "{}", fx.calls());
    assert!(!fx.work(".env").exists());
    assert_eq!(
        fs::read_to_string(outside.join("env.tpl")).unwrap(),
        "A={{ op://fixture/i/f }}\n"
    );
    assert_no_secret(&output);
}

#[test]
fn jsonc_accepts_comments_and_trailing_commas_but_not_json5_syntax() {
    let fx = Fixture::new();
    fx.value("op://fixture/i/f", "sentinel-value");
    fx.declaration(
        "{ // declaration\n\"version\":1,\"project\":\"fixture\",\"targets\":[{\"name\":\"a\",\"output\":\".env\",\"refs\":{\"A\":\"i/f\",},},],}",
    );
    let output = fx.run(true);
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(!fx.work(".env").exists());
    assert_no_secret(&output);

    fx.declaration("{version:1,targets:[]}");
    let invalid = fx.run(false);
    assert_ne!(invalid.status.code(), Some(0));
    assert_no_secret(&invalid);
}

#[test]
fn required_failure_leaves_every_target_unchanged() {
    let fx = Fixture::new();
    fx.declaration(&two_targets(false));
    fs::write(fx.work(".env"), "FIRST='old'\n").unwrap();
    fs::write(fx.work(".env2"), "SECOND='old'\n").unwrap();
    fx.value("op://fixture/one/f", "sentinel-value");
    fx.fail("op://fixture/two/f");
    let output = fx.run(false);
    assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
    assert_eq!(
        fs::read_to_string(fx.work(".env")).unwrap(),
        "FIRST='old'\n"
    );
    assert_eq!(
        fs::read_to_string(fx.work(".env2")).unwrap(),
        "SECOND='old'\n"
    );
    assert_no_secret(&output);
}

#[test]
fn optional_failure_preserves_existing_value_and_writes_required() {
    let fx = Fixture::new();
    fx.declaration(&two_targets(true));
    fs::write(fx.work(".env2"), "SECOND='old'\n").unwrap();
    fx.value("op://fixture/one/f", "sentinel-value");
    fx.fail("op://fixture/two/f");
    let output = fx.run(false);
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(
        fs::read_to_string(fx.work(".env"))
            .unwrap()
            .contains("FIRST='sentinel-value'")
    );
    assert_eq!(
        fs::read_to_string(fx.work(".env2")).unwrap(),
        "SECOND='old'\n"
    );
    assert!(output_text(&output).contains("SECOND"));
    assert!(
        output_text(&output).contains("optional key second:SECOND を省き、既存値を残します"),
        "optional failure must warn"
    );
    assert_no_secret(&output);
}

#[test]
fn home_output_is_private_and_confined_to_repo_config() {
    let fx = Fixture::new();
    fx.declaration(r#"{"version":1,"project":"fixture","targets":[{"name":"home","output":"~/.config/fixture/.env","refs":{"A":"i/f"}}]}"#);
    fx.value("op://fixture/i/f", "sentinel-value");
    let output = fx.run(false);
    assert!(output.status.success(), "{}", output_text(&output));
    let parent = fx.path("home/.config/fixture");
    let file = parent.join(".env");
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains("sentinel-value")
    );
    assert_eq!(mode(&parent), 0o700);
    assert_eq!(mode(&file), 0o600);
    assert_no_secret(&output);
}

#[test]
fn home_output_preserves_shared_config_mode_and_allows_its_symlink() {
    let fx = Fixture::new();
    let shared = fx.path("home/shared-config");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(&shared, fx.path("home/.config")).unwrap();
    fx.declaration(r#"{"version":1,"project":"fixture","targets":[{"name":"home","output":"~/.config/fixture/.env","refs":{"A":"i/f"}}]}"#);
    fx.value("op://fixture/i/f", "sentinel-value");
    let output = fx.run(false);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(mode(&shared), 0o755);
    assert_eq!(mode(&shared.join("fixture")), 0o700);
    assert_eq!(mode(&shared.join("fixture/.env")), 0o600);
    assert_no_secret(&output);
}

#[test]
fn linked_worktree_runs_from_its_own_root() {
    let fx = Fixture::new();
    let repo = fx.path("island/fixture");
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "init",
        ],
    ] {
        let output = develop_tools_core::git(&repo).args(args).output().unwrap();
        assert!(output.status.success(), "{}", output_text(&output));
    }
    let linked = fx.path("island/linked");
    let output = develop_tools_core::git(&repo)
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            linked.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", output_text(&output));
    fs::write(
        linked.join(".develop-secrets.jsonc"),
        r#"{"version":1,"project":"fixture","targets":[{"name":"one","output":".env","refs":{"A":"i/f"}}]}"#,
    )
    .unwrap();
    fx.value("op://fixture/i/f", "sentinel-value");
    let output = Command::new(BIN)
        .args(["--declaration", ".develop-secrets.jsonc"])
        .current_dir(&linked)
        .env_clear()
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", fx.path("bin").display()),
        )
        .env("FAKE_OP_DIR", fx.path("op"))
        .env("OP_SERVICE_ACCOUNT_TOKEN", "fake-token")
        .env("HOME", fx.path("home"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(linked.join(".env").exists());
    assert_no_secret(&output);
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn second_write_error_reports_written_and_unwritten_targets_without_rollback() {
    let fx = Fixture::new();
    fx.declaration(r#"{"version":1,"project":"fixture","targets":[{"name":"first","output":".env","refs":{"FIRST":"one/f"}},{"name":"second","output":"missing/.env","refs":{"SECOND":"two/f"}}]}"#);
    fx.value("op://fixture/one/f", "sentinel-value");
    fx.value("op://fixture/two/f", "sentinel-value");
    let output = fx.run(false);
    assert_eq!(output.status.code(), Some(2), "{}", output_text(&output));
    assert!(
        fs::read_to_string(fx.work(".env"))
            .unwrap()
            .contains("sentinel-value")
    );
    assert!(!fx.work("missing/.env").exists());
    let report = output_text(&output);
    assert!(
        report.contains("first") && report.contains("second"),
        "{report}"
    );
    assert_no_secret(&output);
}
