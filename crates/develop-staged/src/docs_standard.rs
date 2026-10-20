//! 島標準の TODO / CHANGELOG の判定は `island_docs::records` が 1 か所で持つ。ここは pre-commit
//! （`check-staged-docs`）と `archive-changelog` が使う入口と、CHANGELOG の退避だけを持つ。

use island_docs::markdown::fence_mask;
pub use island_docs::records::{
    Finding, FindingKind, TODO_ITEM_WARNING_BYTES, inspect_changelog, inspect_todo,
};
use island_docs::records::{is_month_heading, parse_month_heading};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangelogArchivePlan {
    pub content: String,
    pub archived_months: Vec<String>,
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
        let candidate_preamble = archive_preamble(&preamble, &archived, commit_sha);
        let candidate = render(&candidate_preamble, &sections);
        if candidate.len() <= target {
            let mut archived_months = archived;
            archived_months.sort();
            return Ok(ChangelogArchivePlan {
                content: candidate.clone(),
                archived_months,
                bytes_before,
                bytes_after: candidate.len(),
            });
        }

        let Some((index, _)) = sections
            .iter()
            .enumerate()
            .filter(|(_, section)| section.month != protected_month)
            .min_by(|(_, left), (_, right)| left.month.cmp(&right.month))
        else {
            if candidate.len() <= CHANGELOG_MAX_BYTES {
                let mut archived_months = archived;
                archived_months.sort();
                return Ok(ChangelogArchivePlan {
                    content: candidate.clone(),
                    archived_months,
                    bytes_before,
                    bytes_after: candidate.len(),
                });
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

fn archive_preamble(preamble: &str, archived: &[String], commit_sha: &str) -> String {
    if archived.is_empty() {
        return preamble.to_owned();
    }
    let mut months = archived.to_vec();
    months.sort();
    let note = format!(
        "> {} 以前は git 履歴（退避直前の commit: {commit_sha}）",
        months.last().expect("archived is non-empty")
    );

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
