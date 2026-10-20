//! `develop-docs-check` — 島共通の docs 検査の入口。

use std::path::PathBuf;
use std::process::ExitCode;

use develop_core::cli_help::Help;
use island_docs::check::{self, Mode};
use island_docs::git::Repo;

const NAME: &str = "develop-docs-check";

fn help() -> String {
    Help::new(
        "develop-docs-check (--tracked | --staged [--index-file <path>] | --range <base> <head>) [--help | --version | --check-local-version]",
        "島共通の docs 検査。tracked の Markdown の local link と見出し anchor、repo の scope 表の写し（config/island-docs-scopes.json）にある容量上限を検査します。",
        "cwd を含む git repo を読みます。--tracked は index の全文書、--staged は staged の変更、--range は base..head の変更に関わる問題だけを報告します。--range は head の tree を読み、base が全 0 または手元に無ければ head の第一親との差（root commit は自身の追加分）に絞ります。上限値と repo ごとの行は、検査する snapshot（index または head の tree）にある scope 表の写しだけから決めます。写しは必須で、無ければ exit 2 で止まります。",
        "なし（git の index・commit tree・remote.origin.url を読むだけ）。",
    )
    .exit("0 = 問題なし、1 = 問題あり、2 = 引数・git の読み取りエラー")
    .examples(&[
        "develop-docs-check --tracked",
        "develop-docs-check --staged",
        "develop-docs-check --staged --index-file \"$saved_index_file\"",
        "develop-docs-check --range \"$before\" \"$sha\"",
    ])
    .render()
}

struct Args {
    mode: Mode,
    index_file: Option<PathBuf>,
}

enum Parsed {
    Help,
    Version,
    CheckLocalVersion,
    Run(Args),
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
    let args: Vec<String> = args.into_iter().collect();
    match args.as_slice() {
        [arg] if arg == "--version" => return Ok(Parsed::Version),
        [arg] if arg == "--check-local-version" => return Ok(Parsed::CheckLocalVersion),
        _ => {}
    }
    let mut mode = None;
    let mut index_file = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let next_mode = match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--tracked" => Mode::Tracked,
            "--staged" => Mode::Staged,
            "--range" => {
                let base = args.next().ok_or("--range に base と head が必要です")?;
                let head = args.next().ok_or("--range に base と head が必要です")?;
                Mode::Range { base, head }
            }
            "--index-file" => {
                let path = args
                    .next()
                    .filter(|p| !p.is_empty() && !p.starts_with("--"))
                    .ok_or("--index-file に path が必要です")?;
                index_file = Some(PathBuf::from(path));
                continue;
            }
            other => return Err(format!("不明な引数です: {other}")),
        };
        if mode
            .replace(next_mode.clone())
            .is_some_and(|previous| previous != next_mode)
        {
            return Err("--tracked、--staged、--range は同時に指定できません".to_owned());
        }
    }
    let mode = mode.ok_or("--tracked、--staged、--range のいずれかを指定してください")?;
    if index_file.is_some() && mode != Mode::Staged {
        return Err("--index-file は --staged と一緒に指定してください".to_owned());
    }
    Ok(Parsed::Run(Args { mode, index_file }))
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(Parsed::Help) => {
            print!("{}", help());
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::CheckLocalVersion) => {
            if let Ok(cwd) = std::env::current_dir() {
                if let Some(warning) = develop_core::freshness::pinned_version_warning(
                    // packages/cli へ移す crate なので、配布先でも固定の manifest 名を使う。
                    &cwd.join(".develop-tools.json"),
                    env!("CARGO_PKG_VERSION"),
                ) {
                    eprintln!("{warning}");
                }
            }
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Run(args)) => args,
        Err(message) => {
            eprintln!("{NAME}: {message}");
            eprintln!("{NAME}: 使い方は --help を見てください");
            return ExitCode::from(2);
        }
    };
    let outcome = std::env::current_dir()
        .map_err(|error| format!("cwd を読めません: {error}"))
        .and_then(|cwd| Repo::discover(&cwd, args.index_file.as_deref()))
        .and_then(|repo| check::run(&repo, args.mode));
    match outcome {
        Err(message) => {
            eprintln!("{NAME}: {message}");
            ExitCode::from(2)
        }
        Ok(outcome) if outcome.findings.is_empty() => {
            println!("{NAME}: OK ({} 文書)", outcome.checked);
            ExitCode::SUCCESS
        }
        Ok(outcome) => {
            for finding in &outcome.findings {
                eprintln!("{NAME}: {finding}");
            }
            eprintln!("{NAME}: {} 件の問題があります", outcome.findings.len());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn version_and_warning_mode_are_recognized() {
        assert!(matches!(
            parse(["--version".to_owned()]),
            Ok(Parsed::Version)
        ));
        assert!(matches!(
            parse(["--check-local-version".to_owned()]),
            Ok(Parsed::CheckLocalVersion)
        ));
        assert!(matches!(
            parse(["--staged".to_owned(), "--staged".to_owned()]),
            Ok(Parsed::Run(Args {
                mode: Mode::Staged,
                ..
            }))
        ));
    }
}
