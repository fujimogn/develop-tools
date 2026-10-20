//! staged された repository 直下の TODO.md / CHANGELOG.md を検査する pre-commit 入口。

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use develop_core::cli_help::Help;
use develop_core::git;
use develop_staged::docs_standard::{Finding, FindingKind, inspect_changelog, inspect_todo};
use develop_staged::staged_adr::resolve_index_file;

const DOC_PATHS: [&str; 2] = ["TODO.md", "CHANGELOG.md"];

struct StagedPaths {
    changed: HashSet<Vec<u8>>,
    indexed: HashSet<Vec<u8>>,
}

fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        let help = Help::new(
            "check-staged-docs [--index-file PATH] [--help]",
            "check-staged-docs — staged された TODO.md / CHANGELOG.md を検査します。",
            "cwd の Git repo で、index に staged された repository 直下の対象文書だけを読みます。hook から明示 index を渡せます。",
            "なし。Git index と staged 文書を読み取り、working tree や index を変更しません。",
        )
        .exit(
            "0 = 対象なしまたは警告のみ、1 = 文書違反あり、2 = Git または index の読み取りエラー",
        );
        print!("{}", help.render());
        return;
    }

    let index_file = match parse_args(&args) {
        Ok(index_file) => index_file,
        Err(message) => fatal(&message),
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => fatal(&format!("cwd: {error}")),
    };
    let repo = match git_toplevel(&cwd) {
        Ok(root) => root,
        Err(message) => fatal(&message),
    };
    let index_file = match index_file {
        Some(path) => match resolve_index_file(&cwd, &repo, &path) {
            Ok(path) => Some(path),
            Err(message) => fatal(&message),
        },
        None => None,
    };
    let paths = match staged_and_indexed_paths(&repo, index_file.as_deref()) {
        Ok(paths) => paths,
        Err(message) => fatal(&message),
    };

    let mut errors = 0usize;
    let mut warnings = 0usize;
    for path in DOC_PATHS {
        if !paths.changed.contains(path.as_bytes()) || !paths.indexed.contains(path.as_bytes()) {
            continue;
        }
        let contents = match read_index_blob(&repo, path, index_file.as_deref()) {
            Ok(contents) => contents,
            Err(message) => fatal(&message),
        };
        let findings = match path {
            "TODO.md" => inspect_todo(&contents),
            "CHANGELOG.md" => inspect_changelog(&contents),
            _ => unreachable!("DOC_PATHS is exhaustive"),
        };
        for finding in findings {
            match finding.kind {
                FindingKind::Error => {
                    errors += 1;
                    eprintln!("error: {}: {}", location(path, &finding), finding.message);
                }
                FindingKind::Warning => {
                    warnings += 1;
                    eprintln!("warning: {}: {}", location(path, &finding), finding.message);
                }
            }
        }
    }

    if errors > 0 {
        eprintln!("check-staged-docs: {errors} error(s), {warnings} warning(s)");
        std::process::exit(1);
    }
    if warnings > 0 {
        eprintln!("check-staged-docs: 0 error(s), {warnings} warning(s)");
    }
}

fn parse_args(args: &[OsString]) -> Result<Option<PathBuf>, String> {
    let mut index_file = None;
    let mut position = 0;
    while position < args.len() {
        let arg = &args[position];
        if arg == "--index-file" {
            if index_file.is_some() {
                return Err("--index-file は 1 回だけ指定できます".to_string());
            }
            let Some(path) = args.get(position + 1) else {
                return Err("--index-file に path が必要です".to_string());
            };
            if path.is_empty() {
                return Err("--index-file の path は空にできません".to_string());
            }
            if path == "--index-file" || path == "--help" || path == "-h" {
                return Err("--index-file に path が必要です".to_string());
            }
            index_file = Some(PathBuf::from(path));
            position += 1;
        } else {
            return Err(format!("不明な引数: {}", arg.to_string_lossy()));
        }
        position += 1;
    }
    Ok(index_file)
}

fn location(path: &str, finding: &Finding) -> String {
    match finding.line {
        Some(line) => format!("{path}:{line}"),
        None => path.to_string(),
    }
}

fn fatal(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(2);
}

fn git_toplevel(cwd: &Path) -> Result<PathBuf, String> {
    let output = git(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

fn git_with_index(repo: &Path, index_file: Option<&Path>) -> Command {
    let mut command = git(repo);
    if let Some(index_file) = index_file {
        command.env("GIT_INDEX_FILE", index_file);
    }
    command
}

fn staged_and_indexed_paths(repo: &Path, index_file: Option<&Path>) -> Result<StagedPaths, String> {
    let changed = git_with_index(repo, index_file)
        .args(["diff", "--cached", "--name-only", "-z"])
        .output()
        .map_err(|error| format!("git diff --cached: {error}"))?;
    if !changed.status.success() {
        return Err(format!(
            "git diff --cached failed: {}",
            String::from_utf8_lossy(&changed.stderr).trim()
        ));
    }

    let indexed = git_with_index(repo, index_file)
        .args(["ls-files", "--cached", "-z"])
        .output()
        .map_err(|error| format!("git ls-files: {error}"))?;
    if !indexed.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&indexed.stderr).trim()
        ));
    }

    Ok(StagedPaths {
        changed: nul_paths(&changed.stdout),
        indexed: nul_paths(&indexed.stdout),
    })
}

fn nul_paths(bytes: &[u8]) -> HashSet<Vec<u8>> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

fn read_index_blob(repo: &Path, path: &str, index_file: Option<&Path>) -> Result<Vec<u8>, String> {
    let spec = format!(":{path}");
    let output = git_with_index(repo, index_file)
        .args(["show", &spec])
        .output()
        .map_err(|error| format!("git show {spec}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git show {spec} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}
