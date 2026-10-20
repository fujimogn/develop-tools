//! 見出しに絵文字を使わない（移設元: `check-docs` の `emoji.rs`。文字の範囲は
//! [`crate::constants::EMOJI_RANGES`]）。
//!
//! 対象は docs の Markdown と repo 直下の TODO / CHANGELOG の **`#` 見出しの行だけ**で、本文・表・
//! リストの絵文字は許す（2026-09-30 決定）。見出しだけを止める理由は anchor の揺れ: GitHub は見出しの
//! 絵文字を落として slug を作るが、`⚠️` のような絵文字は異体字セレクタ U+FE0F を伴い、落とし方が実装
//! （GitHub、各 repo の検査、[`crate::markdown::github_slug`]）ごとに違う。`## ⚠️ 注意` への link は
//! 書き手が見た slug と表示側の slug が食い違って切れやすい。fence の中の行は見出しではないので見ない
//! （design §5.5）。

use crate::check::Input;
use crate::constants::{CHANGELOG_PATH, DOCS_DIR, TODO_PATH, is_emoji, is_link_source};
use crate::markdown::{fence_mask, heading_text};
use crate::rules::{Rule, RuleFinding};

pub fn check(input: &Input) -> Vec<RuleFinding> {
    let repo = input.repo.as_deref();
    let docs_prefix = format!("{DOCS_DIR}/");
    let mut findings = Vec::new();
    for (path, text) in input.snapshot.markdown() {
        let in_scope = path == TODO_PATH
            || path == CHANGELOG_PATH
            || (path.starts_with(&docs_prefix) && is_link_source(repo, path));
        let (true, Ok(text)) = (in_scope, text) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (index, (line, fenced)) in lines.iter().zip(fence_mask(&lines)).enumerate() {
            if fenced {
                continue;
            }
            if heading_text(line).is_some_and(|heading| heading.chars().any(is_emoji)) {
                findings.push(
                    RuleFinding::new(
                        Rule::Emoji,
                        path,
                        "見出しに絵文字を使わないでください（anchor が揺れる。本文・表・リストは可）",
                    )
                    .at(index + 1),
                );
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(files: &[(&str, &str)]) -> Vec<String> {
        let mut input = Input::default();
        for (path, text) in files {
            input.snapshot.add_markdown(path, text.as_bytes().to_vec());
        }
        check(&input).iter().map(ToString::to_string).collect()
    }

    #[test]
    fn emoji_in_headings_fail_but_body_tables_lists_and_code_pass() {
        let files = [
            (
                "docs/a.md",
                "# a\n\n## ⚠️ 注意\n\n⚠️ 本文\n- ✅ リスト\n| ✅ | → |\n`✨ commit` は例\n```\n## 🐛 fence\n```\n### 見出し 🚀 ###\n",
            ),
            ("TODO.md", "# TODO\n\n## 進行中\n\n- [ ] **✅ 本文**\n"),
            ("CHANGELOG.md", "# 🚀 Changelog\n"),
            ("AGENTS.md", "## ⚠️ 範囲外\n"),
        ];
        assert_eq!(
            findings(&files),
            [
                "CHANGELOG.md:1: [emoji] 見出しに絵文字を使わないでください（anchor が揺れる。本文・表・リストは可）",
                "docs/a.md:3: [emoji] 見出しに絵文字を使わないでください（anchor が揺れる。本文・表・リストは可）",
                "docs/a.md:12: [emoji] 見出しに絵文字を使わないでください（anchor が揺れる。本文・表・リストは可）",
            ]
        );
    }
}
