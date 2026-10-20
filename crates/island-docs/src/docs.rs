//! 入口・配置・H1・索引（移設元: `check-docs` の `docs.rs`）。
//!
//! - 島の必須 file（[`REQUIRED_FILES`]）が tracked にある
//! - docs 直下の配置は、定数表に行のある repo だけ検査する（島では配置を決めていない）
//! - docs の各 Markdown の最初の非空行（frontmatter の後）は H1
//! - docs の各 Markdown は、同じ dir か上位 dir（`docs` まで）の索引（`README.md`）から link される
//! - scope 表で指定された文書は、指定された各 `README.md` から link される
//!
//! 移設元との差（design §5.5）: 索引は本文の部分一致ではなく link の宛先で判定し、下位の
//! `README.md` からの link も索引として数える（移設元の「ADR は `adr/README.md`」はこの特別な場合）。

use std::collections::{BTreeMap, BTreeSet};

use crate::check::Input;
use crate::constants::{
    DOCS_DIR, INDEX_FILE_NAME, REQUIRED_FILES, docs_layout_for, index_requirement_for,
    index_requirements_for, is_index_record_path, is_link_source,
};
use crate::links::{Resolution, resolve};
use crate::markdown::{fence_mask, link_targets};
use crate::rules::{Rule, RuleFinding};

pub fn check(input: &Input) -> Vec<RuleFinding> {
    let repo = input.repo.as_deref();
    let files: BTreeSet<&str> = input.snapshot.files().collect();
    let mut findings = Vec::new();

    for path in REQUIRED_FILES {
        if !files.contains(path) {
            findings.push(RuleFinding::new(
                Rule::RequiredFile,
                path,
                "島の必須 file が tracked にありません",
            ));
        }
    }

    let docs_prefix = format!("{DOCS_DIR}/");
    for path in files
        .iter()
        .filter(|path| path.starts_with(&docs_prefix) && is_link_source(repo, path))
    {
        let parts: Vec<&str> = path[docs_prefix.len()..].split('/').collect();
        let directory_invalid = parts[..parts.len() - 1]
            .iter()
            .any(|part| !kebab_case(part));
        let name = parts[parts.len() - 1];
        let stem = name.strip_suffix(".md").unwrap_or(name);
        if directory_invalid || (name != INDEX_FILE_NAME && !document_stem(stem)) {
            findings.push(RuleFinding::new(
                Rule::DocsName,
                path,
                "docs の dir と Markdown file 名は kebab-case にしてください（file の先頭 _ と数字.数字の版番号は可）",
            ));
        }
    }
    if let Some(layout) = docs_layout_for(repo) {
        let mut seen = BTreeSet::new();
        for path in files.iter().filter_map(|p| p.strip_prefix(&docs_prefix)) {
            match path.split_once('/') {
                Some((dir, _)) if seen.insert(dir) && !layout.allowed_dirs.contains(&dir) => {
                    findings.push(RuleFinding::new(
                        Rule::DocsLayout,
                        &format!("{docs_prefix}{dir}"),
                        format!(
                            "docs 直下に置ける dir は {} だけです",
                            layout.allowed_dirs.join(" / ")
                        ),
                    ));
                }
                None if path != INDEX_FILE_NAME => findings.push(RuleFinding::new(
                    Rule::DocsLayout,
                    &format!("{docs_prefix}{path}"),
                    format!("docs 直下には {INDEX_FILE_NAME} 以外を置けません"),
                )),
                _ => {}
            }
        }
    }

    let docs: Vec<(&str, &str)> = input
        .snapshot
        .markdown()
        .filter(|(path, _)| path.starts_with(&docs_prefix) && is_link_source(repo, path))
        .filter_map(|(path, text)| text.ok().map(|text| (path, text)))
        .collect();
    let indexed = index_links(&docs);
    let top_index = format!("{docs_prefix}{INDEX_FILE_NAME}");
    for (path, text) in &docs {
        if !first_content_line(text).is_some_and(|line| line.starts_with("# ")) {
            findings.push(RuleFinding::new(
                Rule::Heading,
                path,
                "最初の非空行（frontmatter の後）を H1 にしてください",
            ));
        }
        if *path == top_index {
            continue;
        }
        if let Some(requirement) = index_requirement_for(repo, path) {
            let missing: Vec<&str> = requirement
                .indexes
                .iter()
                .copied()
                .filter(|index| !indexed.get(*path).is_some_and(|from| from.contains(index)))
                .collect();
            if !missing.is_empty() {
                findings.push(
                    RuleFinding::new(
                        Rule::Index,
                        path,
                        format!(
                            "指定された索引から link されていません: {}",
                            missing.join(" / ")
                        ),
                    )
                    .related(missing),
                );
            }
            continue;
        }
        if is_index_record_path(path) {
            continue;
        }
        let candidates = index_candidates(path);
        let linked = indexed
            .get(*path)
            .is_some_and(|from| candidates.iter().any(|index| from.contains(index.as_str())));
        if !linked {
            findings.push(
                RuleFinding::new(
                    Rule::Index,
                    path,
                    format!(
                        "索引に載っていません（同じ dir か上位の {INDEX_FILE_NAME} から link してください）"
                    ),
                )
                .related(candidates.iter().map(String::as_str)),
            );
        }
    }
    for requirement in index_requirements_for(repo) {
        if !files.contains(requirement.path) {
            findings.push(RuleFinding::new(
                Rule::Index,
                requirement.path,
                "索引要求の対象文書が tracked にありません",
            ));
        }
    }
    findings
}

fn kebab_case(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn document_stem(value: &str) -> bool {
    let stem = value.strip_prefix('_').unwrap_or(value);
    !stem.is_empty()
        && stem.split('-').all(|word| {
            !word.is_empty()
                && if word.contains('.') {
                    word.split('.')
                        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                } else {
                    word.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                }
        })
}

/// 索引（docs の `README.md`）から link された path → link した索引。dir への link はその dir の
/// 索引への link として数える。
fn index_links<'a>(docs: &[(&'a str, &str)]) -> BTreeMap<String, BTreeSet<&'a str>> {
    let suffix = format!("/{INDEX_FILE_NAME}");
    let mut indexed: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for (index, text) in docs.iter().filter(|(path, _)| path.ends_with(&suffix)) {
        for link in link_targets(text) {
            if let Resolution::Local { path, .. } = resolve(index, &link.target) {
                indexed
                    .entry(format!("{path}{suffix}"))
                    .or_default()
                    .insert(index);
                indexed.entry(path).or_default().insert(index);
            }
        }
    }
    indexed
}

/// 索引の箇条書き・表の各行で、最初の link を掲載先として数える。
/// 説明欄の後継 ADR への参照は掲載行を増やさない。
pub(crate) fn index_entry_counts(index: &str, text: &str) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    let lines: Vec<_> = text.lines().collect();
    for (line, fenced) in lines.iter().zip(fence_mask(&lines)) {
        if fenced {
            continue;
        }
        let line = line.trim_start();
        if !(line.starts_with("- ") || line.starts_with("| ")) {
            continue;
        }
        if let Some(link) = link_targets(line).into_iter().next()
            && let Resolution::Local { path, .. } = resolve(index, &link.target)
        {
            *counts.entry(path).or_insert(0) += 1;
        }
    }
    counts
}

/// `path` を載せてよい索引: 同じ dir から `docs` までの各 dir の `README.md`（自分自身は除く）。
fn index_candidates(path: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut dir = path;
    while let Some((parent, _)) = dir.rsplit_once('/') {
        let index = format!("{parent}/{INDEX_FILE_NAME}");
        if index != path {
            candidates.push(index);
        }
        if parent == DOCS_DIR {
            break;
        }
        dir = parent;
    }
    candidates
}

/// 最初の非空行を返す。先頭が `---` なら次の `---` までを frontmatter として飛ばす。
/// 閉じていない frontmatter は全体を飛ばすので None になる。
pub fn first_content_line(text: &str) -> Option<&str> {
    let mut lines = text.lines().peekable();
    if lines.peek() == Some(&"---") {
        lines.next();
        for line in lines.by_ref() {
            if line == "---" {
                break;
            }
        }
    }
    lines.find(|line| !line.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(repo: Option<&str>, files: &[(&str, &str)]) -> Vec<String> {
        let mut input = Input {
            repo: repo.map(str::to_owned),
            ..Input::default()
        };
        for (path, text) in files {
            input.snapshot.add_markdown(path, text.as_bytes().to_vec());
        }
        check(&input)
            .iter()
            .filter(|finding| finding.rule != Rule::RequiredFile)
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn missing_required_files_are_reported() {
        let mut input = Input::default();
        input.snapshot.add_markdown("README.md", b"# r\n".to_vec());
        let found: Vec<_> = check(&input)
            .into_iter()
            .filter(|f| f.rule == Rule::RequiredFile)
            .map(|f| f.path)
            .collect();
        assert_eq!(found.len(), REQUIRED_FILES.len() - 1);
        assert!(found.contains(&"CONTEXT.md".to_owned()));
    }

    #[test]
    fn adr_index_entries_ignore_references_in_the_description() {
        let counts = index_entry_counts(
            "docs/adr/README.md",
            "# ADR\n- [0001](0001-one.md) — 後継 [0002](0002-two.md)\n| [`0002-two.md`](./0002-two.md) | [0001](0001-one.md) |\n```md\n- [duplicate](0001-one.md)\n```\n",
        );
        assert_eq!(counts.get("docs/adr/0001-one.md"), Some(&1));
        assert_eq!(counts.get("docs/adr/0002-two.md"), Some(&1));
    }

    #[test]
    fn documents_are_indexed_from_the_nearest_or_an_upper_index() {
        let files = [
            ("docs/README.md", "# d\n[g](guides/)\n[a](guides/a.md)\n"),
            ("docs/guides/README.md", "# g\n[b](b.md)\n"),
            ("docs/guides/a.md", "# a\n"),
            ("docs/guides/b.md", "# b\n"),
            ("docs/guides/c.md", "# c\n本文に c.md と書いただけ\n"),
        ];
        assert_eq!(
            findings(None, &files),
            [
                "docs/guides/c.md: [index] 索引に載っていません（同じ dir か上位の README.md から link してください）"
            ]
        );
    }

    /// 下位の索引は上位の索引から link されていないと、その下の文書ごと見つからない。
    #[test]
    fn a_sub_index_itself_needs_an_upper_index() {
        let files = [
            ("docs/guides/README.md", "# g\n[b](b.md)\n"),
            ("docs/guides/b.md", "# b\n"),
        ];
        assert_eq!(
            findings(None, &files),
            [
                "docs/guides/README.md: [index] 索引に載っていません（同じ dir か上位の README.md から link してください）"
            ]
        );
    }

    #[test]
    fn the_first_content_line_is_an_h1_after_frontmatter() {
        let files = [
            ("docs/README.md", "# d\n[a](a.md) [b](b.md)\n"),
            ("docs/a.md", "---\nstatus: x\n---\n\n# A\n"),
            ("docs/b.md", "\n本文\n# B\n"),
        ];
        assert_eq!(
            findings(None, &files),
            ["docs/b.md: [h1] 最初の非空行（frontmatter の後）を H1 にしてください"]
        );
        assert_eq!(first_content_line("---\nstatus: x\n# x\n"), None);
    }

    /// 配置は定数表に行のある repo だけ。他の repo の docs 直下は自由。
    #[test]
    fn layout_is_checked_only_for_repos_with_a_row() {
        let files = [
            (
                "docs/README.md",
                "# d\n[n](notes/n.md) [x](x.md) [g](guides/g.md)\n",
            ),
            ("docs/notes/n.md", "# n\n"),
            ("docs/x.md", "# x\n"),
            ("docs/guides/g.md", "# g\n"),
        ];
        let home = findings(Some("example/home"), &files);
        assert_eq!(home.len(), 2, "{home:?}");
        assert!(home[0].starts_with("docs/notes: [docs-layout]"), "{home:?}");
        assert!(home[1].starts_with("docs/x.md: [docs-layout]"), "{home:?}");
        assert!(findings(Some("owner/other"), &files).is_empty());
    }
}
