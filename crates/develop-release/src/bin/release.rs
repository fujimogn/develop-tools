//! `release <patch|minor|major|x.y.z>` — 版を 1 段上げ、commit と tag まで作る。
//!
//! push はしない。遠隔へ出すのは `release push`（`~/.agents/AGENTS.md` §4.2）。

use develop_release::release_notes::{self, ReleaseBody};
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
        .ok_or_else(|| "使い方: release <patch | minor | major | x.y.z> [--no-ai] [--first-release-base <完全なOID>]".to_owned())?;
    if arg == "push" {
        return develop_release::push::push_cli(&args.collect::<Vec<_>>());
    }
    if arg == "check" {
        return develop_release::preflight::check_cli(&args.collect::<Vec<_>>());
    }
    if arg == "--help" || arg == "-h" {
        let release = RELEASE_BRANCH;
        let description = format!(
            "Advance declared version files on the integration branch, fast-forward {release}, then create a local tag. It does not push. Publish with release push. When .config/develop-tools.json (or the legacy .develop-tools.json during migration) declares platforms, the latest release check run on HEAD must have a successful job for every declared OS; run release check first. When it declares release.notes, the tag is annotated with the release body; mode ai makes one Anthropic API request for the body and falls back to the CHANGELOG extract when that is not usable. A changelog-section body (mode or fallback) needs exactly one non-empty dated section for the version (## X.Y.Z (YYYY-MM-DD) or ## [X.Y.Z] - YYYY-MM-DD) in CHANGELOG.md at HEAD. A tagless stamped monthly changelog-additions release instead requires --first-release-base with a full commit or tree OID; the full diff must match every pending monthly item. --no-ai skips the request."
        );
        let writes = format!(
            "files declared by release.versionFiles, and root CHANGELOG.md when release.changelogStamp is true; creates a git commit, advances {release}, and creates a v<version> tag (annotated when release.notes is declared); no remote push. With release.notes mode ai and without --no-ai, sends the scrubbed CHANGELOG items to Anthropic once and records AI fallback reasons (timestamp_unix_seconds, tag, reason_code only) in release-notes-fallback.jsonl in the common Git directory, warning with fallback_reason_unrecorded if recording fails while continuing the release; appends one row per API request to the usage ledger named by usage_ledger in ~/.config/develop-tools/llm.json (without that field, nothing is sent). A first release skips inference and uses the version section, or the fixed monthly diff when --first-release-base is supplied. Publish with release push."
        );
        print!(
            "{}",
            Help::new(
                "release <patch | minor | major | x.y.z> [--no-ai] [--first-release-base <完全なOID>]",
                &description,
                "applies changes when a version bump is supplied; without an argument, exits without writing",
                &writes,
            )
            .exit("0 success; 1 invalid input or failed release")
            .render()
        );
        return Ok(());
    }
    let bump: Bump = arg.parse()?;
    let mut no_ai = false;
    let mut first_release_base = None;
    while let Some(option) = args.next() {
        match option.as_str() {
            "--no-ai" if !no_ai => no_ai = true,
            "--first-release-base" if first_release_base.is_none() => {
                first_release_base = Some(
                    args.next()
                        .ok_or("first_release_base_invalid: 値がありません")?,
                );
            }
            other => return Err(format!("知らない引数です: {other}")),
        }
    }
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
    let tag = format!("v{next}");
    if !git(&root, &["tag", "--list", &tag])?.is_empty() {
        return Err(format!("tag {tag} は既に存在します"));
    }
    // **本文の材料は書く前に決める。** 版 commit の後は CHANGELOG の diff を読むだけにし、
    // commit したのに tag を作れない状態を減らす。
    let notes = release_notes::declared_notes(&root)?
        .map(|declared| {
            release_notes::prepare_notes_with_first_release_base(
                &root,
                declared,
                &tag,
                first_release_base.as_deref(),
            )
        })
        .transpose()?;
    if notes.is_none() && first_release_base.is_some() {
        return Err("first_release_base_invalid: release.notes の宣言が必要です".to_owned());
    }

    let written = versions.write(&root, next)?;
    println!("✓ {} を v{next} の版 commit 用に更新", written.join(" / "));

    let mut add = vec!["add"];
    add.extend(written.iter().map(String::as_str));
    git(&root, &add)?;
    git(&root, &["commit", "-m", &format!("🔖 v{next}")])?;
    let released = git(&root, &["rev-parse", "HEAD"])?;
    let body = notes
        .map(|prepared| body(&prepared, &root, &released, no_ai))
        .transpose()
        .map_err(|err| {
            format!(
                "{err}\n{integration_branch} に版 commit {released} を作りましたが、{RELEASE_BRANCH} と tag は動かしていません。tag を手で打つか、版 commit を戻してください"
            )
        })?;
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
    // **ここで失敗すると main だけ進んだ部分完了になる。** 作り直す手順を添えて止める。
    let tagged = match &body {
        Some(body) => release_notes::annotate_tag(&root, &tag, &released, &body.text),
        None => git(&root, &["tag", &tag, &released]).map(drop),
    };
    tagged.map_err(|err| {
        let advanced = if integration_branch == RELEASE_BRANCH {
            format!("{RELEASE_BRANCH} に版 commit {released} を作りました")
        } else {
            format!(
                "{integration_branch} に版 commit {released} を作り、{RELEASE_BRANCH} を {old_main} から進めました"
            )
        };
        let recreate = match &body {
            Some(body) => format!(
                "次の本文を file に保存し、git tag -a --cleanup=verbatim -F <その file> {tag} {released} で tag を作ってから release push してください。\n\n{}",
                body.text
            ),
            None => format!("git tag {tag} {released} で tag を作ってから release push してください"),
        };
        format!("{err}\n{advanced}が、tag {tag} は作れていません。{recreate}")
    })?;
    println!(
        "✓ {integration_branch} に版 commit、{RELEASE_BRANCH} を fast-forward、同じ SHA に tag {tag} を作成"
    );
    if let Some(ReleaseBody { text, source }) = &body {
        println!("\n本文: {source}");
        if matches!(source, release_notes::BodySource::Fallback(_)) {
            println!(
                "  環境側の設定の欄は release check で確かめられます。欠けていれば環境側の bin/install-tools で書き直してください"
            );
        }
        println!("\n{text}");
        println!(
            "直すなら git tag -f -a {tag} {released} で作り直してから release push してください"
        );
    }
    println!("\n遠隔へ出すには release push を実行してください");
    Ok(())
}

/// 本番の送信で本文を決める。環境側の設定は `~/.config/develop-tools/llm.json`。
fn body(
    prepared: &release_notes::PreparedNotes,
    root: &Path,
    end: &str,
    no_ai: bool,
) -> Result<ReleaseBody, String> {
    release_notes::with_default_ai_runtime(|settings, runtime| {
        prepared.body(root, end, no_ai, settings, runtime)
    })
}

fn main() {
    if let Err(err) = run() {
        eprintln!("release: {err}");
        std::process::exit(1);
    }
}
