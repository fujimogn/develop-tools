//! ADR の構造と status 語彙（島の ADR 標準）。
//!
//! 文書 1 つの判定（[`document_problem`]）と file 名の判定（[`adr_file_number`]）は、
//! `develop-health` の ADR lane（`check-project-integrity` / staged ADR）と**この 1 か所を共有する**。
//! 規則は島の ADR 標準の正本（`validAdrDocument` / `adrFileNumber`）と同じ。
//! 移設元の `check-docs` の `adr.rs` との差（design §5.5）: status は `superseded` + `superseded-by`
//! ではなく `superseded by ADR-NNNN` の 1 語、MADR の見出し 6 つではなく `Chosen option:` と
//! `- Good:` / `- Bad:` / `- Neutral:` の行、索引の状態欄の照合と YAML 構文の検査はしない。

use std::collections::BTreeMap;
use std::fmt;

use crate::check::Input;
use crate::constants::{ADR_DIR, ADR_NON_RECORD_FILES, ADR_STATUSES, ADR_SUPERSEDED_PREFIX};
use crate::docs::index_entry_counts;
use crate::rules::{Rule, RuleFinding};

/// ADR 文書が標準構造を満たさない理由（最初に見つかった 1 つ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdrProblem {
    MissingFrontmatter,
    MissingStatus,
    InvalidStatus(String),
    MissingDate,
    InvalidDate(String),
    MissingTitle(String),
    Placeholder,
    MissingChosenOption,
    MissingConsequence(&'static str),
}

impl fmt::Display for AdrProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => f.write_str("先頭に --- で閉じた frontmatter がありません"),
            Self::MissingStatus => f.write_str("frontmatter に status がありません"),
            Self::InvalidStatus(status) => write!(
                f,
                "status が島の語彙にありません ({status})。{} か '{ADR_SUPERSEDED_PREFIX}NNNN' にしてください",
                ADR_STATUSES.join(" / ")
            ),
            Self::MissingDate => f.write_str("frontmatter に date がありません"),
            Self::InvalidDate(date) => write!(f, "date は YYYY-MM-DD にしてください ({date})"),
            Self::MissingTitle(number) => write!(f, "'# ADR-{number}: ' の見出しがありません"),
            Self::Placeholder => f.write_str("template の {...} が残っています"),
            Self::MissingChosenOption => f.write_str("'Chosen option:' の行がありません"),
            Self::MissingConsequence(kind) => write!(f, "'- {kind}:' の行がありません"),
        }
    }
}

/// `NNNN-kebab-case.md` か。合致すれば 4 桁の番号文字列を返す。
pub fn adr_file_number(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".md")?;
    let (number, rest) = stem.split_at_checked(4)?;
    if number.len() != 4 || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let slug = rest.strip_prefix('-')?;
    if slug.is_empty() {
        return None;
    }
    // kebab-case: 英数字の語を `-` でつなぐ。空の語（`--` や末尾 `-`）は不可。
    if slug.split('-').any(|word| {
        word.is_empty()
            || !word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    }) {
        return None;
    }
    Some(number)
}

/// ADR 文書が標準構造を満たすか。満たさなければ最初の理由を返す。
///
/// `## Original Record` より前だけを見る（移行済み ADR は原文をそのまま抱えており、
/// その中身に標準構造の判定を掛けると必ず落ちる）。
pub fn document_problem(content: &str, number: &str) -> Option<AdrProblem> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let standard = normalized
        .split('\n')
        .position(|line| line.trim_end() == "## Original Record")
        .map(|index| {
            normalized
                .split('\n')
                .take(index)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| normalized.clone());

    let Some(frontmatter) = extract_frontmatter(&standard) else {
        return Some(AdrProblem::MissingFrontmatter);
    };
    let Some(status) = field(&frontmatter, "status") else {
        return Some(AdrProblem::MissingStatus);
    };
    if !is_valid_status(&status) {
        return Some(AdrProblem::InvalidStatus(status));
    }
    let Some(date) = field(&frontmatter, "date") else {
        return Some(AdrProblem::MissingDate);
    };
    if !is_valid_date(&date) {
        return Some(AdrProblem::InvalidDate(date));
    }
    if !standard.contains(&format!("# ADR-{number}: ")) {
        return Some(AdrProblem::MissingTitle(number.to_owned()));
    }
    // 未記入の placeholder（`{...}`）が本文に残っていないか。code fence と inline code は除く。
    if has_placeholder(&strip_code(&standard)) {
        return Some(AdrProblem::Placeholder);
    }
    if !standard
        .lines()
        .any(|line| line.starts_with("Chosen option:") && line.len() > "Chosen option:".len() + 1)
    {
        return Some(AdrProblem::MissingChosenOption);
    }
    for consequence in ["Good", "Bad", "Neutral"] {
        let prefix = format!("- {consequence}:");
        if !standard
            .lines()
            .any(|line| line.starts_with(&prefix) && line.trim().len() > prefix.len())
        {
            return Some(AdrProblem::MissingConsequence(consequence));
        }
    }
    None
}

/// status が島の語彙か（`superseded by ADR-0001` の形を含む）。
pub fn is_valid_status(status: &str) -> bool {
    if ADR_STATUSES.contains(&status) {
        return true;
    }
    let Some(number) = status.strip_prefix(ADR_SUPERSEDED_PREFIX) else {
        return false;
    };
    number.len() == 4 && number.chars().all(|c| c.is_ascii_digit())
}

fn extract_frontmatter(content: &str) -> Option<String> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    Some(rest[..end].to_string())
}

fn field(frontmatter: &str, key: &str) -> Option<String> {
    frontmatter.lines().find_map(|line| {
        line.strip_prefix(&format!("{key}:"))
            .map(|value| value.trim().to_string())
    })
}

fn is_valid_date(date: &str) -> bool {
    let parts: Vec<&str> = date.split('-').collect();
    parts.len() == 3
        && parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_digit()))
}

/// code fence と inline code を落とす。placeholder 判定の誤検出を防ぐため。
fn strip_code(content: &str) -> String {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    for line in content.split('\n') {
        let trimmed = line.trim_start();
        match fence {
            Some(marker) => {
                if trimmed.starts_with(marker) {
                    fence = None;
                }
            }
            None => {
                if let Some(marker) = ["```", "~~~"]
                    .into_iter()
                    .find(|marker| trimmed.starts_with(marker))
                {
                    fence = Some(marker);
                } else {
                    out.push(strip_inline_code(line));
                }
            }
        }
    }
    out.join("\n")
}

fn strip_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut inside = false;
    for ch in line.chars() {
        if ch == '`' {
            inside = !inside;
            continue;
        }
        if !inside {
            out.push(ch);
        }
    }
    out
}

/// `{...}`（改行を含まない）が残っているか。template の未記入プレースホルダ。
fn has_placeholder(text: &str) -> bool {
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let tail = &rest[open + 1..];
        match tail.find(['}', '\n']) {
            Some(index) if tail.as_bytes()[index] == b'}' && index > 0 => return true,
            Some(index) => rest = &tail[index..],
            None => break,
        }
    }
    false
}

/// `docs/adr/` 直下の ADR 本体を検査する: file 名、文書の構造、番号の重複。
///
/// 掲載回数は `docs/adr/README.md` の索引行で数え、一般文書の索引到達性は [`crate::docs`] が見る。
pub fn check(input: &Input) -> Vec<RuleFinding> {
    let prefix = format!("{ADR_DIR}/");
    let index_path = format!("{prefix}README.md");
    let index_counts = input
        .snapshot
        .markdown()
        .find_map(|(path, text)| (path == index_path).then_some(text))
        .and_then(Result::ok)
        .map(|text| index_entry_counts(&index_path, text))
        .unwrap_or_default();
    let mut findings = Vec::new();
    let mut numbers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut successors = Vec::new();
    for (path, text) in input.snapshot.markdown() {
        let Some(name) = path.strip_prefix(&prefix) else {
            continue;
        };
        if name.contains('/') || ADR_NON_RECORD_FILES.contains(&name) {
            continue;
        }
        let Some(number) = adr_file_number(name) else {
            findings.push(RuleFinding::new(
                Rule::Adr,
                path,
                "ADR の file 名は NNNN-kebab-case.md にしてください",
            ));
            continue;
        };
        numbers.entry(number).or_default().push(path);
        let count = index_counts.get(path).copied().unwrap_or(0);
        if count != 1 {
            findings.push(
                RuleFinding::new(
                    Rule::Adr,
                    path,
                    format!("{index_path} の索引にちょうど 1 回載せてください（現在 {count} 回）"),
                )
                .related([index_path.as_str()]),
            );
        }
        let Ok(text) = text else { continue };
        if let Some(problem) = document_problem(text, number) {
            findings.push(RuleFinding::new(Rule::Adr, path, problem.to_string()));
        }
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        if let Some(status) =
            extract_frontmatter(&normalized).and_then(|frontmatter| field(&frontmatter, "status"))
            && let Some(successor) = status.strip_prefix(ADR_SUPERSEDED_PREFIX)
            && is_valid_status(&status)
        {
            successors.push((path, successor.to_owned()));
        }
    }
    for (path, successor) in successors {
        if !numbers.contains_key(successor.as_str()) {
            findings.push(
                RuleFinding::new(
                    Rule::Adr,
                    path,
                    format!("後継 ADR-{successor} が存在しません"),
                )
                .related([ADR_DIR]),
            );
        }
    }
    for (number, paths) in numbers {
        if let [first, rest @ ..] = paths.as_slice()
            && !rest.is_empty()
        {
            findings.push(
                RuleFinding::new(
                    Rule::Adr,
                    first,
                    format!("ADR-{number} が重複しています: {}", paths.join(", ")),
                )
                .related(rest.iter().copied()),
            );
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "---\nstatus: accepted\ndate: 2026-09-30\n---\n\n# ADR-0001: 題\n\n\
        Chosen option: A\n\n- Good: 速い\n- Bad: 重い\n- Neutral: なし\n";

    #[test]
    fn a_standard_document_passes() {
        assert_eq!(document_problem(VALID, "0001"), None);
    }

    /// 島の語彙だけを受ける（実装ごとに割れていた `superseded` 単独 + `superseded-by`、単独の
    /// `superseded`、落とされていた `rejected` / `deprecated` の食い違いを 1 つにする）。
    #[test]
    fn status_follows_the_island_vocabulary() {
        for status in [
            "proposed",
            "accepted",
            "rejected",
            "deprecated",
            "superseded by ADR-0009",
        ] {
            assert!(is_valid_status(status), "{status}");
        }
        for status in ["superseded", "superseded by 0009", "Accepted", "done"] {
            assert!(!is_valid_status(status), "{status}");
        }
        let split_form = VALID.replace(
            "status: accepted",
            "status: superseded\nsuperseded-by: 0002",
        );
        assert_eq!(
            document_problem(&split_form, "0001"),
            Some(AdrProblem::InvalidStatus("superseded".to_owned()))
        );
    }

    #[test]
    fn each_structure_rule_fails_on_its_own() {
        let cases = [
            (
                VALID.replacen("---\n", "", 1),
                AdrProblem::MissingFrontmatter,
            ),
            (
                VALID.replace("status: accepted\n", ""),
                AdrProblem::MissingStatus,
            ),
            (
                VALID.replace("date: 2026-09-30", "date: 2026/09/30"),
                AdrProblem::InvalidDate("2026/09/30".to_owned()),
            ),
            (
                VALID.replace("# ADR-0001: ", "# ADR-0009: "),
                AdrProblem::MissingTitle("0001".to_owned()),
            ),
            (
                VALID.replace("Chosen option: A", "Chosen option: {option}"),
                AdrProblem::Placeholder,
            ),
            (
                VALID.replace("Chosen option: A\n", ""),
                AdrProblem::MissingChosenOption,
            ),
            (
                VALID.replace("- Bad: 重い\n", ""),
                AdrProblem::MissingConsequence("Bad"),
            ),
        ];
        for (text, problem) in cases {
            assert_eq!(document_problem(&text, "0001"), Some(problem));
        }
    }

    /// 移行済み ADR の原文（`## Original Record` 以降）と code の中の `{}` は見ない。
    #[test]
    fn original_record_and_code_are_not_judged() {
        let migrated = format!("{VALID}\n## Original Record\n\nstatus: whatever {{x}}\n");
        assert_eq!(document_problem(&migrated, "0001"), None);
        let code = VALID.replace("Chosen option: A", "Chosen option: `{x}`");
        assert_eq!(document_problem(&code, "0001"), None);
    }

    #[test]
    fn file_names_are_numbered_kebab_case() {
        assert_eq!(adr_file_number("0001-first-choice.md"), Some("0001"));
        for name in [
            "0001_first.md",
            "0001-.md",
            "01-x.md",
            "0001-Upper.md",
            "0001-a--b.md",
        ] {
            assert_eq!(adr_file_number(name), None, "{name}");
        }
    }

    #[test]
    fn adr_index_requires_exactly_one_link_and_successor_to_exist() {
        let mut input = Input::default();
        input
            .snapshot
            .add_markdown("docs/adr/0001-first.md", VALID.as_bytes().to_vec());
        input.snapshot.add_markdown(
            "docs/adr/0002-second.md",
            VALID.replace("ADR-0001", "ADR-0002").as_bytes().to_vec(),
        );
        input.snapshot.add_markdown(
            "docs/adr/README.md",
            b"# ADR\n- [one](0001-first.md)\n- [again](0001-first.md#x)\n".to_vec(),
        );
        let found: Vec<_> = check(&input).into_iter().map(|f| f.to_string()).collect();
        assert!(
            found
                .iter()
                .any(|f| f.contains("0001-first.md") && f.contains("2 回")),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|f| f.contains("0002-second.md") && f.contains("0 回")),
            "{found:?}"
        );

        input.snapshot.add_markdown(
            "docs/adr/README.md",
            b"# ADR\n- [one](0001-first.md)\n- [two](0002-second.md)\n".to_vec(),
        );
        input.snapshot.add_markdown(
            "docs/adr/0001-first.md",
            VALID
                .replace("status: accepted", "status: superseded by ADR-0003")
                .as_bytes()
                .to_vec(),
        );
        let found: Vec<_> = check(&input).into_iter().map(|f| f.to_string()).collect();
        assert!(
            found
                .iter()
                .any(|f| f.contains("後継 ADR-0003 が存在しません")),
            "{found:?}"
        );
        input.snapshot.add_markdown(
            "docs/adr/0001-first.md",
            VALID
                .replace("status: accepted", "status: superseded by ADR-0002")
                .as_bytes()
                .to_vec(),
        );
        assert!(check(&input).is_empty());
    }
}
