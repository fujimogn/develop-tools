//! staged された `docs/adr/` を `adr_standard::inspect_staged` へ通す pre-commit 入口。

use develop_adr::{validate_archive, validate_sanitized_archive};
use develop_core::cli_help::Help;
use develop_core::git;
use develop_staged::adr_standard::{ArchiveValidators, Options, inspect_staged_with_index};
use develop_staged::staged_adr::{
    adr_contract, list_staged_with_index, resolve_index_file, staged_readme_with_index,
};
use std::ffi::OsString;
use std::path::PathBuf;

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let index_file = match parse_args(&args) {
        Ok(ParsedArgs::Help) => {
            show_help();
            return;
        }
        Ok(ParsedArgs::Index(index_file)) => index_file,
        Err(message) => fatal(&message),
    };

    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => fatal(&format!("cwd: {error}")),
    };

    let repo = match git_toplevel() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(2);
        }
    };

    let index_file = match index_file {
        Some(path) => match resolve_index_file(&cwd, &repo, &path) {
            Ok(path) => Some(path),
            Err(message) => fatal(&message),
        },
        None => None,
    };

    let staged = match list_staged_with_index(&repo, index_file.as_deref()) {
        Ok(files) => files,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(2);
        }
    };
    let has_staged_adr = staged
        .iter()
        .any(|file| file.name != "README.md" && file.name != "adr-template.md");
    if !has_staged_adr {
        std::process::exit(0);
    }
    let contract = match adr_contract(&repo, index_file.as_deref()) {
        Ok(contract) => contract,
        Err(message) => fatal(&message),
    };
    let readme = staged_readme_with_index(&repo, &staged, index_file.as_deref());
    let validators = ArchiveValidators {
        archive: &validate_archive,
        sanitized: &validate_sanitized_archive,
    };
    let issues = inspect_staged_with_index(
        &Options {
            project_root: &repo,
            // staged の判定は雛形を比べない。雛形の一致は配布と drift の検査が見る。
            canonical_template: "",
            migrated_through: contract.migrated_through,
            reserved_numbers: &contract.reserved_numbers,
            sanitized_numbers: &contract.sanitized_numbers,
        },
        &validators,
        &staged,
        readme.as_deref(),
        index_file.as_deref(),
    );
    if !issues.is_empty() {
        for issue in &issues {
            eprintln!("{}: {}", issue.code, issue.message);
        }
        std::process::exit(1);
    }
}

enum ParsedArgs {
    Help,
    Index(Option<PathBuf>),
}

fn parse_args(args: &[OsString]) -> Result<ParsedArgs, String> {
    let mut index_file = None;
    let mut position = 0;
    while position < args.len() {
        if args[position] == "--help" || args[position] == "-h" {
            return Ok(ParsedArgs::Help);
        }
        if args[position] != "--index-file" {
            return Err(format!("不明な引数: {}", args[position].to_string_lossy()));
        }
        if index_file.is_some() {
            return Err("--index-file は 1 回だけ指定できます".to_string());
        }
        let Some(path) = args.get(position + 1) else {
            return Err("--index-file に path が必要です".to_string());
        };
        if path.is_empty() || path == "--index-file" || path == "--help" || path == "-h" {
            return Err("--index-file に path が必要です".to_string());
        }
        index_file = Some(PathBuf::from(path));
        position += 2;
    }
    Ok(ParsedArgs::Index(index_file))
}

fn show_help() {
    let help = Help::new(
        "check-staged-adr [--index-file PATH] [--help]",
        "check-staged-adr — staged された ADR の構造と legacy archive を検査します。",
        "cwd の Git repo で git diff --cached の docs/adr 配下を検査します。対象 ADR が無い場合は正常終了します。",
        "なし。staged diff と ADR、index の .develop-tools.json の ADR 番号の契約を読み取り、ファイルや index を変更しません。",
    )
    .exit("0 = 対象 ADR なしまたは指摘なし、1 = 指摘あり、2 = Git または ADR 番号の契約の読み取りエラー");
    print!("{}", help.render());
}

fn fatal(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(2);
}

fn git_toplevel() -> Result<std::path::PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("cwd: {error}"))?;
    let output = git(&cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(std::path::PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}
