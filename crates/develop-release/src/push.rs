//! `release push`。統合先、main、tag の順に完全な refspec で push する。
//!
//! tag は、main の SHA の push を契機にした run がすべて成功してから出す。
//! skipped と neutral は成功にしない。design は成功の条件を「すべて成功」とだけ
//! 書いており、この 2 つは取消とは分けて失敗として conclusion を報告する。

use crate::Version;
use crate::ci_runs::{self, ListError, RunFilter, WaitError};
use develop_tools_core::cli_help::Help;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

pub use crate::ci_runs::{CI_WAIT, POLL_INTERVAL};

pub fn push_cli(args: &[String]) -> Result<(), String> {
    let mut skip_ci = false;
    for arg in args {
        match arg.as_str() {
            "--tag-without-ci-check" => skip_ci = true,
            "--help" | "-h" => {
                print!("{}", push_help());
                return Ok(());
            }
            other => return Err(format!("不明な引数です: {other}")),
        }
    }
    let root = crate::current_repo_root()?
        .ok_or_else(|| "Git repo の外では release push を実行できません".to_owned())?;
    let started = std::time::Instant::now();
    run_release_push(&root, skip_ci, || started.elapsed(), std::thread::sleep)
}

/// `release push` の本体。待ち時間は [`CI_WAIT`] だけを見る。
pub fn run_release_push<N, S>(
    root: &Path,
    skip_ci: bool,
    mut now: N,
    mut sleep: S,
) -> Result<(), String>
where
    N: FnMut() -> Duration,
    S: FnMut(Duration),
{
    let integration = develop_tools_core::release_integration_branch(root)?
        .ok_or("版固定 file から日常統合先を決められません")?;
    let main_branch = develop_tools_core::RELEASE_BRANCH;
    let main_sha = rev_parse(root, &format!("refs/heads/{main_branch}"))?;
    let tag = find_tag(root, &main_sha)?;
    let mut integration_ok = false;

    if integration != main_branch {
        push_ref(root, "heads", &integration)
            .map_err(|err| format!("{err}\n{}", stages(&integration, false, false)))?;
        integration_ok = true;
    }
    push_ref(root, "heads", main_branch)
        .map_err(|err| format!("{err}\n{}", stages(&integration, integration_ok, false)))?;

    if !tracked_workflow(root)? {
        println!("✓ tracked の workflow が無いので CI は待ちません");
        return push_ref(root, "tags", &tag);
    }
    if skip_ci {
        eprintln!("警告: CI の確認を飛ばして tag を push します (--tag-without-ci-check)");
        return push_ref(root, "tags", &tag);
    }

    let report = stages(&integration, integration_ok, true);
    ensure_gh(root).map_err(|err| format!("{err}\n{report}"))?;
    wait_for_ci(root, &main_sha, main_branch, &mut now, &mut sleep)
        .map_err(|err| format!("{err}\n{report}"))?;
    push_ref(root, "tags", &tag).map_err(|err| format!("{err}\n{report}"))
}

fn push_help() -> String {
    let examples = ["release push", "release push --tag-without-ci-check"];
    Help::new(
        "release push [--tag-without-ci-check]",
        "Push the integration branch, then main, then the release tag. The tag is pushed only after every workflow run for that main push succeeds.",
        "waits for GitHub Actions up to the tool's CI_WAIT constant",
        "pushes branch and tag refs to origin; leaves the tag local when CI cannot be confirmed",
    )
    .exit("0 when the tag push is verified; 1 when a stage or the CI check fails")
    .examples(&examples)
    .render()
}

fn stages(integration: &str, integration_ok: bool, main_ok: bool) -> String {
    let mut lines = Vec::new();
    if integration != develop_tools_core::RELEASE_BRANCH {
        lines.push(format!(
            "統合先 {integration}: {}",
            if integration_ok {
                "届いた"
            } else {
                "届いていない"
            }
        ));
    }
    lines.push(format!(
        "{}: {}",
        develop_tools_core::RELEASE_BRANCH,
        if main_ok {
            "届いた"
        } else {
            "届いていない"
        }
    ));
    lines.push("tag: 手元に残した".to_owned());
    lines.join("\n")
}

fn push_ref(root: &Path, namespace: &str, name: &str) -> Result<(), String> {
    let spec = format!("refs/{namespace}/{name}");
    let label = if namespace == "tags" {
        format!("tag {name}")
    } else {
        name.to_owned()
    };
    git_push(root, &spec).map_err(|err| format!("{label} の push が拒否されました: {err}"))?;
    let local = rev_parse(root, &spec)?;
    let remote = remote_sha(root, &spec)?;
    if remote != local {
        return Err(format!(
            "{label} の遠隔 SHA が一致しません（ローカル {local}、遠隔 {remote}）"
        ));
    }
    println!("✓ {label} を push し、遠隔の SHA が一致しました");
    Ok(())
}

fn find_tag(root: &Path, sha: &str) -> Result<String, String> {
    let listed = git_stdout(root, &["tag", "--points-at", sha])?;
    let mut tags = Vec::new();
    for line in listed.lines() {
        let Some(version) = line.strip_prefix('v') else {
            continue;
        };
        if version.parse::<Version>().is_ok() {
            tags.push(line.to_owned());
        }
    }
    tags.sort();
    match tags.len() {
        1 => Ok(tags.remove(0)),
        0 => Err("main の SHA に v<semver> の tag がありません".to_owned()),
        _ => Err(format!(
            "main の SHA に版の tag が複数あります: {}",
            tags.join(" ")
        )),
    }
}

fn tracked_workflow(root: &Path) -> Result<bool, String> {
    let listed = git_stdout(root, &["ls-files", "--", ".github/workflows"])?;
    Ok(listed.lines().any(|line| !line.is_empty()))
}

fn ask_to_push_after_checking_ci(reason: &str) -> String {
    format!(
        "{reason}。CI の成功を確かめてから release push --tag-without-ci-check を実行してください"
    )
}

fn ensure_gh(root: &Path) -> Result<(), String> {
    match ci_runs::gh(root, &["auth", "status"]) {
        Ok(_) => Ok(()),
        Err(err) if err.not_found => Err(ask_to_push_after_checking_ci(
            "gh が無いので CI の結論を確かめられません",
        )),
        Err(err) => Err(ask_to_push_after_checking_ci(&format!(
            "gh の認証ができないので CI の結論を確かめられません: {}",
            err.message
        ))),
    }
}

fn wait_for_ci<N, S>(
    root: &Path,
    sha: &str,
    branch: &str,
    now: &mut N,
    sleep: &mut S,
) -> Result<(), String>
where
    N: FnMut() -> Duration,
    S: FnMut(Duration),
{
    let exclude = BTreeSet::new();
    let filter = RunFilter {
        sha,
        event: "push",
        workflow: None,
        branch: Some(branch),
        exclude: &exclude,
    };
    match ci_runs::wait_for_runs(root, &filter, now, sleep) {
        Ok(runs) => ci_runs::decide(&runs),
        Err(WaitError::Timeout { saw_run, pending }) => {
            Err(ci_runs::out_of_time(saw_run, &pending))
        }
        Err(WaitError::List(err)) => Err(cannot_read_ci(err)),
    }
}

fn cannot_read_ci(err: ListError) -> String {
    match err {
        ListError::Gh(err) if err.not_found => {
            ask_to_push_after_checking_ci("gh が無いので CI の結論を確かめられません")
        }
        ListError::Gh(err) => {
            ask_to_push_after_checking_ci(&format!("CI の結論を確かめられません: {}", err.message))
        }
        ListError::Parse(err) => {
            ask_to_push_after_checking_ci(&format!("CI の結論を確かめられません: {err}"))
        }
        ListError::LimitReached => {
            "run が上限に達したため、すべてを確認できません。tag は手元に残しました".to_owned()
        }
    }
}

fn git_push(root: &Path, spec: &str) -> Result<(), String> {
    let refspec = format!("{spec}:{spec}");
    let output = develop_tools_core::git(root)
        .args(["push", "origin", &refspec])
        .output()
        .map_err(|err| format!("git を起動できません: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
}

fn rev_parse(root: &Path, spec: &str) -> Result<String, String> {
    git_stdout(root, &["rev-parse", spec])
}

fn remote_sha(root: &Path, spec: &str) -> Result<String, String> {
    let listed = git_stdout(root, &["ls-remote", "origin", spec])?;
    listed
        .split_whitespace()
        .next()
        .filter(|sha| !sha.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{spec} が遠隔にありません"))
}

pub(crate) fn git_stdout(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = develop_tools_core::git(root)
        .args(args)
        .output()
        .map_err(|err| format!("git を起動できません: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} に失敗しました: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
