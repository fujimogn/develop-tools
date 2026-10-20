//! CHANGELOG.md の古い月を Git 履歴へ退避する。既定では dry-run。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use develop_core::cli_help::Help;
use develop_core::git;
use develop_staged::docs_standard::plan_changelog_archive;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        let help = Help::new(
            "archive-changelog [--yes]",
            "archive-changelog — CHANGELOG.md の古い月を git 履歴へ退避します。",
            "既定は変更しない dry-run です。確認後は --yes で working tree を更新します。",
            "CHANGELOG.md のみを編集します。旧節を別ファイルへ移しません。",
        )
        .exit("成功 = dry-run または反映成功、1 = 当月を残すと上限以下にできない、2 = 入力または Git のエラー")
        .render();
        print!("{help}");
        return;
    }
    if args.iter().any(|arg| arg != "--yes") {
        fatal(2, "使い方: archive-changelog [--yes]");
    }
    let apply = args.iter().any(|arg| arg == "--yes");

    let repo = match git_toplevel() {
        Ok(repo) => repo,
        Err(message) => fatal(2, &message),
    };
    let path = repo.join("CHANGELOG.md");
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => fatal(2, &format!("{}: {error}", path.display())),
    };
    let current_month = match current_month() {
        Ok(month) => month,
        Err(message) => fatal(2, &message),
    };
    let commit_sha = match git_output(&repo, &["rev-parse", "HEAD"]) {
        Ok(sha) => sha,
        Err(message) => fatal(2, &message),
    };
    let plan = match plan_changelog_archive(&content, &current_month, &commit_sha) {
        Ok(plan) => plan,
        Err(message) => fatal(1, &message),
    };

    println!(
        "CHANGELOG.md: {} bytes → {} bytes",
        plan.bytes_before, plan.bytes_after
    );
    if plan.archived_months.is_empty() {
        println!("退避する月: なし（変更不要）");
        return;
    }
    println!(
        "退避する月: {}〜{}",
        plan.archived_months.first().expect("non-empty months"),
        plan.archived_months.last().expect("non-empty months")
    );
    println!("退避直前の commit: {commit_sha}");

    if !apply {
        println!(
            "dry-run のため CHANGELOG.md は変更していません。反映には --yes を指定してください。"
        );
        return;
    }
    if let Err(error) = fs::write(&path, plan.content) {
        fatal(2, &format!("{}: {error}", path.display()));
    }
    println!("CHANGELOG.md を更新しました。");
}

fn git_toplevel() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("cwd: {error}"))?;
    let output = git(&cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("git rev-parse: {error}"))?;
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

fn git_output(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = git(repo)
        .args(args)
        .output()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn current_month() -> Result<String, String> {
    let output = Command::new("date")
        .arg("+%Y-%m")
        .output()
        .map_err(|error| format!("date: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "date +%Y-%m failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn fatal(code: i32, message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(code);
}
