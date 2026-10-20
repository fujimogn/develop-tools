//! `release check`。統合先の HEAD で `release.preflightWorkflow` を dispatch し、その run の結論を待つ。
//!
//! 待つ run は dispatch の直前に記録した ID に無い 1 本だけにする。時刻では特定しないので、
//! 手元と GitHub の時計のずれにも、同じ HEAD の以前の run（失敗・取消を含む）にも左右されない。
//! `release.versionFiles` は読まない。版を持たない repo でも使える。
//!
//! [`verify_platforms`] は `release <bump>` が版を書く前に呼ぶ照合。dispatch せず、HEAD の最新の
//! dispatch の run で、宣言した各 OS の job が success かだけを読む。

use crate::ci_runs::{self, ListError, RunFilter, WaitError};
use crate::push::git_stdout;
use develop_tools_core::cli_help::Help;
use develop_tools_core::develop_tools_manifest::{self, Declaration, Platform};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

const DISPATCH_EVENT: &str = "workflow_dispatch";

pub fn check_cli(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None => {}
        Some("--help" | "-h") => {
            print!("{}", check_help());
            return Ok(());
        }
        Some(other) => return Err(format!("不明な引数です: {other}")),
    }
    let root = crate::current_repo_root()?
        .ok_or_else(|| "Git repo の外では release check を実行できません".to_owned())?;
    let started = std::time::Instant::now();
    run_release_check(&root, || started.elapsed(), std::thread::sleep)
}

/// `release check` の本体。待ち時間は `release push` と同じ [`ci_runs::CI_WAIT`] だけを見る。
pub fn run_release_check<N, S>(root: &Path, mut now: N, mut sleep: S) -> Result<(), String>
where
    N: FnMut() -> Duration,
    S: FnMut(Duration),
{
    let workflow = preflight_workflow(&read_declaration(root)?)?;
    let integration = integration_branch(root)?;
    let spec = format!("refs/heads/{integration}");
    let head = git_stdout(root, &["rev-parse", &spec])?;
    let remote = remote_head(root, &spec)?;
    if remote != head {
        let remote = if remote.is_empty() { "無し" } else { &remote };
        return Err(format!(
            "{integration} の HEAD {head} が origin に届いていません（origin {remote}）。先に push してください"
        ));
    }
    ensure_gh(root)?;

    let none = BTreeSet::new();
    // 統合先の branch で絞る。main に同じ SHA の dispatch があっても自分の run と取り違えない。
    let before = RunFilter {
        sha: &head,
        event: DISPATCH_EVENT,
        workflow: Some(&workflow),
        branch: Some(&integration),
        exclude: &none,
    };
    let recorded: BTreeSet<u64> = ci_runs::list_runs(root, &before)
        .map_err(cannot_read)?
        .into_iter()
        .map(|run| run.database_id)
        .collect();
    ci_runs::gh(root, &["workflow", "run", &workflow, "--ref", &integration])
        .map_err(|err| format!("{workflow} を dispatch できません: {}", err.message))?;
    // dispatch は `--ref` の branch のその時の HEAD で走る。記録の後に統合先が動いていたら、
    // 待つ run は現れないので、待たずに止める。
    let moved = remote_head(root, &spec)?;
    if moved != head {
        return Err(format!(
            "dispatch の前後で origin の {integration} が {head} から {moved} へ動いたため、dispatch した run が {head} を見たか分かりません。release check をやり直してください"
        ));
    }
    println!("✓ {workflow} を {integration}（{head}）で dispatch しました");

    let after = RunFilter {
        exclude: &recorded,
        ..before
    };
    let runs = match ci_runs::wait_for_runs(root, &after, &mut now, &mut sleep) {
        Ok(runs) => runs,
        Err(WaitError::Timeout { saw_run, pending }) => {
            return Err(ci_runs::out_of_time(saw_run, &pending));
        }
        Err(WaitError::List(err)) => return Err(cannot_read(err)),
    };
    let [run] = runs.as_slice() else {
        let ids: Vec<String> = runs
            .iter()
            .map(|run| format!("#{}", run.database_id))
            .collect();
        return Err(format!(
            "dispatch の後に {workflow} の run が複数現れ、自分の run を特定できません: {}",
            ids.join(" ")
        ));
    };
    let jobs = ci_runs::view_jobs(root, run.database_id).map_err(cannot_read)?;
    for job in &jobs {
        println!(
            "  {} {}",
            job.conclusion.as_deref().unwrap_or(&job.status),
            job.name
        );
    }
    ci_runs::decide(std::slice::from_ref(run))?;
    println!(
        "✓ {} #{} が成功しました {}",
        run.name, run.database_id, run.url
    );
    Ok(())
}

/// `release <bump>` が版を書く前の照合。`platforms` が無ければ gh を呼ばずに通す。
///
/// HEAD の `preflightWorkflow` の dispatch の run のうち最も新しい 1 本だけを読み、宣言した各 OS の
/// job がちょうど 1 つあって success のときだけ通す。OS は宣言からだけ取り、実行する機の OS を見ない。
pub fn verify_platforms(root: &Path) -> Result<(), String> {
    let declaration = read_declaration(root)?;
    if declaration.platforms.is_empty() {
        return Ok(());
    }
    let workflow = preflight_workflow(&declaration)?;
    let integration = integration_branch(root)?;
    let head = git_stdout(root, &["rev-parse", "HEAD"])?;
    ensure_gh(root)?;
    let none = BTreeSet::new();
    let filter = RunFilter {
        sha: &head,
        event: DISPATCH_EVENT,
        workflow: Some(&workflow),
        branch: Some(&integration),
        exclude: &none,
    };
    let runs = ci_runs::list_runs(root, &filter).map_err(cannot_read)?;
    // run の ID は作成の順に増えるので、最大の ID が作成の最も新しい run。
    let Some(run) = runs.iter().max_by_key(|run| run.database_id) else {
        return Err(format!(
            "HEAD {head} の {workflow} の dispatch の run がありません。先に release check を実行してください"
        ));
    };
    if run.status != "completed" {
        return Err(format!(
            "HEAD {head} の最新の {workflow} の run #{} がまだ {} です。release check の完了を待ってください {}",
            run.database_id, run.status, run.url
        ));
    }
    let jobs = ci_runs::view_jobs(root, run.database_id).map_err(cannot_read)?;
    let problems: Vec<String> = declaration
        .platforms
        .iter()
        .filter_map(|&platform| platform_problem(platform, &jobs))
        .collect();
    if problems.is_empty() {
        println!(
            "✓ {} #{} で {} の job が成功しています",
            run.name,
            run.database_id,
            declaration
                .platforms
                .iter()
                .map(|platform| platform.as_str())
                .collect::<Vec<_>>()
                .join(" / ")
        );
        return Ok(());
    }
    Err(format!(
        "HEAD {head} の {workflow} の run #{} で確かめられない OS があります。直してから release check を実行してください\n{}",
        run.database_id,
        problems.join("\n")
    ))
}

/// OS の job は `platform (<os>)` そのものか、呼び元の job 名を前に付けた ` / platform (<os>)` で当てる。
fn platform_problem(platform: Platform, jobs: &[ci_runs::Job]) -> Option<String> {
    let bare = format!("platform ({})", platform.as_str());
    let nested = format!(" / {bare}");
    let matched: Vec<&ci_runs::Job> = jobs
        .iter()
        .filter(|job| job.name == bare || job.name.ends_with(&nested))
        .collect();
    match matched.as_slice() {
        [] => Some(format!("{}: {bare} の job がありません", platform.as_str())),
        [job] if job.conclusion.as_deref() == Some("success") => None,
        [job] => Some(format!(
            "{}: {} が {} です {}",
            platform.as_str(),
            job.name,
            job.conclusion.as_deref().unwrap_or(&job.status),
            job.url
        )),
        _ => Some(format!(
            "{}: {bare} に当たる job が {} 個あり、どれか決められません",
            platform.as_str(),
            matched.len()
        )),
    }
}

fn read_declaration(root: &Path) -> Result<Declaration, String> {
    let path = develop_tools_manifest::PATH;
    let raw = std::fs::read_to_string(root.join(path)).map_err(|err| format!("{path}: {err}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|err| format!("{path}: JSON が不正です: {err}"))?;
    develop_tools_manifest::declaration(&value).map_err(|err| format!("{path}: {err}"))
}

fn preflight_workflow(declaration: &Declaration) -> Result<String, String> {
    declaration.preflight_workflow.clone().ok_or_else(|| {
        format!(
            "{} に release.preflightWorkflow がありません",
            develop_tools_manifest::PATH
        )
    })
}

fn integration_branch(root: &Path) -> Result<String, String> {
    Ok(develop_tools_core::release_integration_branch(root)?
        .unwrap_or_else(|| develop_tools_core::RELEASE_BRANCH.to_owned()))
}

fn remote_head(root: &Path, spec: &str) -> Result<String, String> {
    let listed = git_stdout(root, &["ls-remote", "origin", spec])?;
    Ok(listed.split_whitespace().next().unwrap_or("").to_owned())
}

/// gh が無い・認証できないなら、run を読む前に止める（`release push` と同じ事前確認）。
fn ensure_gh(root: &Path) -> Result<(), String> {
    match ci_runs::gh(root, &["auth", "status"]) {
        Ok(_) => Ok(()),
        Err(err) if err.not_found => Err("gh が無いので run を確かめられません".to_owned()),
        Err(err) => Err(format!(
            "gh の認証ができないので run を確かめられません: {}",
            err.message
        )),
    }
}

fn check_help() -> String {
    let examples = ["release check"];
    Help::new(
        "release check",
        "Dispatch release.preflightWorkflow on the integration branch HEAD and wait for that run only. The HEAD must already be on origin.",
        "waits for GitHub Actions up to the tool's CI_WAIT constant",
        "dispatches one workflow run on GitHub; writes no local files",
    )
    .exit("0 when the dispatched run succeeds; 1 when HEAD is not on origin, the manifest lacks release.preflightWorkflow, gh is missing or not authenticated, the integration branch moved on origin across the dispatch, or the run fails")
    .examples(&examples)
    .render()
}

fn cannot_read(err: ListError) -> String {
    match err {
        ListError::Gh(err) if err.not_found => "gh が無いので run を確かめられません".to_owned(),
        ListError::Gh(err) => format!("run を確かめられません: {}", err.message),
        ListError::Parse(err) => format!("run を確かめられません: {err}"),
        ListError::LimitReached => "run が上限に達したため、自分の run を特定できません".to_owned(),
    }
}
