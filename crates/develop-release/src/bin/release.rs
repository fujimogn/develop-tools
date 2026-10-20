//! `release <patch|minor|major|x.y.z>` — 版を 1 段上げ、commit と tag まで作る。
//!
//! push はしない。遠隔へ出すのは `release push`（`~/.agents/AGENTS.md` §4.2）。

use develop_release::{Bump, next_version, version_files::VersionFiles};
use develop_tools_core::{RELEASE_BRANCH, cli_help::Help, release_integration_branch};
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    // 継いだ `GIT_*` は `current_dir` に勝つ。hook 経由でも root を外さない。
    develop_tools_core::strip_git_env(&mut command);
    let out = command
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|err| format!("git を起動できません: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} に失敗しました: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let arg = args
        .next()
        .ok_or_else(|| "使い方: release <patch | minor | major | x.y.z>".to_owned())?;
    if arg == "push" {
        return develop_release::push::push_cli(&args.collect::<Vec<_>>());
    }
    if arg == "check" {
        return develop_release::preflight::check_cli(&args.collect::<Vec<_>>());
    }
    if arg == "--help" || arg == "-h" {
        print!(
            "{}",
            Help::new(
                "release <patch | minor | major | x.y.z>",
                "Advance declared version files on the integration branch, fast-forward main, then create a local tag. It does not push. Publish with release push. When .develop-tools.json declares platforms, the latest release check run on HEAD must have a successful job for every declared OS; run release check first.",
                "applies changes when a version bump is supplied; without an argument, exits without writing",
                "files declared by release.versionFiles, and root CHANGELOG.md when release.changelogStamp is true; creates a git commit, advances main, and creates a v<version> tag; no remote push. Publish with release push.",
            )
            .exit("0 success; 1 invalid input or failed release")
            .render()
        );
        return Ok(());
    }
    let bump: Bump = arg.parse()?;
    let root = develop_release::current_repo_root()?
        .ok_or_else(|| "Git repo の外では release を実行できません".to_owned())?;
    let integration_branch =
        release_integration_branch(&root)?.unwrap_or_else(|| RELEASE_BRANCH.to_owned());

    let branch = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if branch != integration_branch {
        return Err(format!(
            "{integration_branch} branch で実行してください（現在: {branch}）"
        ));
    }

    // **作業が残ったまま版を打たない。** 版と中身が対応しなくなる。
    let dirty = git(&root, &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err(format!(
            "未コミットの変更があります。先に片付けてください:\n{dirty}"
        ));
    }

    let old_main = git(
        &root,
        &["rev-parse", &format!("refs/heads/{RELEASE_BRANCH}")],
    )?;
    git(&root, &["merge-base", "--is-ancestor", &old_main, "HEAD"]).map_err(|_| {
        format!("{RELEASE_BRANCH} が {integration_branch} の祖先ではありません。Release を止めます")
    })?;

    // **宣言した全 OS の検査が緑の HEAD にだけ版を打つ。** platforms が無ければ gh を呼ばない。
    develop_release::preflight::verify_platforms(&root)?;

    let versions = VersionFiles::load(&root)?;
    let current = versions.current();
    let next = next_version(current, bump)?;
    if !git(&root, &["tag", "--list", &format!("v{next}")])?.is_empty() {
        return Err(format!("tag v{next} は既に存在します"));
    }

    let written = versions.write(&root, next)?;
    println!("✓ {} を v{next} の版 commit 用に更新", written.join(" / "));

    let mut add = vec!["add"];
    add.extend(written.iter().map(String::as_str));
    git(&root, &add)?;
    git(&root, &["commit", "-m", &format!("🔖 v{next}")])?;
    let released = git(&root, &["rev-parse", "HEAD"])?;
    if integration_branch != RELEASE_BRANCH {
        git(
            &root,
            &[
                "update-ref",
                &format!("refs/heads/{RELEASE_BRANCH}"),
                &released,
                &old_main,
            ],
        )?;
    }
    git(&root, &["tag", &format!("v{next}")])?;
    println!(
        "✓ {integration_branch} に版 commit、{RELEASE_BRANCH} を fast-forward、同じ SHA に tag v{next} を作成"
    );
    println!("\n遠隔へ出すには release push を実行してください");
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("release: {err}");
        std::process::exit(1);
    }
}
