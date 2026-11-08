use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    process::{Command, ExitStatus},
};

use ci_change_scope::dependency::check;

fn output_retrying_text_file_busy(command: &mut Command) -> std::io::Result<std::process::Output> {
    for attempt in 0..20 {
        match command.output() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempt < 19 => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => return other,
        }
    }
    unreachable!("final attempt always returns")
}

fn git(root: &Path, args: &[&str]) -> ExitStatus {
    let mut command = develop_tools_core::git(root);
    command.args(args);
    let output = output_retrying_text_file_busy(&mut command).unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output.status
}

fn fixture(script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::create_dir_all(root.join(".config")).unwrap();
    fs::write(
        root.join(".config/develop-tools.json"),
        r#"{"ci":{"recordOnlySkip":true,"bun":{}}}"#,
    )
    .unwrap();
    fs::write(
        root.join("package.json"),
        r#"{"scripts":{"test":"bun test.ts"}}"#,
    )
    .unwrap();
    fs::write(root.join("test.ts"), script).unwrap();
    fs::write(
        root.join(".github/workflows/ci-develop.yml"),
        r#"
jobs:
  common:
    steps:
      - run: develop-docs-check --tracked
  scope:
    steps:
      - run: ci-change-scope
  bun:
    steps:
      - run: bun run test
  custom:
    steps:
      - run: echo checked
"#,
    )
    .unwrap();
    assert!(git(root, &["init", "-q"]).success());
    assert!(git(root, &["add", "-A"]).success());
    dir
}

fn covered(entries: &[&str]) -> BTreeSet<String> {
    entries.iter().map(|entry| (*entry).to_owned()).collect()
}

#[test]
fn derives_entries_independently_and_accepts_a_closed_fixture() {
    let repo = fixture("console.log('ok');\n");
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    assert_eq!(check(repo.path(), &expected).unwrap(), expected);
}

#[test]
fn missing_adapter_coverage_and_new_job_are_heavy() {
    let repo = fixture("console.log('ok');\n");
    assert!(
        check(
            repo.path(),
            &covered(&["bun:0", "package:package.json:test", "source:test.ts"])
        )
        .is_err()
    );
    fs::write(
        repo.path().join(".github/workflows/extra.yml"),
        "jobs:\n  surprise:\n    steps:\n      - run: echo new\n",
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    assert!(check(repo.path(), &expected).is_err());

    let repo = fixture("console.log('ok');\n");
    fs::write(
        repo.path().join("package.json"),
        r#"{"scripts":{"test":"bun test.ts","new-check":"echo added"}}"#,
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(check(repo.path(), &expected).is_err());
}

#[test]
fn document_reads_existence_globs_and_dynamic_paths_are_heavy() {
    for script in [
        "readFileSync('TODO.md');",
        "existsSync('CHANGELOG.md');",
        "glob('docs/reviews/*.md');",
        "readFileSync(path);",
        "const p = 'TODO.md'; readFileSync(p);",
    ] {
        let repo = fixture(script);
        let expected = covered(&[
            "bun:0",
            "custom:0",
            "package:package.json:test",
            "source:test.ts",
        ]);
        assert!(check(repo.path(), &expected).is_err(), "{script}");
    }
}

#[test]
fn unknown_entry_and_untracked_input_are_heavy() {
    let repo = fixture("console.log('ok');\n");
    fs::write(
        repo.path().join(".github/workflows/ci-develop.yml"),
        "jobs:\n  bun:\n    steps:\n      - run: unrecognized-check\n",
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(check(repo.path(), &covered(&["bun:0"])).is_err());

    let repo = fixture("console.log('ok');\n");
    fs::write(repo.path().join("test.ts"), "console.log('modified')").unwrap();
    assert!(
        check(
            repo.path(),
            &covered(&[
                "bun:0",
                "custom:0",
                "package:package.json:test",
                "source:test.ts"
            ])
        )
        .is_err()
    );
}

#[test]
fn checked_paths_cannot_escape_repo() {
    let repo = fixture("console.log('ok');\n");
    fs::write(
        repo.path().join("package.json"),
        r#"{"scripts":{"test":"bun ../outside.ts"}}"#,
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(check(repo.path(), &BTreeSet::new()).is_err());
}

#[test]
fn declared_bun_without_generated_test_is_heavy() {
    let repo = fixture("console.log('ok');\n");
    fs::write(repo.path().join(".github/workflows/ci-develop.yml"), "jobs:\n  common:\n    steps:\n      - run: develop-docs-check --tracked\n  scope:\n    steps:\n      - run: ci-change-scope\n  bun:\n    steps:\n      - run: echo no-test\n").unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(
        check(
            repo.path(),
            &covered(&["bun:0", "package:package.json:test", "source:test.ts"])
        )
        .is_err()
    );
}

#[test]
fn new_cargo_target_and_uncovered_nested_script_are_heavy() {
    let repo = fixture("console.log('ok');\n");
    fs::write(
        repo.path().join("Cargo.toml"),
        "[package]\nname='fixture'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::create_dir_all(repo.path().join("src")).unwrap();
    fs::write(
        repo.path().join("src/lib.rs"),
        "pub fn check() { let _ = std::fs::read_to_string(\"TODO.md\"); }\n",
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    assert!(check(repo.path(), &expected).is_err());

    let repo = fixture("console.log('ok');\n");
    fs::create_dir_all(repo.path().join("sub")).unwrap();
    fs::write(
        repo.path().join("sub/package.json"),
        r#"{"scripts":{"test":"echo nested"}}"#,
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(check(repo.path(), &expected).is_err());
}

#[test]
fn dependency_cli_exits_nonzero_for_unproven_inputs() {
    let repo = fixture("console.log('ok');\n");
    let coverage = repo.path().join("coverage.json");
    fs::write(
        &coverage,
        r#"["bun:0","custom:0","package:package.json:test","source:test.ts"]"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ci-change-scope"))
        .current_dir(repo.path())
        .env("GIT_DIR", "/definitely/not/a/repository")
        .arg("--check-dependencies")
        .arg(&coverage)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(repo.path().join("test.ts"), "readFileSync('TODO.md');").unwrap();
    git(repo.path(), &["add", "-A"]);
    let output = Command::new(env!("CARGO_BIN_EXE_ci-change-scope"))
        .current_dir(repo.path())
        .arg("--check-dependencies")
        .arg(&coverage)
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn workflow_and_job_directory_defaults_cannot_hide_a_document_read() {
    for defaults in ["workflow", "job"] {
        let repo = fixture("console.log('ok');\n");
        fs::create_dir_all(repo.path().join("sub")).unwrap();
        fs::write(repo.path().join("check.ts"), "console.log('ok');\n").unwrap();
        fs::write(
            repo.path().join("sub/check.ts"),
            "readFileSync('TODO.md');\n",
        )
        .unwrap();
        let workflow = if defaults == "workflow" {
            "defaults:\n  run:\n    working-directory: sub\njobs:\n  common:\n    steps:\n      - run: develop-docs-check --tracked\n  scope:\n    steps:\n      - run: ci-change-scope\n  bun:\n    steps:\n      - run: bun run test\n        working-directory: .\n      - run: bun check.ts\n  custom:\n    steps:\n      - run: echo checked\n"
        } else {
            "jobs:\n  common:\n    steps:\n      - run: develop-docs-check --tracked\n  scope:\n    steps:\n      - run: ci-change-scope\n  bun:\n    defaults:\n      run:\n        working-directory: sub\n    steps:\n      - run: bun run test\n        working-directory: .\n      - run: bun check.ts\n  custom:\n    steps:\n      - run: echo checked\n"
        };
        fs::write(
            repo.path().join(".github/workflows/ci-develop.yml"),
            workflow,
        )
        .unwrap();
        git(repo.path(), &["add", "-A"]);
        assert!(
            check(
                repo.path(),
                &covered(&[
                    "bun:0",
                    "bun:1",
                    "custom:0",
                    "package:package.json:test",
                    "source:test.ts",
                    "source:check.ts"
                ])
            )
            .is_err(),
            "{defaults}"
        );
    }
}

#[test]
fn unknown_cli_argument_cannot_fall_through_to_successful_classification() {
    let output = Command::new(env!("CARGO_BIN_EXE_ci-change-scope"))
        .arg("--check-dependancies")
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn custom_cargo_targets_cannot_escape_the_inventory() {
    for (manifest, target) in [
        (
            "[package]\nname='fixture'\nversion='0.1.0'\n[lib]\npath='odd.rs'\n",
            "odd.rs",
        ),
        (
            "[package]\nname='fixture'\nversion='0.1.0'\nbuild='build-custom.rs'\n",
            "build-custom.rs",
        ),
        (
            "[package]\nname='fixture'\nversion='0.1.0'\n",
            "examples/example.rs",
        ),
        (
            "[package]\nname='fixture'\nversion='0.1.0'\n",
            "benches/bench.rs",
        ),
    ] {
        let repo = fixture("console.log('ok');\n");
        fs::write(repo.path().join("Cargo.toml"), manifest).unwrap();
        if let Some(parent) = repo.path().join(target).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(repo.path().join(target), "console.log('ok');\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        let result = check(
            repo.path(),
            &covered(&[
                "bun:0",
                "custom:0",
                "package:package.json:test",
                "source:test.ts",
            ]),
        );
        assert!(result.is_err(), "{target}: {result:?}");
    }
}

#[test]
fn runtime_configuration_and_step_environment_are_heavy() {
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    let repo = fixture("console.log('ok');\n");
    fs::write(
        repo.path().join("bunfig.toml"),
        "preload = ['./preload.ts']\n",
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(check(repo.path(), &expected).is_err());

    let repo = fixture("console.log('ok');\n");
    let workflow =
        fs::read_to_string(repo.path().join(".github/workflows/ci-develop.yml")).unwrap();
    fs::write(
        repo.path().join(".github/workflows/ci-develop.yml"),
        workflow.replace(
            "      - run: echo checked",
            "      - run: echo checked\n        env:\n          BUN_OPTIONS: --preload=preload.ts",
        ),
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(check(repo.path(), &expected).is_err());
}

#[test]
fn non_string_working_directory_is_unknown_not_root() {
    let repo = fixture("console.log('ok');\n");
    let workflow =
        fs::read_to_string(repo.path().join(".github/workflows/ci-develop.yml")).unwrap();
    fs::write(
        repo.path().join(".github/workflows/ci-develop.yml"),
        workflow.replace(
            "      - run: bun run test",
            "      - run: bun run test\n        working-directory: 1",
        ),
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    assert!(check(repo.path(), &expected).is_err());
}

#[test]
fn malformed_source_returns_error_without_panicking() {
    let repo = fixture("console.log(');\n");
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    assert!(check(repo.path(), &expected).is_err());
}

#[test]
fn job_container_preload_cannot_hide_a_document_read() {
    let repo = fixture("console.log('ok');\n");
    let workflow =
        fs::read_to_string(repo.path().join(".github/workflows/ci-develop.yml")).unwrap();
    fs::write(repo.path().join(".github/workflows/ci-develop.yml"), workflow.replace("  bun:\n    steps:", "  bun:\n    container:\n      image: oven/bun:1\n      env:\n        BUN_OPTIONS: --preload=./reader.ts\n    steps:")).unwrap();
    fs::write(repo.path().join("reader.ts"), "readFileSync('TODO.md');\n").unwrap();
    git(repo.path(), &["add", "-A"]);
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    assert!(check(repo.path(), &expected).is_err());
}

#[test]
fn unsupported_execution_settings_are_heavy() {
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    for addition in [
        "    container: oven/bun:1\n",
        "    services:\n      cache:\n        image: redis:7\n",
        "    defaults:\n      run:\n        shell: bash\n",
        "    env:\n      UNRECOGNIZED_FLAG: enabled\n",
    ] {
        let repo = fixture("console.log('ok');\n");
        let workflow =
            fs::read_to_string(repo.path().join(".github/workflows/ci-develop.yml")).unwrap();
        fs::write(
            repo.path().join(".github/workflows/ci-develop.yml"),
            workflow.replace("  bun:\n", &format!("  bun:\n{addition}")),
        )
        .unwrap();
        git(repo.path(), &["add", "-A"]);
        assert!(check(repo.path(), &expected).is_err(), "{addition}");
    }
    let repo = fixture("console.log('ok');\n");
    let workflow =
        fs::read_to_string(repo.path().join(".github/workflows/ci-develop.yml")).unwrap();
    fs::write(
        repo.path().join(".github/workflows/ci-develop.yml"),
        workflow.replace(
            "      - run: bun run test",
            "      - run: bun run test\n        shell: bash",
        ),
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(check(repo.path(), &expected).is_err());
}

#[test]
fn supported_keys_reject_unsupported_value_shapes() {
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    for (before, after) in [
        ("jobs:\n", "name: 123\njobs:\n"),
        ("  bun:\n", "  bun:\n    runs-on: [self-hosted]\n"),
        (
            "      - run: bun run test",
            "      - run: bun run test\n        if: true",
        ),
    ] {
        let repo = fixture("console.log('ok');\n");
        let workflow =
            fs::read_to_string(repo.path().join(".github/workflows/ci-develop.yml")).unwrap();
        fs::write(
            repo.path().join(".github/workflows/ci-develop.yml"),
            workflow.replacen(before, after, 1),
        )
        .unwrap();
        git(repo.path(), &["add", "-A"]);
        assert!(check(repo.path(), &expected).is_err(), "{after}");
    }
}

#[test]
fn supported_workflow_job_and_step_shapes_remain_provable() {
    let repo = fixture("console.log('ok');\n");
    fs::write(
        repo.path().join(".github/workflows/ci-develop.yml"),
        r#"
name: CI
on: [push]
permissions:
  contents: read
concurrency:
  group: ci
  cancel-in-progress: false
jobs:
  common:
    runs-on: ubuntu-latest
    steps:
      - run: develop-docs-check --tracked
  scope:
    runs-on: ubuntu-latest
    outputs:
      light: 'false'
    steps:
      - run: ci-change-scope
  bun:
    name: bun
    runs-on: ubuntu-latest
    needs: [scope]
    if: always()
    timeout-minutes: 10
    steps:
      - name: test
        id: test
        if: always()
        run: bun run test
        continue-on-error: false
  custom:
    runs-on: ubuntu-latest
    steps:
      - run: echo checked
"#,
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    assert_eq!(check(repo.path(), &expected).unwrap(), expected);
}

#[test]
fn tracked_dot_env_files_cannot_inject_a_preload() {
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    for path in [".env", ".env.local", "sub/.env.production"] {
        let repo = fixture("console.log('ok');\n");
        if let Some(parent) = repo.path().join(path).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            repo.path().join(path),
            "BUN_OPTIONS=--preload=./reader.ts\n",
        )
        .unwrap();
        fs::write(repo.path().join("reader.ts"), "readFileSync('TODO.md');\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["add", "-f", path]);
        assert!(check(repo.path(), &expected).is_err(), "{path}");
    }
}

#[test]
fn unsupported_tracked_execution_configs_and_lifecycle_scripts_are_heavy() {
    let expected = covered(&[
        "bun:0",
        "custom:0",
        "package:package.json:test",
        "source:test.ts",
    ]);
    for path in [
        ".npmrc",
        ".cargo/config.toml",
        "rust-toolchain.toml",
        "sub/tsconfig.json",
    ] {
        let repo = fixture("console.log('ok');\n");
        if let Some(parent) = repo.path().join(path).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(repo.path().join(path), "unresolved runtime config\n").unwrap();
        git(repo.path(), &["add", "-A"]);
        git(repo.path(), &["add", "-f", path]);
        assert!(check(repo.path(), &expected).is_err(), "{path}");
    }

    let repo = fixture("console.log('ok');\n");
    fs::write(repo.path().join("reader.ts"), "readFileSync('TODO.md');\n").unwrap();
    fs::write(
        repo.path().join("package.json"),
        r#"{"scripts":{"test":"bun test.ts","pretest":"bun reader.ts"}}"#,
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(check(repo.path(), &expected).is_err());

    let repo = fixture("console.log('ok');\n");
    fs::write(
        repo.path().join("package.json"),
        r#"{"scripts":{"test":"bun test.ts"},"type":[]}"#,
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    assert!(check(repo.path(), &expected).is_err());
}

#[test]
fn tracked_node_modules_binary_cannot_replace_an_allowed_command() {
    let repo = fixture("console.log('ok');\n");
    fs::create_dir_all(repo.path().join("node_modules/.bin")).unwrap();
    fs::write(
        repo.path().join("node_modules/.bin/rustup"),
        "#!/bin/sh\ncat TODO.md\n",
    )
    .unwrap();
    fs::write(
        repo.path().join("package.json"),
        r#"{"scripts":{"test":"rustup show"}}"#,
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["add", "-f", "node_modules/.bin/rustup"]);
    assert!(
        check(
            repo.path(),
            &covered(&["bun:0", "custom:0", "package:package.json:test"])
        )
        .is_err()
    );
}
