//! `develop-secrets` — 開発時の `.env` 生成の核の入口。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use develop_tools_core::cli_help::Help;
use island_secrets::op::{Failure, OpCli};
use island_secrets::refs::{self, Entry};
use island_secrets::resolve::{self, Options, Resolved};
use island_secrets::{Secret, dotenv, preflight, write};
use island_secrets::{declaration, multi_write};

const NAME: &str = "develop-secrets";

fn help() -> String {
    Help::new(
        "develop-secrets (--refs <file.json> (--check | --write <path>) | --declaration <file.jsonc> [--repo-root <dir>] [--check] | --put-env <repo-relative-path> <key> | --put-file <repo-relative-path>) [--concurrency <n>] [--help]",
        "1Password の参照または stdin の値を受け、開発用 secret file を作ります。値は画面に出しません。",
        "--refs の JSON（{\"KEY\": \"op://vault/item/field\", ...}）または --declaration の JSONC を読み、service account の token（OP_SERVICE_ACCOUNT_TOKEN、無ければ ~/.config/op/service-account.token）と各 vault を確かめてから op read します。最初の 1 件は直列（未認証ならそこで打ち切り）、残りは並列（既定 6）、一時失敗は間隔を空けて 2 回まで読み直します。--refs の --check は key ごとに表示します。宣言 mode は既定で全 target を書き、--check は取得のみ確かめます。環境変数に同じ key があっても使いません（常に 1Password を読みます）。",
        "--check はなし。書込は各 target と同じ directory の一時 file（.<name>.<乱数>.secret-tmp）と target file。stdin mode は cwd を repo root とし、repo 相対の出力先だけ受けます。",
    )
    .exit("0 = 全件取得（--write は書き込み済み）、1 = 事前確認か取得の失敗（何も書かない）、2 = 引数・入力・file の読み書きのエラー（rename 後の directory fsync 失敗は書込済みだが永続化未確認）")
    .examples(&[
        "develop-secrets --refs secrets.json --check",
        "develop-secrets --refs secrets.json --write .env",
        "develop-secrets --declaration .config/develop-secrets.jsonc --repo-root .",
        r#"printf '%s' "$TOKEN" | develop-secrets --put-env .env TOKEN"#,
        "cat token.json | develop-secrets --put-file credentials/refresh-token.json",
    ])
    .render()
}

enum Mode {
    Check,
    Write(PathBuf),
    PutEnv(PathBuf, String),
    PutFile(PathBuf),
}

struct Args {
    refs: Option<PathBuf>,
    declaration: Option<PathBuf>,
    repo_root: Option<PathBuf>,
    mode: Mode,
    concurrency: usize,
}

enum Parsed {
    Help,
    Run(Args),
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
    let mut refs = None;
    let mut declaration = None;
    let mut repo_root = None;
    let mut mode = None;
    let mut concurrency = resolve::DEFAULT_CONCURRENCY;
    let mut args = args.into_iter();
    let value_of = |flag: &str, args: &mut dyn Iterator<Item = String>| {
        args.next()
            .filter(|v| !v.is_empty() && !v.starts_with("--"))
            .ok_or(format!("{flag} に値が必要です"))
    };
    while let Some(arg) = args.next() {
        let next_mode = match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--refs" => {
                refs = Some(PathBuf::from(value_of("--refs", &mut args)?));
                continue;
            }
            "--declaration" => {
                declaration = Some(PathBuf::from(value_of("--declaration", &mut args)?));
                continue;
            }
            "--repo-root" => {
                repo_root = Some(PathBuf::from(value_of("--repo-root", &mut args)?));
                continue;
            }
            "--concurrency" => {
                concurrency = value_of("--concurrency", &mut args)?
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--concurrency は 1 以上の整数にしてください")?;
                continue;
            }
            "--check" => Mode::Check,
            "--write" => Mode::Write(PathBuf::from(value_of("--write", &mut args)?)),
            "--put-env" => {
                let path = PathBuf::from(value_of("--put-env", &mut args)?);
                let key = value_of("--put-env", &mut args)?;
                if !refs::is_env_key(&key) {
                    return Err("--put-env の key が不正です".to_owned());
                }
                Mode::PutEnv(path, key)
            }
            "--put-file" => Mode::PutFile(PathBuf::from(value_of("--put-file", &mut args)?)),
            other => return Err(format!("不明な引数です: {other}")),
        };
        if mode.replace(next_mode).is_some() {
            return Err("書込または check の mode は 1 回だけ指定してください".to_owned());
        }
    }
    if matches!(mode, Some(Mode::PutEnv(..) | Mode::PutFile(..))) {
        if refs.is_some()
            || declaration.is_some()
            || repo_root.is_some()
            || concurrency != resolve::DEFAULT_CONCURRENCY
        {
            return Err(
                "stdin mode に refs、declaration、その他の option は指定しません".to_owned(),
            );
        }
        return Ok(Parsed::Run(Args {
            refs,
            declaration,
            repo_root,
            mode: mode.expect("stdin mode"),
            concurrency,
        }));
    }
    if refs.is_some() == declaration.is_some() {
        return Err("--refs と --declaration はどちらか一方を指定してください".to_owned());
    }
    if declaration.is_some() && matches!(mode, Some(Mode::Write(_))) {
        return Err("宣言 mode では --write を指定しません".to_owned());
    }
    if refs.is_some() && repo_root.is_some() {
        return Err("--repo-root は宣言 mode 専用です".to_owned());
    }
    let mode = if declaration.is_some() {
        mode.unwrap_or(Mode::Write(PathBuf::new()))
    } else {
        mode.ok_or("--check か --write <path> を指定してください")?
    };
    Ok(Parsed::Run(Args {
        refs,
        declaration,
        repo_root,
        mode,
        concurrency,
    }))
}

/// 失敗の出口。どれも値を含まない。
enum Stop {
    /// 事前確認か取得の失敗（exit 1）。
    Fetch(String),
    /// 引数・入力・file の読み書き（exit 2）。
    Usage(String),
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(Parsed::Help) => {
            print!("{}", help());
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Run(args)) => args,
        Err(message) => {
            eprintln!("{NAME}: {message}");
            eprintln!("{NAME}: 使い方は --help を見てください");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stop::Fetch(message)) => {
            eprintln!("{NAME}: {message}");
            ExitCode::FAILURE
        }
        Err(Stop::Usage(message)) => {
            eprintln!("{NAME}: {message}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &Args) -> Result<(), Stop> {
    match &args.mode {
        Mode::PutEnv(path, key) => return run_stdin(path, Some(key)),
        Mode::PutFile(path) => return run_stdin(path, None),
        _ => {}
    }
    if let Some(path) = &args.declaration {
        return run_declaration(args, path);
    }
    let refs_path = args
        .refs
        .as_ref()
        .expect("parse requires refs or declaration");
    let text = std::fs::read_to_string(refs_path)
        .map_err(|error| Stop::Usage(format!("{}: {error}", refs_path.display())))?;
    let entries = refs::parse_json(&text).map_err(Stop::Usage)?;
    if let Mode::Write(target) = &args.mode {
        // 読み始める前に弾く。1Password を読んだ後で書けないと分かるのは無駄。
        write::ensure_writable_target(target).map_err(|e| Stop::Usage(e.to_string()))?;
    }

    let env_token = std::env::var(preflight::TOKEN_ENV).ok();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let token = preflight::locate_token(env_token.as_deref(), home.as_deref())
        .map_err(|error| Stop::Fetch(error.to_string()))?;
    let client = OpCli::with_token(token.into_child_secret());
    let mut vaults: Vec<&str> = entries
        .iter()
        .filter_map(|e| refs::vault_of(&e.reference))
        .collect();
    vaults.sort_unstable();
    vaults.dedup();
    preflight::check(true, &client, &vaults).map_err(|error| Stop::Fetch(error.to_string()))?;

    let references: Vec<&str> = entries.iter().map(|e| e.reference.as_str()).collect();
    let resolved = resolve::resolve(
        &client,
        &references,
        &Options {
            concurrency: args.concurrency,
            ..Options::default()
        },
    );

    match &args.mode {
        Mode::Check => check(&entries, &resolved),
        Mode::Write(target) => write_env(target, &entries, resolved),
        Mode::PutEnv(..) | Mode::PutFile(..) => unreachable!("handled before refs"),
    }
}

fn run_stdin(path: &Path, key: Option<&String>) -> Result<(), Stop> {
    let repo =
        std::env::current_dir().map_err(|_| Stop::Usage("repo root を読めません".to_owned()))?;
    let target =
        declaration::repo_output_path(&repo, &path.to_string_lossy()).map_err(Stop::Usage)?;
    write::ensure_writable_target(&target).map_err(write_error_stop)?;
    let mut value = String::new();
    std::io::stdin()
        .read_to_string(&mut value)
        .map_err(|_| Stop::Usage("stdin を読めません".to_owned()))?;
    let content = if let Some(key) = key {
        let existing = write::read_existing(&target).map_err(write_error_stop)?;
        dotenv::merge(existing.expose(), &[(key.clone(), Secret::new(value))])
            .map_err(|error| Stop::Usage(error.to_string()))?
    } else {
        Secret::new(value)
    };
    write::write_secret_file(&target, &content).map_err(write_error_stop)?;
    println!("{NAME}: {} を書きました（0600）", path.display());
    Ok(())
}

fn run_declaration(args: &Args, path: &std::path::Path) -> Result<(), Stop> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Stop::Usage("HOME がありません".to_owned()))?;
    let declaration =
        declaration::load(path, args.repo_root.as_deref(), &home).map_err(Stop::Usage)?;
    let env_token = std::env::var(preflight::TOKEN_ENV).ok();
    let token = preflight::locate_token(env_token.as_deref(), Some(&home))
        .map_err(|error| Stop::Fetch(error.to_string()))?;
    let client = OpCli::with_token(token.into_child_secret());
    multi_write::run(
        &declaration,
        &client,
        matches!(args.mode, Mode::Check),
        args.concurrency,
    )
    .map_err(|error| match error {
        multi_write::Error::Fetch(message) => Stop::Fetch(message),
        multi_write::Error::Io(message) => Stop::Usage(message),
    })
}

/// key 順の 1 行ずつ。値は出さない。
fn report_lines(entries: &[Entry], resolved: &Resolved) -> (Vec<String>, usize) {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    let mut failed = 0;
    let lines = sorted
        .into_iter()
        .map(|entry| match resolved.failures.get(&entry.reference) {
            None => format!("✓ {}", entry.key),
            Some(failure) => {
                failed += 1;
                format!(
                    "✗ {} — {} ({})",
                    entry.key,
                    entry.reference,
                    failure.label()
                )
            }
        })
        .collect();
    (lines, failed)
}

fn check(entries: &[Entry], resolved: &Resolved) -> Result<(), Stop> {
    let (lines, failed) = report_lines(entries, resolved);
    for line in &lines {
        println!("{line}");
    }
    if failed > 0 {
        return Err(Stop::Fetch(failure_summary(
            failed,
            entries.len(),
            resolved,
        )));
    }
    println!("{NAME}: {} 件とも取得できます", entries.len());
    Ok(())
}

fn failure_summary(failed: usize, total: usize, resolved: &Resolved) -> String {
    let signin = resolved
        .failures
        .values()
        .any(|f| *f == Failure::NotSignedIn);
    let hint = if signin {
        "。service account の token を確かめてください"
    } else {
        ""
    };
    format!("{total} 件中 {failed} 件を取得できませんでした{hint}")
}

fn write_env(target: &std::path::Path, entries: &[Entry], resolved: Resolved) -> Result<(), Stop> {
    let (lines, failed) = report_lines(entries, &resolved);
    if failed > 0 {
        for line in lines.iter().filter(|line| line.starts_with('✗')) {
            eprintln!("{line}");
        }
        return Err(Stop::Fetch(format!(
            "{}。{} には何も書いていません",
            failure_summary(failed, entries.len(), &resolved),
            target.display()
        )));
    }
    let values: Vec<(String, Secret)> = entries
        .iter()
        .map(|entry| {
            let value = resolved.values[&entry.reference].clone();
            (entry.key.clone(), value)
        })
        .collect();
    let existing = write::read_existing(target).map_err(|e| Stop::Usage(e.to_string()))?;
    let merged = dotenv::merge(existing.expose(), &values).map_err(|error| {
        Stop::Fetch(format!(
            "{error}。{} には何も書いていません",
            target.display()
        ))
    })?;
    write::write_secret_file(target, &merged).map_err(write_error_stop)?;
    println!(
        "{NAME}: {} に {} 件を書きました（0600）",
        target.display(),
        entries.len()
    );
    Ok(())
}

fn write_error_stop(error: write::WriteError) -> Stop {
    match error {
        write::WriteError::DurabilityUnconfirmed(path, cause) => Stop::Usage(format!(
            "{} は書込済みですが、directory fsync に失敗したため永続化は未確認です: {cause}",
            path.display()
        )),
        other => Stop::Usage(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_rename_sync_failure_is_reported_as_written_but_unconfirmed() {
        let error = write::WriteError::DurabilityUnconfirmed(
            PathBuf::from(".env"),
            std::io::Error::other("injected directory fsync failure"),
        );
        let Stop::Usage(message) = write_error_stop(error) else {
            panic!("write failure must preserve exit code 2");
        };
        assert!(message.contains(".env は書込済みですが"), "{message}");
        assert!(message.contains("永続化は未確認"), "{message}");
    }
}
