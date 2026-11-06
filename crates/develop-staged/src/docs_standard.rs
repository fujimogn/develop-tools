//! 島標準の TODO / CHANGELOG の判定は `island_docs::records` が 1 か所で持つ。ここは pre-commit
//! （`check-staged-docs`）と `archive-changelog` が使う入口と、CHANGELOG の退避だけを持つ。

use std::collections::{BTreeSet, HashSet};

pub use island_docs::records::{
    ChangelogStyle, Finding, FindingKind, TODO_ITEM_WARNING_BYTES, inspect_changelog,
    inspect_changelog_with_style, inspect_todo,
};
use island_docs::records::{changelog_item_date, is_month_heading, parse_month_heading};
use island_docs::{check::Input, git::Repo, markdown::fence_mask, records};

/// staged 本文を共通の parser / Git 証拠 / records 判定へ渡す。
/// 既存入口の容量診断と warning はそのまま保ち、共通判定の error は重複させない。
/// 呼出側は CHANGELOG が変更されたときだけ呼び、snapshot 読取前の HEAD を渡す。
pub fn inspect_changelog_with_git(
    content: &[u8],
    style: ChangelogStyle,
    repo: &Repo,
    basis: &Result<String, String>,
) -> Vec<Finding> {
    let mut findings = inspect_changelog_with_style(content, style);
    let Ok(text) = std::str::from_utf8(content) else {
        return findings;
    };
    let prefixes: BTreeSet<&str> = records::changelog_commit_links(text)
        .into_iter()
        .filter_map(|(_, link)| link.ok().map(|link| link.sha))
        .collect();
    if prefixes.is_empty() {
        return findings;
    }
    let mut input = Input {
        repo: repo.github_name(),
        changelog_stamp: style == ChangelogStyle::Compact,
        ..Input::default()
    };
    input
        .snapshot
        .add_markdown("CHANGELOG.md", content.to_vec());
    input.changelog_commits = match basis
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|basis| repo.commit_link_evidence(&prefixes, basis))
    {
        Ok(evidence) => evidence,
        Err(problem) => prefixes
            .into_iter()
            .map(|prefix| (prefix.to_owned(), Err(problem.clone())))
            .collect(),
    };
    let mut seen: HashSet<_> = findings
        .iter()
        .filter(|finding| finding.kind == FindingKind::Error)
        .map(|finding| (finding.line, finding.message.clone()))
        .collect();
    for found in records::check(&input) {
        if !seen.insert((found.line, found.message.clone())) {
            continue;
        }
        let finding = Finding {
            kind: FindingKind::Error,
            line: found.line,
            message: found.message,
        };
        findings.push(finding);
    }
    findings
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogArchivePlan {
    pub content: String,
    pub archived_months: Vec<String>,
    /// 保護月の中から退避した日付（古い月だけで上限に戻らないときだけ）。
    pub archived_dates: Vec<String>,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

// 上限値は島の docs 検査の定数表が正本（openspec island-rust-tool-release design §5.2）。
pub use island_docs::constants::{CHANGELOG_MAX_BYTES, TODO_MAX_BYTES};
pub const CHANGELOG_ARCHIVE_TARGET_PERCENT: usize = 80;

pub fn plan_changelog_archive(
    content: &str,
    current_month: &str,
    commit_sha: &str,
) -> Result<ChangelogArchivePlan, String> {
    let bytes_before = content.len();
    if bytes_before <= CHANGELOG_MAX_BYTES {
        return Ok(ChangelogArchivePlan {
            content: content.to_owned(),
            archived_months: Vec::new(),
            archived_dates: Vec::new(),
            bytes_before,
            bytes_after: bytes_before,
        });
    }

    let (preamble, mut sections) = parse_sections(content)?;
    let protects_current_month = sections
        .iter()
        .any(|section| section.month == current_month);
    let protected_month = if protects_current_month {
        current_month.to_owned()
    } else {
        sections
            .iter()
            .map(|section| section.month.as_str())
            .max()
            .expect("parse_sections returns at least one month")
            .to_owned()
    };

    let target = CHANGELOG_MAX_BYTES * CHANGELOG_ARCHIVE_TARGET_PERCENT / 100;
    let mut archived = Vec::new();
    loop {
        let candidate_preamble = archive_preamble(&preamble, newest(&archived), commit_sha);
        let candidate = render(&candidate_preamble, &sections);
        if candidate.len() <= target {
            return Ok(finished_plan(candidate, archived, Vec::new(), bytes_before));
        }

        let Some((index, _)) = sections
            .iter()
            .enumerate()
            .filter(|(_, section)| section.month != protected_month)
            .min_by(|(_, left), (_, right)| left.month.cmp(&right.month))
        else {
            if candidate.len() <= CHANGELOG_MAX_BYTES {
                return Ok(finished_plan(candidate, archived, Vec::new(), bytes_before));
            }
            if let Some(plan) = archive_old_dates(
                &preamble,
                &mut sections,
                &protected_month,
                archived.clone(),
                commit_sha,
                target,
                bytes_before,
            ) {
                return Ok(plan);
            }
            let message = if protects_current_month {
                "古い月をすべて退避しても上限に戻りません。当月の項目を畳む必要があります"
            } else {
                "古い月をすべて退避しても上限に戻りません。最新の月の項目を畳む必要があります"
            };
            return Err(message.to_owned());
        };
        archived.push(sections[index].month.clone());
        sections.remove(index);
    }
}

fn finished_plan(
    content: String,
    mut archived_months: Vec<String>,
    mut archived_dates: Vec<String>,
    bytes_before: usize,
) -> ChangelogArchivePlan {
    archived_months.sort();
    archived_dates.sort();
    let bytes_after = content.len();
    ChangelogArchivePlan {
        content,
        archived_months,
        archived_dates,
        bytes_before,
        bytes_after,
    }
}

fn newest(labels: &[String]) -> Option<&str> {
    labels.iter().max().map(String::as_str)
}

/// 古い月を尽くしても上限を超えるとき、保護月の項目を日付の古いものから日付単位で除く。
/// いちばん新しい日付と日付を読めない行は残す。上限に戻らなければ `None`。
fn archive_old_dates(
    preamble: &str,
    sections: &mut [MonthSection],
    protected_month: &str,
    archived_months: Vec<String>,
    commit_sha: &str,
    target: usize,
    bytes_before: usize,
) -> Option<ChangelogArchivePlan> {
    let index = sections
        .iter()
        .position(|section| section.month == protected_month)?;
    let mut segments = split_items(&sections[index].raw);
    let mut dates = segments
        .iter()
        .filter_map(|segment| segment.date.clone())
        .collect::<Vec<_>>();
    dates.sort();
    dates.dedup();
    dates.pop();

    let mut archived_dates = Vec::new();
    let mut last = None;
    for date in dates {
        segments.retain(|segment| segment.date.as_deref() != Some(date.as_str()));
        archived_dates.push(date);
        sections[index].raw = segments
            .iter()
            .map(|segment| segment.raw.as_str())
            .collect();
        let candidate = render(
            &archive_preamble(preamble, newest(&archived_dates), commit_sha),
            sections,
        );
        if candidate.len() <= target {
            return Some(finished_plan(
                candidate,
                archived_months,
                archived_dates,
                bytes_before,
            ));
        }
        last = Some(candidate);
    }
    last.filter(|candidate| candidate.len() <= CHANGELOG_MAX_BYTES)
        .map(|candidate| finished_plan(candidate, archived_months, archived_dates, bytes_before))
}

struct ItemSegment {
    date: Option<String>,
    raw: String,
}

/// 月節を、トップレベル項目とそれ以外の行に分ける。項目の続きは、字下げした行と、空行の後で
/// 字下げが続く行だけにする。字下げの無い項目以外の行（fence・見出し・本文）は日付なしで残す。
fn split_items(raw: &str) -> Vec<ItemSegment> {
    let records = raw.split_inclusive('\n').collect::<Vec<_>>();
    let lines = records
        .iter()
        .map(|record| record.trim_end_matches(['\r', '\n']))
        .collect::<Vec<_>>();
    let fenced = fence_mask(&lines);
    let indented = |line: &str| line.starts_with(' ') || line.starts_with('\t');
    let mut segments: Vec<ItemSegment> = Vec::new();
    for (index, record) in records.iter().enumerate() {
        let line = lines[index];
        let starts_item = !fenced[index] && line.starts_with("- ");
        let in_item = segments
            .last()
            .is_some_and(|segment| segment.date.is_some());
        let continues_item = !starts_item
            && in_item
            && if line.trim().is_empty() {
                lines[index + 1..]
                    .iter()
                    .find(|next| !next.trim().is_empty())
                    .is_some_and(|next| indented(next))
            } else {
                indented(line)
            };
        if continues_item {
            segments.last_mut().expect("checked").raw.push_str(record);
            continue;
        }
        segments.push(ItemSegment {
            date: starts_item.then(|| changelog_item_date(line)).flatten(),
            raw: (*record).to_owned(),
        });
    }
    segments
}

#[derive(Debug, Clone)]
struct MonthSection {
    month: String,
    raw: String,
}

fn parse_sections(content: &str) -> Result<(String, Vec<MonthSection>), String> {
    let records = content.split_inclusive('\n').collect::<Vec<_>>();
    let lines = records
        .iter()
        .map(|record| record.trim_end_matches(['\r', '\n']))
        .collect::<Vec<_>>();
    let fenced = fence_mask(&lines);
    let Some(first) = lines
        .iter()
        .enumerate()
        .position(|(index, line)| !fenced[index] && is_month_heading(line))
    else {
        return Err("CHANGELOG.md に ## YYYY-MM の月見出しがありません".to_owned());
    };
    let mut starts = Vec::new();
    for (index, line) in lines.iter().enumerate().skip(first) {
        if fenced[index] {
            continue;
        }
        if line.starts_with("##") && !line.starts_with("###") {
            let Some(month) = parse_month_heading(line) else {
                return Err(format!("CHANGELOG.md の見出しが不正です: {}", index + 1));
            };
            starts.push((index, month));
        }
    }
    let preamble = records[..first].concat();
    let sections = starts
        .iter()
        .enumerate()
        .map(|(position, (start, month))| {
            let end = starts
                .get(position + 1)
                .map(|(index, _)| *index)
                .unwrap_or(records.len());
            MonthSection {
                month: month.clone(),
                raw: records[*start..end].concat(),
            }
        })
        .collect();
    Ok((preamble, sections))
}

fn archive_preamble(preamble: &str, newest_archived: Option<&str>, commit_sha: &str) -> String {
    let Some(newest_archived) = newest_archived else {
        return preamble.to_owned();
    };
    let note = format!("> {newest_archived} 以前は git 履歴（退避直前の commit: {commit_sha}）");

    let records = preamble.split_inclusive('\n').collect::<Vec<_>>();
    if let Some(note_index) = records
        .iter()
        .position(|record| is_archive_history_note(record.trim_end_matches(['\r', '\n'])))
    {
        let mut output = String::new();
        for (index, record) in records.iter().enumerate() {
            let line = record.trim_end_matches(['\r', '\n']);
            if is_archive_history_note(line) {
                if index == note_index {
                    output.push_str(&note);
                    if record.ends_with("\r\n") {
                        output.push_str("\r\n");
                    } else if record.ends_with('\n') {
                        output.push('\n');
                    }
                }
                continue;
            }
            output.push_str(record);
        }
        return output;
    }

    let preamble = preamble.trim_end_matches(['\r', '\n']);
    if preamble.is_empty() {
        format!("# Changelog\n\n{note}\n\n")
    } else {
        format!("{preamble}\n\n{note}\n\n")
    }
}

fn is_archive_history_note(line: &str) -> bool {
    let normalized = line.trim().to_ascii_lowercase();
    normalized.starts_with('>')
        && normalized.contains("git")
        && (line.contains("履歴") || normalized.contains("history"))
}

fn render(preamble: &str, sections: &[MonthSection]) -> String {
    let mut content = preamble.to_owned();
    for section in sections {
        content.push_str(&section.raw);
    }
    content
}

#[cfg(test)]
mod tests;
