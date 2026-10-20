//! 島標準の TODO.md / CHANGELOG.md の形式（`docs/develop-docs.md`、spec `island-docs-standard`）。
//!
//! 判定の実装はここ 1 か所で、`develop-health` の `check-staged-docs`（pre-commit）と
//! `develop-docs-check`（[`check`]）が同じ関数を呼ぶ。容量上限の値は [`crate::constants`] の表。
//! TODO.md の見出しの語彙と順（[`todo_headings`]、2026-09-30 決定）は `check-staged-docs` と
//! `develop-docs-check` の両方が使う。移設元の `check-docs` の `records.rs` から移した項目の形（取り消し線・
//! 太字の名前・親のない子項目）は [`todo_layout`] に分けて、`develop-docs-check` だけが使う（design §5.5）。

use std::str;

use crate::check::Input;
use crate::constants::{
    CHANGELOG_MAX_BYTES, CHANGELOG_PATH, TODO_MAX_BYTES, TODO_PATH, TODO_SECTIONS,
};
use crate::markdown::fence_mask;
use crate::rules::{Rule, RuleFinding};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub kind: FindingKind,
    pub line: Option<usize>,
    pub message: String,
}

pub const TODO_ITEM_WARNING_BYTES: usize = 500;

fn item_body_bytes(lines: &[&str], start: usize, end: usize) -> usize {
    let mut item_end = end;
    while item_end > start && lines[item_end - 1].trim_end_matches('\r').is_empty() {
        item_end -= 1;
    }
    lines[start..item_end]
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            let body = if offset == item_end - start - 1 {
                line.trim_end_matches('\r')
            } else {
                line
            };
            body.len() + usize::from(offset > 0)
        })
        .sum()
}

pub fn inspect_todo(content: &[u8]) -> Vec<Finding> {
    let Ok(text) = str::from_utf8(content) else {
        return vec![finding(
            FindingKind::Error,
            None,
            "文書が UTF-8 ではありません",
        )];
    };

    let mut findings = Vec::new();
    if content.len() > TODO_MAX_BYTES {
        findings.push(finding(
            FindingKind::Error,
            None,
            format!(
                "TODO.md が上限 {TODO_MAX_BYTES} bytes を超えています: {} bytes",
                content.len()
            ),
        ));
    }
    findings.extend(todo_format(text));
    findings.extend(
        todo_headings(text)
            .into_iter()
            .map(|(line, message)| finding(FindingKind::Error, Some(line), message)),
    );
    findings.sort_by_key(|finding| finding.line);
    findings
}

/// TODO.md の形式（容量上限は含めない）。`- [x]` は error、500 byte 超の項目は warning。
pub fn todo_format(text: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let lines = text.split('\n').collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim_start_matches([' ', '\t']);
        if trimmed.starts_with("- [x]") || trimmed.starts_with("- [X]") {
            findings.push(finding(
                FindingKind::Error,
                Some(line_number),
                "TODO.md に完了済みの - [x] 項目を残せません（未完了だけを置いてください）",
            ));
        }
        if !line.starts_with("- ") {
            continue;
        }

        let end = lines
            .iter()
            .enumerate()
            .skip(index + 1)
            .find(|(_, continuation)| {
                continuation.starts_with("- ")
                    || continuation
                        .trim_start_matches([' ', '\t'])
                        .starts_with('#')
            })
            .map(|(continuation_index, _)| continuation_index)
            .unwrap_or(lines.len());
        let bytes = item_body_bytes(&lines, index, end);
        if bytes > TODO_ITEM_WARNING_BYTES {
            findings.push(finding(
                FindingKind::Warning,
                Some(line_number),
                format!(
                    "TODO.md の項目が warning 目安 {TODO_ITEM_WARNING_BYTES} bytes を超えています: {bytes} bytes"
                ),
            ));
        }
    }
    findings
}

pub fn inspect_changelog(content: &[u8]) -> Vec<Finding> {
    let Ok(text) = str::from_utf8(content) else {
        return vec![finding(
            FindingKind::Error,
            None,
            "文書が UTF-8 ではありません",
        )];
    };

    let mut findings = Vec::new();
    if content.len() > CHANGELOG_MAX_BYTES {
        findings.push(finding(
            FindingKind::Error,
            None,
            format!(
                "CHANGELOG.md が上限 {CHANGELOG_MAX_BYTES} bytes を超えています: {} bytes。archive-changelog を実行してください",
                content.len()
            ),
        ));
    }

    findings.extend(changelog_format(text));
    findings
}

/// CHANGELOG.md の形式（容量上限は含めない）。
pub fn changelog_format(text: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let lines = text.split('\n').collect::<Vec<_>>();
    if !lines
        .first()
        .is_some_and(|line| line.trim_end_matches('\r').starts_with("# Changelog"))
    {
        findings.push(finding(
            FindingKind::Error,
            Some(1),
            "CHANGELOG.md の1行目は # Changelog で始めてください",
        ));
    }

    let fenced = fence_mask(&lines);
    let first_heading = lines
        .iter()
        .enumerate()
        .position(|(index, line)| !fenced[index] && is_month_heading(line));
    if let Some(index) = first_heading {
        let has_history_note = lines[..index].iter().any(|line| {
            let normalized = line.trim().to_ascii_lowercase();
            normalized.starts_with('>')
                && normalized.contains("git")
                && (line.contains("履歴") || normalized.contains("history"))
        });
        if !has_history_note {
            findings.push(finding(
                FindingKind::Error,
                Some(index + 1),
                "最初の月見出しより前に、古い分を git 履歴に任せる注記が必要です",
            ));
        }
    } else {
        findings.push(finding(
            FindingKind::Error,
            None,
            "CHANGELOG.md に ## YYYY-MM の月見出しがありません",
        ));
    }

    let mut current_month = None;
    let mut previous_month = None;
    let mut previous_date: Option<String> = None;
    let mut item_indices = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let line_number = index + 1;
        if fenced[index] {
            continue;
        }
        if line.starts_with("##") {
            match parse_month_heading(line) {
                Some(month) => {
                    if let Some(previous) = previous_month.as_deref() {
                        if previous < month.as_str() {
                            findings.push(finding(
                                FindingKind::Error,
                                Some(line_number),
                                "CHANGELOG.md の月見出しは新しい月から並べてください",
                            ));
                        }
                    }
                    previous_month = Some(month.clone());
                    current_month = Some(month);
                    previous_date = None;
                }
                None => findings.push(finding(
                    FindingKind::Error,
                    Some(line_number),
                    "CHANGELOG.md の見出しは ## YYYY-MM だけにしてください",
                )),
            }
            continue;
        }
        if !line.starts_with("- ") {
            continue;
        }
        item_indices.push(index);
        let Some(month) = current_month.as_deref() else {
            findings.push(finding(
                FindingKind::Error,
                Some(line_number),
                "CHANGELOG.md の成果項目は月見出しの下に置いてください",
            ));
            continue;
        };
        match parse_changelog_item(line) {
            Ok(date) => {
                if date.get(..7) != Some(month) {
                    findings.push(finding(
                        FindingKind::Error,
                        Some(line_number),
                        format!("成果項目の日付の年月が見出し {month} と一致しません"),
                    ));
                } else {
                    if previous_date
                        .as_deref()
                        .is_some_and(|previous| previous < date.as_str())
                    {
                        findings.push(finding(
                            FindingKind::Error,
                            Some(line_number),
                            "成果項目は月内で新しい日付から並べてください",
                        ));
                    }
                    previous_date = Some(date);
                }
            }
            Err(message) => findings.push(finding(FindingKind::Error, Some(line_number), message)),
        }
    }

    for (position, index) in item_indices.iter().enumerate() {
        let next_item = item_indices.get(position + 1).copied();
        let next_heading = lines
            .iter()
            .enumerate()
            .skip(*index + 1)
            .take_while(|(line_index, _)| next_item.is_none_or(|next_item| *line_index < next_item))
            .find(|(index, line)| !fenced[*index] && line.starts_with("##"))
            .map(|(index, _)| index);
        let end = next_item
            .into_iter()
            .chain(next_heading)
            .min()
            .unwrap_or(lines.len());
        let bytes = item_body_bytes(&lines, *index, end);
        if bytes > TODO_ITEM_WARNING_BYTES {
            findings.push(finding(
                FindingKind::Warning,
                Some(index + 1),
                format!(
                    "CHANGELOG.md の項目が warning 目安 {TODO_ITEM_WARNING_BYTES} bytes を超えています: {bytes} bytes"
                ),
            ));
        }
    }
    findings
}

fn finding(kind: FindingKind, line: Option<usize>, message: impl Into<String>) -> Finding {
    Finding {
        kind,
        line,
        message: message.into(),
    }
}

pub fn is_month_heading(line: &str) -> bool {
    line.starts_with("## ") && parse_month_heading(line).is_some()
}

pub fn parse_month_heading(line: &str) -> Option<String> {
    let month = line.strip_prefix("## ")?.trim_end_matches('\r');
    if month.len() != 7
        || month.as_bytes().get(4) != Some(&b'-')
        || !month
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 4 || byte.is_ascii_digit())
    {
        return None;
    }
    let month_number = month.get(5..7)?.parse::<u8>().ok()?;
    if !(1..=12).contains(&month_number) {
        return None;
    }
    Some(month.to_owned())
}

fn parse_changelog_item(line: &str) -> Result<String, String> {
    let Some(rest) = line.strip_prefix("- **") else {
        return Err(
            "CHANGELOG.md のトップレベル項目は - **成果（YYYY-MM-DD）**: 要約 の形にしてください"
                .to_owned(),
        );
    };
    let Some(marker) = rest.find("**:") else {
        return Err(
            "CHANGELOG.md のトップレベル項目は - **成果（YYYY-MM-DD）**: 要約 の形にしてください"
                .to_owned(),
        );
    };
    let title = &rest[..marker];
    let summary = rest[marker + 3..].trim();
    if summary.is_empty() {
        return Err("CHANGELOG.md の成果項目には要約が必要です".to_owned());
    }
    let Some(open) = title.rfind('（') else {
        return Err("CHANGELOG.md の成果項目に全角括弧の日付がありません".to_owned());
    };
    if !title.ends_with('）') {
        return Err("CHANGELOG.md の成果項目の日付括弧が閉じていません".to_owned());
    }
    let date_and_note = &title[open + '（'.len_utf8()..title.len() - '）'.len_utf8()];
    let date = date_and_note.get(..10).unwrap_or_default();
    if !valid_date(date) || (date_and_note.len() > 10 && !date_and_note[10..].starts_with('、')) {
        return Err(
            "CHANGELOG.md の成果項目の日付は YYYY-MM-DD（任意の補足）にしてください".to_owned(),
        );
    }
    Ok(date.to_owned())
}

pub fn valid_date(date: &str) -> bool {
    if !(date.len() == 10
        && date.as_bytes().get(4) == Some(&b'-')
        && date.as_bytes().get(7) == Some(&b'-')
        && date
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit()))
    {
        return false;
    }

    let Ok(year) = date[..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = date[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = date[8..10].parse::<u32>() else {
        return false;
    };
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return false,
    };
    year > 0 && (1..=days_in_month).contains(&day)
}

/// TODO.md の `##` 見出しの語彙と順（[`TODO_SECTIONS`]）。語彙の外・順番違い・重複を
/// 返す。項目の無い（見出しだけの）セクションは許す（2026-10-09 決定）。fence の中は見ない。`###` 以下は見ない。
pub fn todo_headings(text: &str) -> Vec<(usize, &'static str)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut problems = Vec::new();
    let mut last_section = None;
    for (index, (line, fenced)) in lines.iter().zip(fence_mask(&lines)).enumerate() {
        if fenced {
            continue;
        }
        let Some(name) = line.strip_prefix("## ") else {
            continue;
        };
        let line_number = index + 1;
        match TODO_SECTIONS.iter().position(|section| *section == name.trim()) {
            None => problems.push((
                line_number,
                "TODO.md の見出しは '## 進行中' / '## 次に着手' / '## 待ち' / '## いつか' だけにしてください（分野は項目名の先頭に書く）",
            )),
            Some(position) if last_section.is_some_and(|last| position <= last) => problems.push((
                line_number,
                "TODO.md の見出しは 進行中 → 次に着手 → 待ち → いつか の順に 1 回ずつ置いてください",
            )),
            Some(position) => last_section = Some(position),
        }
    }
    problems.sort_by_key(|(line, _)| *line);
    problems
}

/// TODO.md の項目の形（移設元: `check-docs` の `records.rs`）。島標準の「未完了だけ・
/// 1 項目 1 完了条件」を、`- [ ] **成果単位の名前**`、取り消し線の禁止、親のない子項目の禁止として
/// 機械で見る。見出しの規則は [`todo_headings`]。fence の中は見ない。
pub fn todo_layout(text: &str) -> Vec<(usize, &'static str)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut problems = todo_headings(text);
    let mut has_parent = false;
    for (index, (line, fenced)) in lines.iter().zip(fence_mask(&lines)).enumerate() {
        if fenced {
            continue;
        }
        let line_number = index + 1;
        if line.starts_with('#') {
            has_parent = false;
        }
        if line.starts_with("- ~~") || line.starts_with("  - ~~") {
            problems.push((
                line_number,
                "取り消し線の項目があります。完了なら CHANGELOG.md へ移してください",
            ));
        } else if line.starts_with("- [ ] **") {
            has_parent = true;
        } else if line.starts_with("- [ ] ") {
            problems.push((
                line_number,
                "項目は '- [ ] **成果単位の名前**' で始めてください",
            ));
        } else if line.starts_with("  - ") && !has_parent {
            problems.push((line_number, "親項目のない子箇条書きです"));
        }
    }
    problems.sort_by_key(|(line, _)| *line);
    problems
}

/// repo 直下の TODO.md / CHANGELOG.md の形式を検査する（容量上限は [`crate::check`] が表で見る）。
/// 500 byte 超の項目の warning は pre-commit（`check-staged-docs`）だけが出し、ここでは数えない。
pub fn check(input: &Input) -> Vec<RuleFinding> {
    let mut findings = Vec::new();
    for (path, text) in input.snapshot.markdown() {
        let Ok(text) = text else { continue };
        let (rule, errors) = match path {
            TODO_PATH => (Rule::Todo, todo_format(text)),
            CHANGELOG_PATH => (Rule::Changelog, changelog_format(text)),
            _ => continue,
        };
        for finding in errors
            .into_iter()
            .filter(|finding| finding.kind == FindingKind::Error)
        {
            let found = RuleFinding::new(rule, path, finding.message);
            findings.push(match finding.line {
                Some(line) => found.at(line),
                None => found,
            });
        }
        if rule == Rule::Todo {
            for (line, message) in todo_layout(text) {
                findings.push(RuleFinding::new(Rule::TodoLayout, path, message).at(line));
            }
        }
    }
    findings.sort_by_key(|finding| (finding.path.clone(), finding.line));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(todo: &str, changelog: &str) -> Vec<String> {
        let mut input = Input::default();
        input
            .snapshot
            .add_markdown(TODO_PATH, todo.as_bytes().to_vec());
        input
            .snapshot
            .add_markdown(CHANGELOG_PATH, changelog.as_bytes().to_vec());
        check(&input).iter().map(ToString::to_string).collect()
    }

    const TODO: &str = "# TODO\n\n## 進行中\n\n- [ ] **項目**。現在地。\n  - 補足\n";
    const CHANGELOG: &str = "# Changelog\n\n> 古い分は git 履歴に任せる。\n\n## 2026-09\n\n- **成果（2026-09-30）**: 要約。\n";

    #[test]
    fn valid_records_pass() {
        assert!(findings(TODO, CHANGELOG).is_empty());
        // fence の中の例は見ない。
        let fenced = format!("{TODO}\n```text\n## 例\n  - 例\n- [ ] 例\n```\n");
        assert!(findings(&fenced, CHANGELOG).is_empty());
    }

    #[test]
    fn crlf_todo_item_warning_excludes_the_final_line_ending() {
        for (body_bytes, warning) in [
            (TODO_ITEM_WARNING_BYTES, false),
            (TODO_ITEM_WARNING_BYTES + 1, true),
        ] {
            let item = format!("- {}\r\n", "a".repeat(body_bytes - 2));
            let found = inspect_todo(item.as_bytes());
            assert_eq!(
                found
                    .iter()
                    .any(|finding| finding.kind == FindingKind::Warning),
                warning,
                "{body_bytes} byte item: {found:?}"
            );
        }
    }

    #[test]
    fn crlf_todo_continuation_counts_its_line_ending_bytes() {
        for (first_line_bytes, warning) in [(495, false), (496, true)] {
            let item = format!("- {}\r\nb\r\n", "a".repeat(first_line_bytes));
            let found = inspect_todo(item.as_bytes());
            assert_eq!(
                found
                    .iter()
                    .any(|finding| finding.kind == FindingKind::Warning),
                warning,
                "{first_line_bytes} byte first line: {found:?}"
            );
        }
    }

    /// 4 語を順に、項目付きで使えば通る。全部は要らない。`###` と fence の中は見ない。
    #[test]
    fn todo_sections_in_order_pass() {
        let all = "# TODO\n\n## 進行中\n\n- [ ] **a**\n\n## 次に着手\n\n- [ ] **b**\n\n### 補足\n\n## 待ち\n\n- [ ] **c**\n\n## いつか\n\n- [ ] **d**\n";
        assert!(findings(all, CHANGELOG).is_empty());
        let some = "# TODO\n\n## 次に着手\n\n- [ ] **b**\n\n## いつか\n\n- [ ] **d**\n\n```text\n## Stage 1 — 例\n```\n";
        assert!(findings(some, CHANGELOG).is_empty());
    }

    /// 項目の無い（見出しだけの）セクションは許す（2026-10-09 決定）。語彙と順は引き続き守る。
    #[test]
    fn a_heading_without_items_is_allowed() {
        let empty_last = format!("{TODO}\n## 待ち\n\n");
        assert!(findings(&empty_last, CHANGELOG).is_empty());
        let empty_middle = format!("{TODO}\n## 待ち\n\n### 補足\n\n## いつか\n\n- [ ] **x**\n");
        assert!(findings(&empty_middle, CHANGELOG).is_empty());
        let wrong_order = format!("{TODO}\n## いつか\n\n## 待ち\n\n");
        assert_eq!(findings(&wrong_order, CHANGELOG).len(), 1);
    }

    /// 見出しの規則は、1 つずつ壊すと落ちる。`check-staged-docs` の [`inspect_todo`] も同じ判定で落ちる。
    #[test]
    fn each_todo_heading_rule_fails_on_its_own() {
        let cases = [
            ("\n## Stage 1 — 今\n\n- [ ] **x**\n", "だけにしてください"),
            ("\n## Now\n\n- [ ] **x**\n", "だけにしてください"),
            ("\n## 🚧 待ち\n\n- [ ] **x**\n", "だけにしてください"),
            (
                "\n## いつか\n\n- [ ] **x**\n\n## 待ち\n\n- [ ] **y**\n",
                "の順に 1 回ずつ",
            ),
            ("\n## 進行中\n\n- [ ] **x**\n", "の順に 1 回ずつ"),
        ];
        for (append, needle) in cases {
            let todo = format!("{TODO}{append}");
            let found = findings(&todo, CHANGELOG);
            assert_eq!(found.len(), 1, "{append:?}: {found:?}");
            assert!(found[0].contains("[todo-layout]"), "{found:?}");
            assert!(found[0].contains(needle), "{needle}: {found:?}");
            let staged = inspect_todo(todo.as_bytes());
            assert_eq!(staged.len(), 1, "{append:?}: {staged:?}");
            assert_eq!(staged[0].kind, FindingKind::Error);
            assert!(staged[0].message.contains(needle), "{needle}: {staged:?}");
        }
    }

    /// 移設元由来の項目の各規則は、1 つずつ壊すと落ちる。
    #[test]
    fn each_todo_layout_rule_fails_on_its_own() {
        let cases = [
            ("- ~~**終わった**~~\n", "取り消し線"),
            ("- [ ] 太字の無い項目\n", "成果単位の名前"),
            (
                "\n## 待ち\n\n  - 孤立した補足\n- [ ] **x**\n",
                "親項目のない子箇条書き",
            ),
            ("- [x] **終わった**\n", "- [x] 項目を残せません"),
        ];
        for (append, needle) in cases {
            let found = findings(&format!("{TODO}{append}"), CHANGELOG);
            assert_eq!(found.len(), 1, "{append:?}: {found:?}");
            assert!(found[0].starts_with("TODO.md:"), "{found:?}");
            assert!(found[0].contains(needle), "{needle}: {found:?}");
        }
    }

    /// 島標準の CHANGELOG の形（移設元の `- **要約**: 本文` より厳しい。日付と月の一致まで見る）。
    #[test]
    fn changelog_follows_the_island_standard() {
        let cases = [
            (
                "\n## 2026-10\n\n- **後の月（2026-10-01）**: 本文。\n",
                "新しい月から",
            ),
            ("\n## v0.1\n", "## YYYY-MM だけ"),
            ("- 要約の無い項目\n", "- **成果（YYYY-MM-DD）**: 要約"),
            (
                "- **月違い（2026-08-01）**: 本文。\n",
                "見出し 2026-09 と一致しません",
            ),
        ];
        for (append, needle) in cases {
            let found = findings(TODO, &format!("{CHANGELOG}{append}"));
            assert_eq!(found.len(), 1, "{append:?}: {found:?}");
            assert!(found[0].contains(needle), "{needle}: {found:?}");
        }
    }

    /// 容量上限は表の検査が出すので、ここでは二重に数えない。
    #[test]
    fn size_is_not_reported_by_the_format_check() {
        let long = format!("{TODO}{}", "- [ ] **x**\n".repeat(3_001));
        assert!(long.len() > TODO_MAX_BYTES);
        assert!(findings(&long, CHANGELOG).is_empty());
    }
}
