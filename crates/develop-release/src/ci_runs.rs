//! GitHub Actions の run の取得・待ち・時間の上限。`release push` と `release check` が共有する。
//!
//! 絞り込み（event・workflow・branch・除く run ID）は [`RunFilter`] で呼び出し側が決める。
//! 待ち方は 1 つだけにし、呼び出しごとの文言は呼び出し側が [`ListError`] から作る。

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// run の出現と完了を待つ上限。出現しない場合も、現れたまま終わらない場合も、この値だけを使う。
pub const CI_WAIT: Duration = Duration::from_secs(30 * 60);

/// 完了に見えた run を、もう一度見るまでの間隔。待ちの上限は [`CI_WAIT`]。
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);
pub const RUN_LIST_LIMIT: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Run {
    #[serde(rename = "databaseId")]
    pub database_id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub event: String,
    #[serde(rename = "headSha")]
    pub head_sha: String,
    #[serde(rename = "headBranch")]
    pub head_branch: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Job {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: String,
}

#[derive(Debug, serde::Deserialize)]
struct Jobs {
    jobs: Vec<Job>,
}

pub struct GhError {
    pub not_found: bool,
    pub message: String,
}

pub enum ListError {
    Gh(GhError),
    Parse(String),
    /// 1 回の取得が上限に達し、すべての run を見られない。
    LimitReached,
}

pub enum WaitError {
    List(ListError),
    /// 上限の時間内に、絞り込みに当たる run が完了しなかった（現れなかった場合を含む）。
    Timeout {
        saw_run: bool,
        pending: Vec<Run>,
    },
}

/// `gh run list` の絞り込み。`workflow` と `branch` は `None` なら条件にしない。
pub struct RunFilter<'a> {
    pub sha: &'a str,
    pub event: &'a str,
    pub workflow: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub exclude: &'a BTreeSet<u64>,
}

pub fn list_runs(root: &Path, filter: &RunFilter) -> Result<Vec<Run>, ListError> {
    let limit = RUN_LIST_LIMIT.to_string();
    let mut args = vec!["run", "list"];
    if let Some(workflow) = filter.workflow {
        args.extend(["--workflow", workflow]);
    }
    args.extend(["--commit", filter.sha, "--event", filter.event]);
    if let Some(branch) = filter.branch {
        args.extend(["--branch", branch]);
    }
    args.extend([
        "--limit",
        &limit,
        "--json",
        "databaseId,name,status,conclusion,event,headSha,headBranch,url",
    ]);
    let stdout = gh(root, &args).map_err(ListError::Gh)?;
    let parsed: Vec<Run> =
        serde_json::from_str(&stdout).map_err(|err| ListError::Parse(err.to_string()))?;
    if parsed.len() == RUN_LIST_LIMIT {
        return Err(ListError::LimitReached);
    }
    Ok(parsed
        .into_iter()
        .filter(|run| {
            run.event == filter.event
                && run.head_sha == filter.sha
                && filter.branch.is_none_or(|branch| run.head_branch == branch)
                && !filter.exclude.contains(&run.database_id)
        })
        .collect())
}

pub fn view_jobs(root: &Path, run_id: u64) -> Result<Vec<Job>, ListError> {
    let id = run_id.to_string();
    let stdout = gh(root, &["run", "view", &id, "--json", "jobs"]).map_err(ListError::Gh)?;
    let parsed: Jobs =
        serde_json::from_str(&stdout).map_err(|err| ListError::Parse(err.to_string()))?;
    Ok(parsed.jobs)
}

/// 絞り込みに当たる run がすべて完了し、続けて 2 回同じ結果に見えるまで待つ。
/// `now` は呼び出しの起点からの経過で、[`CI_WAIT`] を超えたら止める。
pub fn wait_for_runs<N, S>(
    root: &Path,
    filter: &RunFilter,
    now: &mut N,
    sleep: &mut S,
) -> Result<Vec<Run>, WaitError>
where
    N: FnMut() -> Duration,
    S: FnMut(Duration),
{
    let mut previous: Option<Vec<(u64, String, Option<String>)>> = None;
    let mut saw_run = false;
    let mut last_pending = Vec::new();
    loop {
        let runs = list_runs(root, filter).map_err(WaitError::List)?;
        if !runs.is_empty() {
            saw_run = true;
        }
        let pending: Vec<Run> = runs.iter().filter(|run| is_pending(run)).cloned().collect();
        if !runs.is_empty() && pending.is_empty() {
            let signature = signature_of(&runs);
            if previous.as_ref() == Some(&signature) {
                return Ok(runs);
            }
            previous = Some(signature);
        } else {
            previous = None;
            if !pending.is_empty() {
                last_pending = pending;
            }
        }
        let timeout = || WaitError::Timeout {
            saw_run,
            pending: last_pending.clone(),
        };
        if now() >= CI_WAIT {
            return Err(timeout());
        }
        let step = POLL_INTERVAL.min(CI_WAIT.saturating_sub(now()));
        if step.is_zero() {
            return Err(timeout());
        }
        sleep(step);
    }
}

pub fn out_of_time(saw_run: bool, pending: &[Run]) -> String {
    if !saw_run {
        return "run が無い".to_owned();
    }
    let mut lines = vec!["時間切れ".to_owned()];
    for run in pending {
        lines.push(format!(
            "{} #{} は {} {}",
            run.name, run.database_id, run.status, run.url
        ));
    }
    lines.join("\n")
}

/// すべて success なら成功。skipped と neutral も失敗として conclusion を報告する。
pub fn decide(runs: &[Run]) -> Result<(), String> {
    let mut lines = Vec::new();
    for run in runs {
        match run.conclusion.as_deref() {
            Some("success") => {}
            Some("cancelled") => lines.push(format!(
                "CI が取消されました: {} #{} {}",
                run.name, run.database_id, run.url
            )),
            Some(conclusion) => lines.push(format!(
                "CI が失敗しました: {} #{} conclusion={conclusion} {}",
                run.name, run.database_id, run.url
            )),
            None => lines.push(format!(
                "CI が失敗しました: {} #{} conclusion=無し {}",
                run.name, run.database_id, run.url
            )),
        }
    }
    if lines.is_empty() {
        Ok(())
    } else {
        Err(lines.join("\n"))
    }
}

fn is_pending(run: &Run) -> bool {
    matches!(
        run.status.as_str(),
        "queued" | "in_progress" | "requested" | "waiting" | "pending" | "action_required" | ""
    )
}

fn signature_of(runs: &[Run]) -> Vec<(u64, String, Option<String>)> {
    let mut items: Vec<_> = runs
        .iter()
        .map(|run| (run.database_id, run.status.clone(), run.conclusion.clone()))
        .collect();
    items.sort();
    items
}

pub fn gh(root: &Path, args: &[&str]) -> Result<String, GhError> {
    let mut command = Command::new("gh");
    develop_tools_core::strip_git_env(&mut command);
    let output = match command.args(args).current_dir(root).output() {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(GhError {
                not_found: true,
                message: err.to_string(),
            });
        }
        Err(err) => {
            return Err(GhError {
                not_found: false,
                message: err.to_string(),
            });
        }
    };
    if !output.status.success() {
        return Err(GhError {
            not_found: false,
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
